//! Leaf utilities shared across `anterm-core`.
//!
//! This module has zero dependencies on any other module in `anterm-core`,
//! keeping it safely at the bottom of the dependency DAG so `core`, `daemon`,
//! `host`, and `client` can all depend on it without introducing cycles.

use std::io;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 64-bit FNV-1a hash used for deterministic host/socket keys.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Current Unix timestamp in whole seconds (0 if the system clock precedes the epoch).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Strip a leading UTF-8 BOM (`U+FEFF`), if present.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// Strip any leading UTF-8 BOM characters (e.g. from noisy Windows/WSL output).
pub fn trim_boms(text: &str) -> &str {
    text.trim_start_matches('\u{FEFF}')
}

/// Platform home directory: `$HOME` on Unix, `%USERPROFILE%` on Windows.
/// Ignores unset or empty environment variables.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    let var = "HOME";
    #[cfg(not(unix))]
    let var = "USERPROFILE";
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Home directory checking `$HOME` first, then `%USERPROFILE%`, ignoring empty values.
/// Used by cross-platform history/server code that honors `$HOME` even on Windows.
pub fn home_dir_any() -> Option<PathBuf> {
    let pick = |key: &str| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    pick("HOME").or_else(|| pick("USERPROFILE"))
}

/// Per-process unique instance identifier minted once per OS process image.
/// Shared by both the pane daemon protocol and the control plane so one daemon
/// process reports a single consistent instance UUID across all sockets.
pub fn process_instance() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Serialize a wire payload to JSON bytes, mapping serde errors to `InvalidData`.
pub fn to_json<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Deserialize a wire payload from JSON bytes, mapping serde errors to `InvalidData`.
pub fn from_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> io::Result<T> {
    serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Deserialize a field leniently: if the value fails to deserialize, log a
/// warning and fall back to `T::default()` instead of rejecting the whole file.
pub fn de_lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(&value).unwrap_or_else(|e| {
        log::warn!("ignoring invalid config value {value}: {e}; using default");
        T::default()
    }))
}

/// Atomically write `bytes` to `path` via a sibling temp file + rename,
/// following any symlink chain so symlinked dotfiles stay symlinked.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_mode(path, bytes, false)
}

/// Atomically write `bytes` to `path` with `0o600` permissions on Unix.
pub fn write_atomic_private(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_mode(path, bytes, true)
}

fn write_atomic_mode(path: &std::path::Path, bytes: &[u8], private: bool) -> io::Result<()> {
    use std::io::Write as _;
    let resolved = resolve_symlinks(path);
    let path = resolved.as_path();
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
        std::process::id()
    ));
    {
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt as _;
            open.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = open.open(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        let _ = f.sync_all();
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn resolve_symlinks(path: &std::path::Path) -> PathBuf {
    const MAX_HOPS: usize = 40;

    let mut resolved = path.to_path_buf();
    for _ in 0..MAX_HOPS {
        let Ok(target) = std::fs::read_link(&resolved) else {
            break;
        };
        resolved = match resolved.parent() {
            Some(parent) => parent.join(target),
            None => target,
        };
    }
    resolved
}

/// Serde adapter encoding `Vec<u8>` as standard base64 strings on JSON wire frames.
pub mod b64 {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_known_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn strip_bom_removes_single_leading_bom() {
        assert_eq!(strip_bom("\u{feff}hello"), "hello");
        assert_eq!(strip_bom("hello"), "hello");
        assert_eq!(trim_boms("\u{feff}\u{feff}hello"), "hello");
    }

    #[test]
    fn process_instance_is_stable_within_process() {
        let a = process_instance();
        let b = process_instance();
        assert!(!a.is_empty());
        assert_eq!(a, b);
    }
}
