//! What a pane looked like, kept where the daemon's own death cannot reach it.
//!
//! The replay ring already holds every pane's recent output so a reattaching
//! client can be shown the screen it left. It holds it in this process, which
//! makes it exactly as durable as this process: a crash, a `kill -9`, or a
//! reboot takes the panes and their scrollback together, and the window comes
//! back to a row of blank shells with no trace of what was in them.
//!
//! The planned upgrade path does not lose anything — see `daemon::handoff`,
//! which carries the live ptys and their rings into the new binary without the
//! shells ever noticing. This module is for the paths a handoff cannot cover,
//! where the process does not get to run any code on its way out. It cannot
//! keep the *processes*; nothing written to a file can. It keeps the picture.
//!
//! Consequences of that being the goal:
//!
//! - **The snapshot is periodic, not write-through.** Every pty byte reaching
//!   the disk would be an enormous amount of write amplification to buy a few
//!   seconds of freshness at the tail of a crash. The writer runs on a timer
//!   and skips panes whose ring has not changed.
//! - **It is capped and compressed.** [`SNAPSHOT_CAP`] keeps what fills several
//!   screens, compressed with deflate (`ANTERMSB\x02\x00`), shrinking on-disk
//!   size by 4–6× while remaining backwards-compatible with v1 snapshots.
//! - **It is dropped as soon as it is meaningless.** A pane that is closed,
//!   cleared via `ClearScrollback`, or that no workspace refers to any more, has
//!   its file removed, and dormant snapshots are bounded by [`MAX_STORAGE_BYTES`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::daemon::pane::HeadCut;
use crate::daemon::protocol::WinSize;

/// How much of a pane's ring reaches the disk, per pane (uncompressed bytes).
///
/// A screenful of dense output is on the order of 20 KiB once escape sequences
/// are counted, so this is "the last several screens" rather than "everything".
/// The in-memory ring stays at its own, much larger, cap: this bound is about
/// what is worth persisting, not what is worth keeping while running.
pub const SNAPSHOT_CAP: usize = 256 * 1024;

/// Maximum uncompressed payload accepted when inflating a v2 snapshot,
/// guarding against corrupt or maliciously crafted files.
const MAX_INFLATE_BYTES: usize = 16 * SNAPSHOT_CAP;

/// Total on-disk budget for scrollback snapshots across dormant/closed panes.
/// Live panes are never evicted; oldest non-live snapshots are dropped first
/// when total usage exceeds this cap.
pub const MAX_STORAGE_BYTES: u64 = 50 * 1024 * 1024;

/// How often the writer looks for panes whose ring has moved.
pub const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

const MAGIC_V1: &[u8; 10] = b"ANTERMSB\x01\x00";
const MAGIC_V2: &[u8; 10] = b"ANTERMSB\x02\x00";

/// One geometry-homogeneous run of pane output, the unit the replay ring is
/// segmented into: replaying a segment means telling the client the size those
/// bytes were written at, then handing it the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub size: WinSize,
    pub bytes: Vec<u8>,
}

/// Drop bytes from the oldest segments until the total fits `cap`, advancing
/// past any split VT escape sequence or UTF-8 continuation byte so the kept
/// tail always begins at a clean parser boundary.
///
/// Trimming from the front is what makes a truncated snapshot still make sense:
/// terminal output is only interpretable forwards, so the tail can stand on its
/// own in a way the head cannot. A partially eaten segment keeps its geometry —
/// the bytes that remain were still written at that size.
pub fn trim_to(segments: &mut Vec<Segment>, cap: usize) {
    let total: usize = segments.iter().map(|s| s.bytes.len()).sum();
    let mut over = total.saturating_sub(cap);
    if over == 0 {
        return;
    }
    let mut head = HeadCut::default();
    while over > 0 {
        let Some(head_seg) = segments.first_mut() else {
            return;
        };
        let drop = over.min(head_seg.bytes.len());
        head.fold(&head_seg.bytes[..drop]);
        head_seg.bytes.drain(..drop);
        over -= drop;
        if !head_seg.bytes.is_empty() {
            // Advance past any incomplete VT sequence or UTF-8 continuation byte.
            let mut extra = 0;
            while extra < head_seg.bytes.len() && !head.at_boundary(head_seg.bytes[extra]) {
                head.step(head_seg.bytes[extra]);
                extra += 1;
            }
            if extra > 0 {
                head_seg.bytes.drain(..extra);
            }
            if !head_seg.bytes.is_empty() {
                return;
            }
        }
        // The last segment is kept even when it is empty: the ring this feeds
        // back into is defined by always having a tail to append to.
        if segments.len() == 1 {
            return;
        }
        segments.remove(0);
    }
}

fn encode_payload(segments: &[Segment], title: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        4 + segments.iter().map(|s| s.bytes.len() + 12).sum::<usize>()
            + title.map_or(0, |t| 4 + t.len()),
    );
    out.extend_from_slice(&(segments.len() as u32).to_le_bytes());
    for seg in segments {
        out.extend_from_slice(&seg.size.cols.to_le_bytes());
        out.extend_from_slice(&seg.size.rows.to_le_bytes());
        out.extend_from_slice(&seg.size.cell_w.to_le_bytes());
        out.extend_from_slice(&seg.size.cell_h.to_le_bytes());
        out.extend_from_slice(&(seg.bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&seg.bytes);
    }
    if let Some(title) = title {
        out.extend_from_slice(&(title.len() as u32).to_le_bytes());
        out.extend_from_slice(title.as_bytes());
    }
    out
}

pub fn encode(segments: &[Segment], title: Option<&str>) -> Vec<u8> {
    let payload = encode_payload(segments, title);
    let compressed = miniz_oxide::deflate::compress_to_vec(&payload, 6);
    let mut out = Vec::with_capacity(MAGIC_V2.len() + compressed.len());
    out.extend_from_slice(MAGIC_V2);
    out.extend_from_slice(&compressed);
    out
}

#[cfg(test)]
fn encode_v1(segments: &[Segment], title: Option<&str>) -> Vec<u8> {
    let payload = encode_payload(segments, title);
    let mut out = Vec::with_capacity(MAGIC_V1.len() + payload.len());
    out.extend_from_slice(MAGIC_V1);
    out.extend_from_slice(&payload);
    out
}

/// `None` for anything that is not a snapshot this build wrote.
///
/// Accepts both v2 (deflate-compressed) and v1 (uncompressed) snapshots so
/// upgrades across releases remain seamless.
pub fn decode(raw: &[u8]) -> Option<(Vec<Segment>, Option<String>)> {
    if let Some(compressed) = raw.strip_prefix(MAGIC_V2.as_slice()) {
        let inflated =
            miniz_oxide::inflate::decompress_to_vec_with_limit(compressed, MAX_INFLATE_BYTES)
                .ok()?;
        return decode_payload(&inflated);
    }
    let cur = raw.strip_prefix(MAGIC_V1.as_slice())?;
    decode_payload(cur)
}

fn decode_payload(mut cur: &[u8]) -> Option<(Vec<Segment>, Option<String>)> {
    let count = u32::from_le_bytes(take(&mut cur, 4)?.try_into().ok()?) as usize;
    let mut segments = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let cols = u16::from_le_bytes(take(&mut cur, 2)?.try_into().ok()?);
        let rows = u16::from_le_bytes(take(&mut cur, 2)?.try_into().ok()?);
        let cell_w = u16::from_le_bytes(take(&mut cur, 2)?.try_into().ok()?);
        let cell_h = u16::from_le_bytes(take(&mut cur, 2)?.try_into().ok()?);
        let len = u32::from_le_bytes(take(&mut cur, 4)?.try_into().ok()?) as usize;
        let bytes = take(&mut cur, len)?.to_vec();
        segments.push(Segment {
            size: WinSize {
                cols,
                rows,
                cell_w,
                cell_h,
            },
            bytes,
        });
    }
    // A file from before titles were stored ends here; a damaged tail costs
    // only the title, never the screen.
    let title = (|| {
        let len = u32::from_le_bytes(take(&mut cur, 4)?.try_into().ok()?) as usize;
        String::from_utf8(take(&mut cur, len)?.to_vec()).ok()
    })();
    Some((segments, title))
}

fn take<'a>(cur: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if cur.len() < n {
        return None;
    }
    let (head, rest) = cur.split_at(n);
    *cur = rest;
    Some(head)
}

fn dir() -> Option<PathBuf> {
    crate::config::config_path("scrollback")
}

fn path_for(pane_id: u64) -> Option<PathBuf> {
    Some(dir()?.join(format!("{pane_id}.bin")))
}

/// Write one pane's snapshot, replacing whatever was there.
///
/// Empty untitled snapshots are not written (and remove any stale file) so
/// cleared or blank panes do not accumulate useless files on disk.
///
/// Renamed into place so a reader never sees a half-written file, and mode 0600
/// from creation rather than after the fact — a window in which someone else's
/// terminal output is world-readable is not one worth leaving open.
pub fn save(pane_id: u64, segments: &[Segment], title: Option<&str>) {
    if segments.iter().all(|s| s.bytes.is_empty()) && title.is_none() {
        forget(pane_id);
        return;
    }
    let Some(path) = path_for(pane_id) else {
        return;
    };
    let Some(parent) = path.parent().map(|p| p.to_path_buf()) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&parent) {
        log::warn!("no scrollback directory ({e}); pane {pane_id} is not persisted");
        return;
    }
    let temp = parent.join(format!("{pane_id}.{}.tmp", std::process::id()));
    if let Err(e) = write_private(&temp, &encode(segments, title)) {
        log::warn!("could not stage pane {pane_id}'s scrollback: {e}");
        let _ = std::fs::remove_file(&temp);
        return;
    }
    if let Err(e) = std::fs::rename(&temp, &path) {
        log::warn!("could not store pane {pane_id}'s scrollback: {e}");
        let _ = std::fs::remove_file(&temp);
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    // Windows inherits the config directory's ACL, which is already per-user.
    std::fs::write(path, bytes)
}

pub fn load(pane_id: u64) -> Option<(Vec<Segment>, Option<String>)> {
    let raw = std::fs::read(path_for(pane_id)?).ok()?;
    match decode(&raw) {
        Some(snapshot) => Some(snapshot),
        None => {
            log::debug!("pane {pane_id}'s stored scrollback is not readable by this build");
            None
        }
    }
}

pub fn forget(pane_id: u64) {
    if let Some(path) = path_for(pane_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Drop snapshots for panes that nothing refers to any more.
pub fn sweep(keep: &HashSet<u64>) {
    sweep_budgeted(keep, keep);
}

/// Drop unreferenced snapshots, then enforce [`MAX_STORAGE_BYTES`] by evicting
/// oldest non-live snapshots first. Live panes are never evicted.
pub fn sweep_budgeted(keep: &HashSet<u64>, live: &HashSet<u64>) {
    let Some(dir) = dir() else {
        return;
    };
    sweep_in(&dir, keep, live, MAX_STORAGE_BYTES);
}

fn sweep_in(dir: &Path, keep: &HashSet<u64>, live: &HashSet<u64>, budget: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut total_bytes: u64 = 0;
    let mut evictable: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(id) = name
            .strip_suffix(".bin")
            .and_then(|stem| stem.parse::<u64>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if !keep.contains(&id) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let len = meta.len();
            total_bytes = total_bytes.saturating_add(len);
            if !live.contains(&id) {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                evictable.push((mtime, len, path));
            }
        }
    }
    if total_bytes <= budget || evictable.is_empty() {
        return;
    }
    evictable.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, len, path) in evictable {
        if total_bytes <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total_bytes = total_bytes.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: u16, rows: u16) -> WinSize {
        WinSize {
            cols,
            rows,
            cell_w: 8,
            cell_h: 17,
        }
    }

    fn seg(cols: u16, bytes: &[u8]) -> Segment {
        Segment {
            size: size(cols, 24),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn segments_survive_the_round_trip_with_their_geometry() {
        let segments = vec![seg(80, b"before the resize"), seg(120, b"after it")];
        let decoded = decode(&encode(&segments, None))
            .map(|(s, _)| s)
            .expect("what we wrote is readable");
        assert_eq!(decoded, segments);
    }

    #[test]
    fn v1_uncompressed_snapshots_still_decode() {
        let segments = vec![seg(80, b"legacy uncompressed bytes")];
        let raw = encode_v1(&segments, Some("legacy title"));
        let (decoded, title) = decode(&raw).expect("v1 snapshot remains readable");
        assert_eq!(decoded, segments);
        assert_eq!(title.as_deref(), Some("legacy title"));
    }

    #[test]
    fn v2_compression_shrinks_repetitive_terminal_output() {
        let big = "hello terminal scrollback line with ANSI \x1b[32mgreen\x1b[0m\r\n".repeat(400);
        let segments = vec![seg(100, big.as_bytes())];
        let v1 = encode_v1(&segments, None);
        let v2 = encode(&segments, None);
        assert!(
            v2.len() * 4 < v1.len(),
            "v2 ({}) should compress well below v1 ({})",
            v2.len(),
            v1.len()
        );
        assert_eq!(decode(&v2).unwrap().0, segments);
    }

    #[test]
    fn a_truncated_or_foreign_file_decodes_to_nothing() {
        let raw = encode(&[seg(80, b"hello")], None);
        assert!(decode(&raw[..raw.len() - 2]).is_none());
        assert!(decode(b"").is_none());
        assert!(decode(b"PLAIN TEXT, NOT OURS").is_none());
    }

    #[test]
    fn trimming_keeps_the_tail_and_the_geometry_of_what_it_keeps() {
        let mut segments = vec![seg(80, b"0123456789"), seg(120, b"abcdefghij")];
        trim_to(&mut segments, 12);
        let kept: Vec<u8> = segments.iter().flat_map(|s| s.bytes.clone()).collect();
        assert_eq!(kept, b"89abcdefghij");
        assert_eq!(segments[0].size, size(80, 24));
    }

    #[test]
    fn trimming_advances_past_split_utf8_and_ansi_sequences() {
        // UTF-8 CJK: "你好世界" (12 bytes). Slicing inside '好' (at byte 4) must
        // skip the remaining continuation bytes and land cleanly at '世'.
        let mut segments = vec![seg(80, "你好世界".as_bytes())];
        trim_to(&mut segments, 8); // over = 4, cuts inside '好' (bytes 3..6)
        assert_eq!(
            String::from_utf8(segments[0].bytes.clone()).unwrap(),
            "世界"
        );

        // CSI sequence split mid-parameter: "\x1b[38;2;1;2;3mOK"
        let mut segments = vec![seg(80, b"AAAA\x1b[38;2;1;2;3mOK")];
        trim_to(&mut segments, 12); // cuts inside the CSI sequence
        assert_eq!(segments[0].bytes, b"OK");
    }

    #[test]
    fn trimming_below_one_segment_still_leaves_something_to_replay() {
        let mut segments = vec![seg(80, b"0123456789")];
        trim_to(&mut segments, 0);
        assert_eq!(segments.len(), 1);
        assert!(segments[0].bytes.is_empty());
    }

    fn pin_config_dir() {
        let dir = std::env::temp_dir().join(format!("anterm-covtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        crate::config::set_config_dir(dir);
    }

    #[test]
    fn a_stored_screen_comes_back_and_can_be_dropped() {
        pin_config_dir();
        let pane = 90_001;
        save(pane, &[seg(80, b"what the pane had on it")], None);
        assert_eq!(
            load(pane).map(|(s, _)| s).expect("a saved screen is readable"),
            vec![seg(80, b"what the pane had on it")],
        );
        forget(pane);
        assert!(load(pane).is_none());
    }

    #[test]
    fn empty_untitled_save_skips_disk_and_clears_existing_snapshot() {
        pin_config_dir();
        let pane = 90_006;
        save(pane, &[seg(80, b"something")], None);
        assert!(load(pane).is_some());
        save(pane, &[seg(80, b"")], None);
        assert!(
            load(pane).is_none(),
            "saving an empty snapshot must drop any stale file rather than writing a blank one"
        );
    }

    #[test]
    fn saving_twice_replaces_rather_than_appends() {
        pin_config_dir();
        let pane = 90_002;
        save(pane, &[seg(80, b"first")], None);
        save(pane, &[seg(80, b"second")], None);
        assert_eq!(
            load(pane).map(|(s, _)| s).expect("still readable"),
            vec![seg(80, b"second")],
        );
        forget(pane);
    }

    #[test]
    fn the_sweep_keeps_only_panes_something_can_still_ask_for() {
        let dir = std::env::temp_dir().join(format!("anterm-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (kept, dropped) = (90_003, 90_004);
        for id in [kept, dropped] {
            std::fs::write(
                dir.join(format!("{id}.bin")),
                encode(&[seg(80, b"x")], None),
            )
            .unwrap();
        }
        let set = HashSet::from([kept]);
        sweep_in(&dir, &set, &set, MAX_STORAGE_BYTES);
        let left = |id: u64| dir.join(format!("{id}.bin")).exists();
        assert!(left(kept));
        assert!(!left(dropped));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_budget_evicts_oldest_dormant_snapshots_but_keeps_live_ones() {
        let dir = tempfile::tempdir().unwrap();
        let payload = vec![b'x'; 2048];
        let encoded = encode(&[seg(80, &payload)], None);
        let file_size = encoded.len() as u64;

        let p_old = dir.path().join("1.bin");
        let p_new = dir.path().join("2.bin");
        let p_live = dir.path().join("3.bin");
        std::fs::write(&p_old, &encoded).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p_new, &encoded).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p_live, &encoded).unwrap();

        let keep = HashSet::from([1, 2, 3]);
        let live = HashSet::from([3]);
        // Budget allows 2 files, so oldest dormant (1) is evicted, 2 and live 3 stay.
        sweep_in(dir.path(), &keep, &live, file_size * 2);
        assert!(!p_old.exists(), "oldest dormant snapshot should be evicted");
        assert!(p_new.exists(), "newer dormant snapshot fits budget");
        assert!(p_live.exists(), "live pane snapshot is never evicted");
    }

    #[cfg(unix)]
    #[test]
    fn a_stored_screen_is_not_readable_by_anyone_else() {
        use std::os::unix::fs::PermissionsExt as _;
        pin_config_dir();
        let pane = 90_005;
        save(pane, &[seg(80, b"a token someone echoed")], None);
        let mode = std::fs::metadata(path_for(pane).expect("a path under the config dir"))
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
        forget(pane);
    }

    #[test]
    fn the_title_rides_the_snapshot() {
        let segments = vec![seg(80, b"a screenful")];
        let (decoded, title) = decode(&encode(&segments, Some("✳ fixing the switcher")))
            .expect("what we wrote is readable");
        assert_eq!(decoded, segments);
        assert_eq!(title.as_deref(), Some("✳ fixing the switcher"));
    }

    #[test]
    fn a_snapshot_without_a_title_still_decodes() {
        let segments = vec![seg(80, b"an old file")];
        let (decoded, title) = decode(&encode(&segments, None)).expect("still a valid snapshot");
        assert_eq!(decoded, segments);
        assert_eq!(title, None);
    }

    #[test]
    fn an_empty_snapshot_round_trips() {
        assert_eq!(
            decode(&encode(&[], None))
                .map(|(s, _)| s)
                .expect("still a valid snapshot"),
            Vec::new(),
        );
    }
}
