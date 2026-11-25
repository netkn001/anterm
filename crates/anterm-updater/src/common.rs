//! Shared platform-independent updater utilities (CLI argument parsing,
//! checksum verification, logging, outcome reporting, relaunch health checks,
//! and Unix swap/wait primitives).

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Child;
#[cfg(unix)]
use std::process::Command;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use anterm_protocol::updater::contract::{FLAG_CONFIG_DIR, FLAG_RESULT_FILE};

#[cfg(unix)]
pub(crate) const PARENT_POLL: Duration = Duration::from_millis(100);
pub(crate) const LAUNCH_GRACE: Duration = Duration::from_secs(1);

pub(crate) fn next_path(
    args: &mut impl Iterator<Item = OsString>,
    usage: impl FnOnce() -> String,
) -> Result<PathBuf, String> {
    args.next().map(PathBuf::from).ok_or_else(usage)
}

pub(crate) fn next_string(
    args: &mut impl Iterator<Item = OsString>,
    usage: impl FnOnce() -> String,
) -> Result<String, String> {
    args.next()
        .and_then(|arg| arg.into_string().ok())
        .ok_or_else(usage)
}

pub(crate) fn reject_extra(
    mut args: impl Iterator<Item = OsString>,
    usage: impl FnOnce() -> String,
) -> Result<(), String> {
    if args.next().is_some() {
        Err(usage())
    } else {
        Ok(())
    }
}

/// Apply `--config-dir` to both this process's config state and environment so
/// child processes (relaunched app, `--stop-daemon`) inherit the same dir.
pub(crate) fn apply_config_dir(dir: Option<&Path>) {
    let Some(dir) = dir else { return };
    #[cfg(windows)]
    anterm_core::config::set_config_dir(dir.to_path_buf());
    // Safe: argument parsing runs before any worker thread is spawned.
    unsafe { std::env::set_var("ANTERM_CONFIG_DIR", dir) };
}

/// Named tail options shared by Unix `install` invocations.
#[cfg(unix)]
#[derive(Default)]
pub(crate) struct UnixTailOptions {
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) result_file: Option<PathBuf>,
}

#[cfg(unix)]
impl UnixTailOptions {
    pub(crate) fn parse(
        mut args: impl Iterator<Item = OsString>,
        usage: fn() -> String,
    ) -> Result<Self, String> {
        let mut options = Self::default();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some(FLAG_CONFIG_DIR) => options.config_dir = Some(next_path(&mut args, usage)?),
                Some(FLAG_RESULT_FILE) => options.result_file = Some(next_path(&mut args, usage)?),
                _ => return Err(usage()),
            }
        }
        Ok(options)
    }

    pub(crate) fn apply(&self) {
        apply_config_dir(self.config_dir.as_deref());
    }
}

/// Record the terminal outcome of an update attempt for the next GUI launch to
/// merge into its update state (#540). Best-effort, like every updater log line.
pub(crate) fn report_outcome(
    result_file: Option<&Path>,
    log: &Path,
    version: &str,
    result: &Result<(), String>,
) {
    let Some(path) = result_file else { return };
    let outcome = anterm_protocol::updater::outcome::UpdateOutcome {
        version: version.to_string(),
        ok: result.is_ok(),
        detail: result.as_ref().err().cloned(),
    };
    if let Err(error) = anterm_protocol::updater::outcome::write_outcome(path, &outcome) {
        log_line(
            log,
            &format!(
                "could not record the update outcome at {}: {error}",
                path.display()
            ),
        );
    }
}

pub(crate) fn verify_archive(
    archive: &Path,
    checksums: &Path,
    asset_name: &str,
) -> Result<(), String> {
    let bytes =
        fs::read(archive).map_err(|error| format!("reading {}: {error}", archive.display()))?;
    let manifest = fs::read_to_string(checksums)
        .map_err(|error| format!("reading {}: {error}", checksums.display()))?;
    anterm_protocol::updater::checksums::verify(&manifest, asset_name, &bytes)
        .map_err(|error| error.to_string())
}

pub(crate) fn healthy_after_grace(child: &mut Child) -> Result<(), String> {
    thread::sleep(LAUNCH_GRACE);
    match child
        .try_wait()
        .map_err(|error| format!("checking the relaunched app: {error}"))?
    {
        None => Ok(()),
        Some(status) => Err(format!(
            "the relaunched app exited immediately with {status}"
        )),
    }
}

#[cfg(unix)]
pub(crate) fn remove_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| format!("removing {}: {error}", path.display()))
}

#[cfg(unix)]
pub(crate) fn run_checked(command: &mut Command, context: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{context}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub(crate) fn log_line(path: &Path, message: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

/// Wait until the spawning parent process exits. Watching `getppid()` when we
/// are a direct child is immune to PID reuse; falls back to `kill(pid, 0)` for
/// hand-run invocations.
#[cfg(unix)]
pub(crate) fn wait_for_exit(pid: u32) {
    let pid = pid as libc::pid_t;
    if unsafe { libc::getppid() } == pid {
        while unsafe { libc::getppid() } == pid {
            thread::sleep(PARENT_POLL);
        }
        return;
    }
    while process_alive(pid) {
        thread::sleep(PARENT_POLL);
    }
}

#[cfg(unix)]
pub(crate) fn process_alive(pid: libc::pid_t) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Shared Unix atomic backup-swap-relaunch-rollback transaction used by both
/// macOS `.app` bundle replacement and Linux `.AppImage` replacement.
#[cfg(unix)]
pub(crate) fn swap_and_relaunch(
    current: &Path,
    replacement: &Path,
    stage: &Path,
    backup_name: &str,
    noun: &str,
    prepare: impl FnOnce(&Path, &Path) -> Result<(), String>,
    launch: impl Fn(&Path) -> Result<(), String>,
    report: impl Fn(&Result<(), String>),
) -> Result<(), String> {
    let backup = stage.join(backup_name);
    if backup.exists() {
        let result = Err(format!(
            "the update staging backup already exists: {}",
            backup.display()
        ));
        report(&result);
        return result;
    }
    if let Err(error) = prepare(current, replacement) {
        report(&Err(error.clone()));
        return Err(error);
    }
    if let Err(error) = fs::rename(current, &backup) {
        let result = Err(format!("moving the current {noun} aside: {error}"));
        report(&result);
        return result;
    }
    if let Err(error) = fs::rename(replacement, current) {
        let _ = fs::rename(&backup, current);
        let _ = fs::remove_dir_all(stage);
        let result = Err(format!("putting the staged {noun} in place: {error}"));
        report(&result);
        return result;
    }

    match launch(current) {
        Ok(()) => {
            let _ = remove_path(&backup);
            let _ = fs::remove_dir_all(stage);
            let result = Ok(());
            report(&result);
            result
        }
        Err(error) => {
            let _ = remove_path(current);
            let (result, relaunch) = match fs::rename(&backup, current) {
                Ok(()) => {
                    let _ = fs::remove_dir_all(stage);
                    (Err(error), true)
                }
                Err(restore) => (
                    Err(format!("{error}; restoring the previous {noun}: {restore}")),
                    false,
                ),
            };
            // The outcome lands before the old app does: the relaunched GUI
            // merges it at startup, and a write afterward races that merge (#540).
            report(&result);
            if relaunch {
                let _ = launch(current);
            }
            result
        }
    }
}
