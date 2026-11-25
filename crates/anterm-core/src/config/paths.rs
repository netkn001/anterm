use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static CONFIG_DIR_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

pub fn set_config_dir(dir: PathBuf) {
    let _ = CONFIG_DIR_OVERRIDE.set(dir);
}

fn config_dir() -> Option<PathBuf> {
    resolve_config_dir(
        CONFIG_DIR_OVERRIDE.get().cloned(),
        env_config_dir(),
        portable_config_dir(),
        default_config_dir,
    )
}

/// Where the config directory comes from, first match wins: `--config-dir`,
/// then `$ANTERM_CONFIG_DIR`, then a portable install's own data folder, then
/// the platform default. Explicit beats implicit, so a portable copy can still
/// be pointed somewhere else for one launch or one session.
///
/// `default` is a closure because the answer rarely gets that far and the
/// default reads the environment again.
pub(super) fn resolve_config_dir(
    flag: Option<PathBuf>,
    env: Option<PathBuf>,
    portable: Option<PathBuf>,
    default: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    flag.or(env).or(portable).or_else(default)
}

fn env_config_dir() -> Option<PathBuf> {
    std::env::var_os("ANTERM_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// The marker the Windows portable ZIP ships beside its executables. The
/// updater reads it as "this is a portable install" (see `update.rs` and
/// `anterm-updater`); here it is half of the opt-in to keeping data beside the
/// executables too.
pub const PORTABLE_MARKER: &str = ".anterm-portable";

/// The folder, beside the executables, a portable install keeps its whole
/// config directory in — `config.json`, sessions, scrollback, history, the
/// daemon's socket and lock.
///
/// Never one of the updater's managed roots: those are moved aside and
/// replaced on every update, and this is the one thing in the install
/// directory that must survive one.
pub const PORTABLE_DATA_DIR: &str = "data";

/// `<exe dir>\data`, when the running executable is a portable install that
/// opted in to keeping its data there. Windows only; `None` everywhere else.
///
/// Resolved once per process: the executable does not move, and a folder
/// created while anterm runs must not move the config directory under a live
/// daemon.
fn portable_config_dir() -> Option<PathBuf> {
    // `cfg!` rather than `#[cfg]`, so every platform's build type-checks it.
    if !cfg!(windows) {
        return None;
    }
    static PORTABLE: OnceLock<Option<PathBuf>> = OnceLock::new();
    PORTABLE
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            portable_data_dir(exe.parent()?)
        })
        .clone()
}

/// The portable data folder for executables in `exe_dir`, if both the marker
/// and the folder are there.
///
/// The folder has to exist rather than being created on demand, because the
/// marker alone says nothing new: every portable ZIP has shipped it since the
/// in-app updater arrived, as the updater's cue. Keying on the marker by
/// itself would move every existing portable user onto an empty directory at
/// their next update — their settings and sessions still sitting in
/// `%APPDATA%\anterm`, looking lost. Creating `data` is the explicit opt-in, as
/// the `.portable` file is for Windows Terminal. The ZIP cannot ship the folder
/// either: the updater rejects package entries outside its managed roots, and
/// anything inside them is replaced on update.
pub fn portable_data_dir(exe_dir: &Path) -> Option<PathBuf> {
    let data = exe_dir.join(PORTABLE_DATA_DIR);
    (exe_dir.join(PORTABLE_MARKER).is_file() && data.is_dir()).then_some(data)
}

/// The config directory this machine resolves to when no single invocation
/// redirects it — `$ANTERM_CONFIG_DIR` where the box names one, the default under
/// `$HOME` otherwise.
///
/// [`config_dir`] with the `--config-dir` override left off, which is the
/// question "is this the machine's anterm or a second one somebody pointed
/// elsewhere" (see `machine::adopt_legacy_data_dir`). It cannot be answered by
/// whether the override is set: `daemon::spawn` passes `--config-dir` to every
/// daemon it starts, the ordinary install's included.
///
/// A portable install's data folder is deliberately not part of it: that copy
/// is a second anterm beside whatever the machine has installed, and must not
/// inherit the installed one's legacy tree.
pub fn machine_config_dir() -> Option<PathBuf> {
    env_config_dir().or_else(default_config_dir)
}

#[cfg(not(windows))]
pub fn default_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".config/anterm"))
}

#[cfg(windows)]
pub fn default_config_dir() -> Option<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(appdata).join("anterm"));
    }
    let profile = std::env::var_os("USERPROFILE").filter(|d| !d.is_empty())?;
    Some(PathBuf::from(profile).join(".config").join("anterm"))
}

pub fn config_path(file: &str) -> Option<PathBuf> {
    Some(config_dir()?.join(file))
}

/// Sets a corrupt state file aside (copied, the original left in place) so the
/// caller can fall back to defaults without silently destroying what was there.
pub(crate) fn quarantine(path: &std::path::Path) {
    // A broken file is read again and again — `Config::load` alone runs on
    // every pane spawn and every palette command — so this is reached over
    // and over for the same contents. A sibling already holding those bytes
    // *is* the copy this call would make; without the check, opening a
    // couple of tabs on a broken config.json fills the config directory with
    // eight identical `.corrupt` files and then overwrites the first one.
    if let Ok(bytes) = std::fs::read(path)
        && already_kept(path, &bytes)
    {
        return;
    }
    let aside = quarantine_path(path);
    match std::fs::copy(path, &aside) {
        Ok(_) => log::warn!("the previous contents were kept at {}", aside.display()),
        Err(e) => log::warn!("could not keep a copy at {}: {e}", aside.display()),
    }
}

/// Whether an earlier quarantine of `path` already holds exactly `bytes`.
fn already_kept(path: &std::path::Path, bytes: &[u8]) -> bool {
    quarantine_candidates(path).any(|kept| std::fs::read(&kept).is_ok_and(|held| held == bytes))
}

/// Like [`quarantine`], but moves the file out of the way — for files that
/// cannot even be read, where copying would fail too.
pub(crate) fn quarantine_by_rename(path: &std::path::Path) {
    let aside = quarantine_path(path);
    match std::fs::rename(path, &aside) {
        Ok(()) => log::warn!("the previous contents were moved to {}", aside.display()),
        Err(e) => log::warn!("could not move the file to {}: {e}", aside.display()),
    }
}

/// Every name a quarantined copy of `path` may go under, oldest first.
pub(super) fn quarantine_candidates(path: &std::path::Path) -> impl Iterator<Item = PathBuf> + use<'_> {
    const MAX_QUARANTINED: u32 = 8;

    std::iter::once(path.with_extension("json.corrupt"))
        .chain((1..MAX_QUARANTINED).map(|n| path.with_extension(format!("json.corrupt.{n}"))))
}

fn quarantine_path(path: &std::path::Path) -> PathBuf {
    let mut candidates = quarantine_candidates(path);
    let base = candidates.next().expect("the base name is always offered");
    if !base.exists() {
        return base;
    }
    candidates
        .find(|candidate| !candidate.exists())
        .unwrap_or(base)
}

pub use anterm_protocol::util::{write_atomic, write_atomic_private};

pub fn config_dir_path() -> Option<PathBuf> {
    config_dir()
}
