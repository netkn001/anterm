//! A history file per pane, instead of one everybody writes into.
//!
//! Used by shell-level `per_pane_history` (when a user drives zsh/bash directly
//! or runs `prompt_editor: false`). Stored under `<config_dir>/pane-history/`
//! — deliberately separate from the GUI's `history.db` (and legacy `history`
//! file) so the two never collide on the same filesystem path.
//!
//! Two invariants govern how each pane's file behaves:
//!
//! - **The pane does not start blank.** The integration snippet seeds the pane's
//!   private file atomically from the tail of the shell's real `HISTFILE` and
//!   enables incremental appending (`INC_APPEND_HISTORY` / `history -a`) so
//!   commands survive forced closes.
//! - **Nothing is lost and nothing is corrupted on close.** When the pane closes
//!   for good, `retire()` merges only genuinely new records back into `origin`:
//!   - Fast path: verify the seeded prefix is still intact using both byte
//!     length (`.seed`) and tail fingerprint (`.seed.tail`), then append the
//!     exact suffix.
//!   - Fallback path: when the shell rewrote its private file (e.g.
//!     `HIST_SAVE_NO_DUPS`, `HIST_IGNORE_ALL_DUPS`, `HISTCONTROL=erasedups`),
//!     parse both files into logical shell records and append only whole records
//!     not already present in `origin` — never slicing mid-line or mid-UTF-8.
//!   - Hibernating or gracefully restarting the daemon preserves the pane's file
//!     instead of retiring it, so [`carry`] hands it straight to the replacement
//!     pane.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The pane's own history file, as handed to the shell.
pub const FILE_ENV: &str = "ANTERM_HISTFILE";

const SIDECARS: [&str; 4] = ["", ".seed", ".seed.tail", ".origin"];

/// Shells whose integration snippet knows what to do with [`FILE_ENV`].
fn understands(shell: &str) -> bool {
    let stem = Path::new(shell)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(stem.as_str(), "zsh" | "bash")
}

pub fn enabled() -> bool {
    crate::config::Config::load().per_pane_history
}

fn dir() -> Option<PathBuf> {
    crate::config::config_path("pane-history")
}

pub fn path_for(pane: u64) -> Option<PathBuf> {
    Some(dir()?.join(format!("pane-{pane}")))
}

/// What to put in a pane's environment, if it should have a history of its own.
pub fn env_for(pane: u64, shell: &str) -> Option<(String, String)> {
    if !understands(shell) || !enabled() {
        return None;
    }
    let path = path_for(pane)?;
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::debug!("no pane-history directory ({e}); pane {pane} shares the usual one");
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    Some((FILE_ENV.to_string(), path.to_string_lossy().into_owned()))
}

/// Hand a dead or hibernated pane's history to the pane that replaces it.
pub fn carry(from: u64, to: u64) {
    let (Some(old), Some(new)) = (path_for(from), path_for(to)) else {
        return;
    };
    if !old.exists() {
        return;
    }
    for suffix in SIDECARS {
        let (a, b) = (with_suffix(&old, suffix), with_suffix(&new, suffix));
        let _ = std::fs::rename(&a, &b);
    }
}

/// Give a closed pane's commands back to the history file they came from, then
/// remove the pane's private files.
pub fn retire(pane: u64) {
    let Some(path) = path_for(pane) else {
        return;
    };
    if !path.exists() {
        for suffix in SIDECARS {
            let _ = std::fs::remove_file(with_suffix(&path, suffix));
        }
        return;
    }
    merge_back(&path);
    for suffix in SIDECARS {
        let _ = std::fs::remove_file(with_suffix(&path, suffix));
    }
}

fn merge_back(path: &Path) {
    let Some(origin) = read_origin(&with_suffix(path, ".origin")) else {
        return;
    };
    let seeded: usize = std::fs::read_to_string(with_suffix(path, ".seed"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    let seed_tail = std::fs::read(with_suffix(path, ".seed.tail")).ok();
    let Ok(body) = std::fs::read(path) else {
        return;
    };
    if body.is_empty() {
        return;
    }

    let added = extract_added(&body, seeded, seed_tail.as_deref(), &origin);
    if added.is_empty() {
        return;
    }

    if let Err(e) = append_locked(&origin, &added) {
        log::debug!(
            "pane history could not be merged into {}: {e}",
            origin.display()
        );
    }
}

/// Decide which bytes from `body` are genuinely new to `origin`.
///
/// Fast path: when the seeded prefix is still byte-identical (verified by
/// length and tail fingerprint, or legacy `.seed` without `.seed.tail` when
/// `body.len() >= seeded`), slice `&body[seeded..]`.
///
/// Fallback path: when the shell rewrote/deduped its file (e.g. zsh
/// `HIST_SAVE_NO_DUPS` or bash `HISTCONTROL=erasedups`), parse whole logical
/// records from `origin` and `body` and return only records not already present
/// in `origin`.
fn extract_added(
    body: &[u8],
    seeded: usize,
    seed_tail: Option<&[u8]>,
    origin: &Path,
) -> Vec<u8> {
    let prefix_intact = if body.len() >= seeded {
        match seed_tail {
            Some(tail) if !tail.is_empty() => {
                let start = seeded.saturating_sub(tail.len());
                body.get(start..seeded) == Some(tail)
            }
            // No tail fingerprint (e.g., seeded == 0 or legacy sidecar): trust
            // the boundary when seeded == 0, or when `body[seeded - 1] == b'\n'`.
            _ => seeded == 0 || body.get(seeded.saturating_sub(1)) == Some(&b'\n'),
        }
    } else {
        false
    };

    if prefix_intact {
        let slice = &body[seeded..];
        if slice.is_empty() {
            return Vec::new();
        }
        return slice.to_vec();
    }

    // Rewritten/shrunk/shifted file: fall back to record-level diff so we
    // neither silently lose new commands nor slice mid-line/mid-UTF-8.
    let origin_bytes = std::fs::read(origin).unwrap_or_default();
    let origin_records = split_shell_records(&origin_bytes);
    let known: HashSet<&[u8]> = origin_records.into_iter().collect();

    let mut out = Vec::new();
    for rec in split_shell_records(body) {
        if !known.contains(rec) {
            out.extend_from_slice(rec);
            if !out.ends_with(b"\n") {
                out.push(b'\n');
            }
        }
    }
    out
}

/// Split raw shell history bytes into logical record slices (preserving raw
/// bytes, including zsh metafied sequences). Handles:
/// - zsh extended history records (`: <ts>:<elapsed>;...`) with `\` continuations
/// - bash timestamped records (`#<digits>\n<cmd>`) with `\` continuations
/// - plain single-line and `\`-continued records
fn split_shell_records(bytes: &[u8]) -> Vec<&[u8]> {
    let mut records: Vec<&[u8]> = Vec::new();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        let start = i;
        // Advance through optional bash `#<digits>\n` timestamp line.
        let mut line_end = next_line_end(bytes, i);
        let first_line = strip_crlf(&bytes[i..line_end]);
        if is_bash_ts_line(first_line) && line_end < n {
            i = line_end;
            line_end = next_line_end(bytes, i);
        }
        // Advance through backslash-continued physical lines.
        loop {
            let line = strip_crlf(&bytes[i..line_end]);
            i = line_end;
            if ends_with_unescaped_backslash(line) && i < n {
                line_end = next_line_end(bytes, i);
            } else {
                break;
            }
        }
        let rec = strip_crlf(&bytes[start..i]);
        if !rec.is_empty() {
            records.push(rec);
        }
    }
    records
}

fn next_line_end(bytes: &[u8], from: usize) -> usize {
    match memchr::memchr(b'\n', &bytes[from..]) {
        Some(off) => from + off + 1,
        None => bytes.len(),
    }
}

fn strip_crlf(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn is_bash_ts_line(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b"#") else {
        return false;
    };
    !rest.is_empty() && rest.iter().all(u8::is_ascii_digit)
}

fn ends_with_unescaped_backslash(line: &[u8]) -> bool {
    let trailing = line.iter().rev().take_while(|&&b| b == b'\\').count();
    trailing % 2 == 1
}

/// Append `added` to `origin` under an advisory exclusive lock (on Unix),
/// ensuring a newline separates the existing content from `added` when the
/// existing file does not end with `\n`.
fn append_locked(origin: &Path, added: &[u8]) -> std::io::Result<()> {
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(origin)?;

    #[cfg(unix)]
    let _lock = FileLock::exclusive(&file);

    let len = file.seek(SeekFrom::End(0))?;
    if len > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        if file.read_exact(&mut last).is_ok()
            && last[0] != b'\n'
            && !added.starts_with(b"\n")
            && !added.starts_with(b"\r\n")
        {
            file.seek(SeekFrom::End(0))?;
            file.write_all(b"\n")?;
        } else {
            file.seek(SeekFrom::End(0))?;
        }
    }
    file.write_all(added)?;
    file.flush()
}

#[cfg(unix)]
struct FileLock {
    fd: std::os::fd::RawFd,
}

#[cfg(unix)]
impl FileLock {
    fn exclusive(file: &std::fs::File) -> Option<Self> {
        use std::os::fd::AsRawFd as _;
        let fd = file.as_raw_fd();
        let rc = unsafe { libc::flock(fd, libc::LOCK_EX) };
        (rc == 0).then_some(Self { fd })
    }
}

#[cfg(unix)]
impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.fd, libc::LOCK_UN);
        }
    }
}

/// The path the shell said it seeded from, if it is one worth writing to.
fn read_origin(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    let origin = PathBuf::from(text.trim());
    if !origin.is_absolute() || origin.is_dir() {
        return None;
    }
    let parent_exists = origin.parent().is_some_and(Path::is_dir);
    (origin.is_file() || parent_exists).then_some(origin)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    if suffix.is_empty() {
        return path.to_path_buf();
    }
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Drop the files of panes that no longer exist.
pub fn sweep(keep: &HashSet<u64>) {
    let Some(dir) = dir() else {
        return;
    };
    sweep_in(&dir, keep);
}

fn sweep_in(dir: &Path, keep: &HashSet<u64>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let stem = name
            .split_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(name.as_ref());
        let Some(id) = stem
            .strip_prefix("pane-")
            .and_then(|id| id.parse::<u64>().ok())
        else {
            continue;
        };
        if !keep.contains(&id) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin_config_dir() {
        let dir = std::env::temp_dir().join(format!("anterm-covtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        crate::config::set_config_dir(dir);
    }

    fn seed(pane: u64, origin: &Path) -> PathBuf {
        let path = path_for(pane).expect("a path under the config dir");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let seeded = std::fs::read(origin).unwrap_or_default();
        std::fs::write(&path, &seeded).unwrap();
        std::fs::write(with_suffix(&path, ".seed"), seeded.len().to_string()).unwrap();
        let tail_start = seeded.len().saturating_sub(512);
        std::fs::write(with_suffix(&path, ".seed.tail"), &seeded[tail_start..]).unwrap();
        std::fs::write(
            with_suffix(&path, ".origin"),
            format!("{}\n", origin.display()),
        )
        .unwrap();
        path
    }

    fn a_global_history(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("anterm-history-{}-{name}", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn pane_history_dir_does_not_collide_with_legacy_history_file() {
        pin_config_dir();
        let pane_dir = dir().unwrap();
        assert_ne!(
            pane_dir.file_name().and_then(|s| s.to_str()),
            Some("history"),
            "pane-history directory must not share a path with the GUI history file"
        );
    }

    #[test]
    fn a_closed_pane_gives_back_only_what_it_added() {
        pin_config_dir();
        let origin = a_global_history("added", "old command\n");
        let pane = seed(80_001, &origin);
        std::fs::write(&pane, "old command\nnew command\n").unwrap();

        retire(80_001);

        assert_eq!(
            std::fs::read_to_string(&origin).unwrap(),
            "old command\nnew command\n",
        );
        assert!(!pane.exists());
        for suffix in SIDECARS {
            assert!(!with_suffix(&pane, suffix).exists());
        }
        let _ = std::fs::remove_file(&origin);
    }

    #[test]
    fn a_pane_that_ran_nothing_does_not_touch_the_users_history() {
        pin_config_dir();
        let origin = a_global_history("untouched", "old command\n");
        seed(80_002, &origin);

        retire(80_002);

        assert_eq!(std::fs::read_to_string(&origin).unwrap(), "old command\n");
        let _ = std::fs::remove_file(&origin);
    }

    #[test]
    fn a_rewritten_deduped_file_merges_only_whole_new_records_without_corruption() {
        pin_config_dir();
        let origin = a_global_history(
            "rewritten",
            ": 1700000000:0;dup cmd\n: 1700000001:0;keep cmd\n: 1700000002:0;dup cmd\n",
        );
        let pane = seed(80_003, &origin);
        // Simulate zsh HIST_SAVE_NO_DUPS removing the earlier duplicate and
        // adding a brand-new CJK command: byte offsets shifted, AND total len
        // may be longer or shorter than `seeded`.
        std::fs::write(
            &pane,
            ": 1700000001:0;keep cmd\n: 1700000002:0;dup cmd\n: 1700000099:0;echo 你好世界\n",
        )
        .unwrap();

        retire(80_003);

        let merged = std::fs::read_to_string(&origin).unwrap();
        assert!(
            merged.contains(": 1700000099:0;echo 你好世界\n"),
            "new command must survive HIST_SAVE_NO_DUPS rewrite: {merged:?}"
        );
        assert_eq!(
            merged.matches("keep cmd").count(),
            1,
            "seed records must not be duplicated: {merged:?}"
        );
        let _ = std::fs::remove_file(&origin);
    }

    #[test]
    fn merging_into_origin_without_trailing_newline_does_not_fuse_lines() {
        pin_config_dir();
        let origin = a_global_history("no-nl", "old command");
        let pane = seed(80_010, &origin);
        std::fs::write(&pane, "old command\nnew command\n").unwrap();

        retire(80_010);

        let merged = std::fs::read_to_string(&origin).unwrap();
        assert_eq!(merged, "old command\nnew command\n");
        let _ = std::fs::remove_file(&origin);
    }

    #[test]
    fn an_origin_that_is_not_a_real_absolute_file_is_refused() {
        pin_config_dir();
        let path = path_for(80_004).expect("a path");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "some command\n").unwrap();
        std::fs::write(with_suffix(&path, ".seed"), "0").unwrap();
        std::fs::write(with_suffix(&path, ".origin"), "../relative/history\n").unwrap();

        retire(80_004);
        assert!(!Path::new("../relative/history").exists());
    }

    #[test]
    fn a_restored_pane_inherits_its_predecessors_history() {
        pin_config_dir();
        let origin = a_global_history("carried", "old command\n");
        let before = seed(80_005, &origin);
        std::fs::write(&before, "old command\nfrom the dead pane\n").unwrap();

        carry(80_005, 80_006);

        assert!(!before.exists());
        let after = path_for(80_006).expect("a path");
        assert_eq!(
            std::fs::read_to_string(&after).unwrap(),
            "old command\nfrom the dead pane\n",
        );
        assert_eq!(
            std::fs::read_to_string(with_suffix(&after, ".seed")).unwrap(),
            "12",
        );

        retire(80_006);
        assert_eq!(
            std::fs::read_to_string(&origin).unwrap(),
            "old command\nfrom the dead pane\n"
        );
        let _ = std::fs::remove_file(&origin);
    }

    #[test]
    fn the_sweep_keeps_only_the_panes_that_still_exist() {
        let dir = std::env::temp_dir().join(format!("anterm-sweeptest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let kept = dir.join("pane-80007");
        let dropped = dir.join("pane-80008");
        std::fs::write(&kept, "old\n").unwrap();
        std::fs::write(&dropped, "old\n").unwrap();
        std::fs::write(with_suffix(&dropped, ".seed"), "0").unwrap();
        std::fs::write(with_suffix(&dropped, ".seed.tail"), "old\n").unwrap();

        sweep_in(&dir, &HashSet::from([80_007]));

        assert!(kept.exists());
        assert!(!dropped.exists());
        assert!(!with_suffix(&dropped, ".seed").exists());
        assert!(!with_suffix(&dropped, ".seed.tail").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_shells_whose_integration_knows_the_variable_are_offered_it() {
        assert!(understands("/bin/zsh"));
        assert!(understands("/opt/homebrew/bin/bash"));
        assert!(!understands("/bin/sh"));
        assert!(!understands("/usr/local/bin/fish"));
        assert!(!understands("pwsh.exe"));
        assert!(!understands("C:\\Windows\\System32\\cmd.exe"));
    }
}
