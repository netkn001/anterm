//! Native shell history parsers (zsh, bash, fish, PSReadLine) and legacy anterm
//! TSV format reader.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One parsed raw history record prior to DB insertion or snapshot normalization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawEntry {
    pub cmd: String,
    pub cwd: Option<String>,
    pub ts: Option<u64>,
    pub exit: Option<i32>,
    pub duration_ms: Option<u64>,
    pub session_id: Option<String>,
}

impl RawEntry {
    pub fn bare(cmd: String) -> Self {
        Self {
            cmd,
            cwd: None,
            ts: None,
            exit: None,
            duration_ms: None,
            session_id: None,
        }
    }
}

pub const FISH_HISTORY_FILE: &str = "fish_history";
pub const FISH_HISTORY_UNDER_DATA_DIR: &str = "fish/fish_history";
pub const FISH_HISTORY_UNDER_HOME: &str = ".local/share/fish/fish_history";

pub const PSREADLINE_HISTORY_FILE: &str = "ConsoleHost_history.txt";
pub const PSREADLINE_HISTORY_UNDER_APPDATA: &str =
    "Microsoft/Windows/PowerShell/PSReadLine/ConsoleHost_history.txt";
pub const PSREADLINE_HISTORY_UNDER_HOME: &str =
    "AppData/Roaming/Microsoft/Windows/PowerShell/PSReadLine/ConsoleHost_history.txt";
pub const PSREADLINE_HISTORY_UNDER_DATA_DIR: &str = "pwsh/PSReadLine/ConsoleHost_history.txt";

/// Written by the shell integration snippet, one absolute path per line.
pub const REPORTED_HISTORY_FILES: &str = "shell-histfiles";

/// History files worth fetching from a remote host, relative to its home dir.
pub fn shell_history_names() -> [&'static str; 4] {
    [
        ".zsh_history",
        ".bash_history",
        FISH_HISTORY_UNDER_HOME,
        PSREADLINE_HISTORY_UNDER_HOME,
    ]
}

/// Return the single expected remote history file path when the remote shell is
/// already known, avoiding unnecessary SFTP round trips for unrelated shells.
pub fn shell_history_name_for_shell(shell_program: &str) -> Option<&'static str> {
    let stem = Path::new(shell_program)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match stem.as_str() {
        "zsh" => Some(".zsh_history"),
        "bash" => Some(".bash_history"),
        "fish" => Some(FISH_HISTORY_UNDER_HOME),
        "pwsh" | "powershell" => Some(PSREADLINE_HISTORY_UNDER_HOME),
        _ => None,
    }
}

pub fn looks_absolute(p: &str) -> bool {
    match p.as_bytes() {
        [b'/' | b'\\', ..] => true,
        [d, b':', ..] => d.is_ascii_alphabetic(),
        _ => false,
    }
}

/// Parse one line from anterm's legacy TSV history file (all 3 generations).
pub fn parse_own_line(line: &str) -> RawEntry {
    let mut f = line.splitn(4, '\t');
    if let (Some(ts), Some(exit), Some(cwd), Some(cmd)) = (f.next(), f.next(), f.next(), f.next())
        && !ts.is_empty()
        && ts.bytes().all(|b| b.is_ascii_digit())
        && (exit.is_empty() || exit.parse::<i32>().is_ok())
        && (cwd.is_empty() || looks_absolute(cwd))
    {
        return RawEntry {
            cmd: cmd.to_string(),
            cwd: (!cwd.is_empty()).then(|| cwd.to_string()),
            ts: ts.parse().ok(),
            exit: exit.parse().ok(),
            duration_ms: None,
            session_id: None,
        };
    }
    if let Some((cwd, cmd)) = line.split_once('\t')
        && looks_absolute(cwd)
    {
        return RawEntry {
            cmd: cmd.to_string(),
            cwd: Some(cwd.to_string()),
            ts: None,
            exit: None,
            duration_ms: None,
            session_id: None,
        };
    }
    RawEntry::bare(line.to_string())
}

/// Discover all local shell history files present on this machine, ordered by
/// modification time (oldest first).
pub fn local_shell_history_paths() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |p: PathBuf| {
        if p.is_file() && seen.insert(p.clone()) {
            files.push(p);
        }
    };
    if let Some(hf) = std::env::var_os("HISTFILE") {
        add(PathBuf::from(hf));
    }
    for hf in reported_history_files() {
        add(hf);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        add(home.join(".zsh_history"));
        add(home.join(".bash_history"));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        add(PathBuf::from(xdg).join(FISH_HISTORY_UNDER_DATA_DIR));
    } else if let Some(home) = std::env::var_os("HOME") {
        add(PathBuf::from(home).join(FISH_HISTORY_UNDER_HOME));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
        add(PathBuf::from(appdata).join(PSREADLINE_HISTORY_UNDER_APPDATA));
    } else if let Some(home) = std::env::var_os("HOME") {
        add(PathBuf::from(home).join(PSREADLINE_HISTORY_UNDER_HOME));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        add(PathBuf::from(xdg).join(PSREADLINE_HISTORY_UNDER_DATA_DIR));
    } else if cfg!(not(windows))
        && let Some(home) = std::env::var_os("HOME")
    {
        add(PathBuf::from(home)
            .join(".local/share")
            .join(PSREADLINE_HISTORY_UNDER_DATA_DIR));
    }
    files.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    files
}

fn reported_history_files() -> Vec<PathBuf> {
    crate::config::config_path(REPORTED_HISTORY_FILES)
        .and_then(|list| std::fs::read_to_string(list).ok())
        .map(|text| parse_reported_history_files(&text))
        .unwrap_or_default()
}

pub fn parse_reported_history_files(text: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut files: Vec<PathBuf> = text
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| {
            looks_absolute(line)
                && !line.contains("/anterm-zdotdir-")
                && !line.contains("\\anterm-zdotdir-")
                && !line.contains("/pane-history/")
                && !line.contains("\\pane-history\\")
                && seen.insert(*line)
        })
        .take(32)
        .map(PathBuf::from)
        .collect();
    files.reverse();
    files
}

/// zsh's escape byte (`Meta` = 0x83).
const ZSH_META: u8 = 0x83;

pub fn decode_history_file(name: &str, bytes: &[u8]) -> String {
    if is_zsh_history(name, bytes) {
        String::from_utf8_lossy(&unmetafy(bytes)).into_owned()
    } else if name == PSREADLINE_HISTORY_FILE {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn is_zsh_history(name: &str, bytes: &[u8]) -> bool {
    name.ends_with("zsh_history")
        || name == ".zhistory"
        || bytes.split(|&b| b == b'\n').any(is_zsh_extended_line)
}

fn is_zsh_extended_line(line: &[u8]) -> bool {
    let Some(rest) = line.strip_prefix(b": ") else {
        return false;
    };
    let Some(semi) = rest.iter().position(|&b| b == b';') else {
        return false;
    };
    let mut fields = rest[..semi].split(|&b| b == b':');
    let digits = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_digit);
    matches!(
        (fields.next(), fields.next(), fields.next()),
        (Some(start), Some(elapsed), None) if digits(start) && digits(elapsed)
    )
}

fn unmetafy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter().copied();
    while let Some(b) = it.next() {
        if b == ZSH_META
            && let Some(next) = it.next()
        {
            out.push(next ^ 0x20);
        } else {
            out.push(b);
        }
    }
    out
}

pub fn parse_history_file(name: &str, content: &str, out: &mut Vec<RawEntry>) {
    if name == FISH_HISTORY_FILE {
        parse_fish_history(content, out);
    } else if name == PSREADLINE_HISTORY_FILE {
        parse_psreadline_history(content, out);
    } else {
        parse_shell_history(content, out);
    }
}

pub fn parse_psreadline_history(content: &str, out: &mut Vec<RawEntry>) {
    for raw in content.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw).trim();
        if !line.is_empty() && !line.ends_with('`') {
            out.push(RawEntry::bare(line.to_string()));
        }
    }
}

/// Parse `fish_history` directly (preserving multiline commands that fish
/// escapes with `\n`, now natively supported by SQLite and the inline editor).
pub fn parse_fish_history(content: &str, out: &mut Vec<RawEntry>) {
    let mut pending: Option<RawEntry> = None;
    for raw in content.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(cmd) = line.strip_prefix("- cmd:") {
            out.extend(pending.take());
            let cmd = unescape_fish(cmd.trim());
            if !cmd.trim().is_empty() {
                pending = Some(RawEntry::bare(cmd));
            }
            continue;
        }
        if pending.is_none() {
            continue;
        }
        let Some(key) = line.strip_prefix("  ") else {
            out.extend(pending.take());
            continue;
        };
        if let Some(when) = key.strip_prefix("when:")
            && let Ok(ts) = when.trim().trim_matches('\'').parse::<u64>()
            && let Some(entry) = pending.as_mut()
        {
            entry.ts = Some(ts);
        }
    }
    out.extend(pending);
}

fn unescape_fish(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let mut tail = rest[at..].chars();
        tail.next();
        match tail.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            _ => {
                out.push('\\');
                rest = &rest[at + 1..];
                continue;
            }
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// Parse zsh (extended or plain) and bash (with or without `#timestamp` lines),
/// rejoining backslash-continued multiline commands cleanly instead of emitting
/// broken fragments.
pub fn parse_shell_history(content: &str, out: &mut Vec<RawEntry>) {
    let mut pending_ts: Option<u64> = None;
    let mut lines = content.split('\n').peekable();

    while let Some(raw) = lines.next() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(ts) = bash_timestamp(line) {
            pending_ts = Some(ts);
            continue;
        }
        let Some((cmd_head, zsh_ts, zsh_elapsed)) = start_of_command(line) else {
            pending_ts = None;
            continue;
        };

        let mut cmd = cmd_head.to_string();
        while ends_with_unescaped_backslash(&cmd) {
            let Some(next_raw) = lines.peek() else {
                break;
            };
            let next_line = next_raw.strip_suffix('\r').unwrap_or(next_raw);
            // Guard against a stray trailing backslash swallowing the start of
            // a brand-new zsh extended or bash timestamped record.
            if bash_timestamp(next_line).is_some()
                || start_of_command(next_line).is_some_and(|(_, ts, _)| ts.is_some())
            {
                break;
            }
            lines.next();
            cmd.pop(); // strip continuation backslash
            cmd.push('\n');
            cmd.push_str(next_line);
        }

        let trimmed = cmd.trim();
        if !trimmed.is_empty() {
            out.push(RawEntry {
                cmd: trimmed.to_string(),
                cwd: None,
                ts: zsh_ts.or(pending_ts),
                exit: None,
                duration_ms: zsh_elapsed.filter(|&s| s > 0).map(|s| s.saturating_mul(1_000)),
                session_id: None,
            });
        }
        pending_ts = None;
    }
}

fn ends_with_unescaped_backslash(s: &str) -> bool {
    let trailing = s.bytes().rev().take_while(|&b| b == b'\\').count();
    trailing % 2 == 1
}

fn bash_timestamp(line: &str) -> Option<u64> {
    let rest = line.strip_prefix('#')?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn start_of_command(line: &str) -> Option<(&str, Option<u64>, Option<u64>)> {
    if line.is_empty() {
        return None;
    }
    if let Some(rest) = line.strip_prefix(": ")
        && let Some(semi) = rest.find(';')
    {
        let meta = &rest[..semi];
        if meta.bytes().any(|b| b.is_ascii_digit())
            && meta.bytes().all(|b| b.is_ascii_digit() || b == b':')
        {
            let mut parts = meta.split(':');
            let start = parts.next().and_then(|t| t.parse::<u64>().ok());
            let elapsed = parts.next().and_then(|t| t.parse::<u64>().ok());
            return Some((&rest[semi + 1..], start, elapsed));
        }
    }
    Some((line, None, None))
}
