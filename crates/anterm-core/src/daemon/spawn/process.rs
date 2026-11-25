#[cfg(any(unix, windows, test))]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
#[cfg(any(target_os = "macos", target_os = "linux", windows))]
use std::time::Instant;

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
use super::POLL_INTERVAL;
use crate::daemon::pidfile;

#[cfg(any(target_os = "macos", target_os = "linux"))]
const REAP_TERM_TIMEOUT: Duration = Duration::from_secs(6);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const REAP_KILL_TIMEOUT: Duration = Duration::from_secs(2);

const DAEMON_EXE_STEMS: [&str; 3] = ["anterm-app", "anterm-server", "anterm"];

pub(super) fn strip_exe_suffix(name: &str) -> &str {
    match name.len().checked_sub(4) {
        Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") => &name[..i],
        _ => name,
    }
}

fn exe_names_equal(a: &str, b: &str) -> bool {
    let a = strip_exe_suffix(a);
    let b = strip_exe_suffix(b);
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

pub(super) fn is_reapable_daemon_name(name: &str) -> bool {
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    own.as_deref().is_some_and(|own| exe_names_equal(own, name))
        || DAEMON_EXE_STEMS
            .iter()
            .any(|stem| exe_names_equal(stem, name))
}

/// Whether the recorded daemon process is gone — the cheap, certain signal
/// that `daemon.port` and `control.port` are stale. One process-liveness
/// query, no TCP: a dead recorded daemon means a connect to the endpoint
/// would only pay the OS's refusal delay (seconds on some machines) for
/// information the pidfile already has.
///
/// The GUI's GuiOpen handoff probe relies on the same signal: the probe
/// connects to the daemon's control listener, so a dead recorded daemon
/// guarantees no GUI is registered there — running the probe would only wait
/// out the refusal delay on the stale `control.port` that `ensure_running`
/// clears later. The probe skips itself instead.
pub fn recorded_daemon_is_dead() -> bool {
    recorded_daemon_is_dead_with(pidfile::read(), daemon_process_alive)
}

/// The rule alone, so it can be tested without a pidfile on disk — pinning a
/// config directory would mean an env var, and that is process-global state
/// every other test running beside this one would inherit.
///
/// Note what a missing pidfile answers: **not dead**, because nothing here
/// knows otherwise. Callers must not read that as "alive" — a refused connect
/// is still the authority, and `ensure_running` keeps its own stale branch for
/// exactly that.
pub(super) fn recorded_daemon_is_dead_with(
    recorded: Option<u32>,
    alive: impl Fn(u32) -> bool,
) -> bool {
    let Some(pid) = recorded else {
        return false;
    };
    pid > 4 && pid != std::process::id() && !alive(pid)
}

#[cfg(windows)]
fn daemon_process_alive(pid: u32) -> bool {
    !crate::daemon::winproc::wait_for_exit(pid, Duration::ZERO)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn daemon_process_alive(pid: u32) -> bool {
    process_alive(pid as libc::pid_t)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn daemon_process_alive(_pid: u32) -> bool {
    // No cheap liveness query on this platform: assume alive, which keeps
    // the TCP probe as the authority.
    true
}

pub(super) fn live_daemon_pid() -> Option<u32> {
    let holder = crate::daemon::singleton::holder_pid();
    #[cfg(windows)]
    let holder = holder.or_else(|| {
        // Windows holds the singleton file with share_mode(0), so its pid
        // cannot be read while held. Use the live recorded process instead.
        let pid = pidfile::read()?;
        if !daemon_process_alive(pid) {
            return None;
        }
        let ours = pid == std::process::id()
            || crate::daemon::winproc::snapshot()
                .iter()
                .any(|p| p.pid == pid && is_reapable_daemon_name(&p.name));
        ours.then_some(pid)
    });
    holder
}

/// Whether the recorded daemon process actually exited within `timeout`. The
/// endpoint file only says the daemon stopped listening; this is what says its
/// executable image is no longer mapped.
#[cfg(windows)]
pub(super) fn wait_for_recorded_exit(pid: u32, timeout: Duration) -> bool {
    crate::daemon::winproc::wait_for_exit(pid, timeout)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn wait_for_recorded_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while process_alive(pid as libc::pid_t) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    true
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(super) fn wait_for_recorded_exit(_pid: u32, _timeout: Duration) -> bool {
    true
}

/// `recorded` is a pid the caller captured before asking the daemon to die;
/// the pidfile is the first fallback, because a shutdown that stalled after
/// its cleanup may have already deleted it — and the pid in the singleton
/// lock file is the last one, for the #667 state where the pidfile is gone
/// entirely but a survivor still holds the seat.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn reap_recorded_daemon(recorded: Option<u32>) {
    let Some(pid) = recorded
        .or_else(pidfile::read)
        .or_else(crate::daemon::singleton::holder_pid)
    else {
        return;
    };
    if pid <= 1 || pid == std::process::id() {
        clear_daemon_records();
        return;
    }
    if !process_alive(pid as libc::pid_t) {
        clear_daemon_records();
        return;
    }
    match process_identity(pid as libc::pid_t) {
        ProcessIdentity::OurDaemon => {
            log::warn!("reaping unreachable daemon (pid {pid}); its sessions will be hung up");
            if !reap_process(pid as libc::pid_t) {
                // The pid is the only handle left on the survivor; keep the
                // file so the next attempt still has someone to reap.
                return;
            }
        }
        // The recorded pid now belongs to some unrelated program: the record
        // is stale, not the process.
        ProcessIdentity::Foreign => {}
        ProcessIdentity::Unknown => {
            // Alive but unnameable. Deleting the record here is what used to
            // strand the machine (#667): the pid is the only handle on
            // whatever this is, so it must outlive this attempt.
            log::warn!(
                "recorded daemon pid {pid} is alive but its executable cannot be identified; \
                 keeping its record and leaving it alone"
            );
            return;
        }
    }
    clear_daemon_records();
}

/// Clears both records of a daemon the reap has confirmed dealt with — the
/// pidfile, and the pid in the lock file (which is only touched if the seat
/// is actually free; see `clear_record_if_free`).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn clear_daemon_records() {
    pidfile::remove();
    crate::daemon::singleton::clear_record_if_free();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) enum ProcessIdentity {
    /// Named like a daemon of ours: safe to reap.
    OurDaemon,
    /// Named like something else: the recorded pid has been reused.
    Foreign,
    /// Alive, but neither its executable path nor its comm name is readable.
    Unknown,
}

/// What the process behind `pid` is, judged by name — by executable path
/// first, and by the kernel's comm name when the path is unreadable.
///
/// The path is unreadable in exactly the case the reap exists for: on macOS
/// `proc_pidpath` fails outright for a live process whose binary has been
/// deleted, which is every daemon still running through an update that
/// replaced the installation. The comm name is recorded at `exec` time and
/// survives the deletion on both platforms.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn process_identity(pid: libc::pid_t) -> ProcessIdentity {
    let named = process_path(pid)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| process_comm(pid));
    match named {
        Some(name) if is_reapable_daemon_name(&name) => ProcessIdentity::OurDaemon,
        Some(_) => ProcessIdentity::Foreign,
        None => ProcessIdentity::Unknown,
    }
}

#[cfg(target_os = "macos")]
pub(super) fn process_comm(pid: libc::pid_t) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    // 2 * MAXCOMLEN, the buffer libproc's own callers use; proc_name refuses
    // anything smaller.
    let mut buf = [0u8; 64];
    let len =
        unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..len as usize]).into_owned())
}

#[cfg(target_os = "linux")]
pub(super) fn process_comm(pid: libc::pid_t) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_string())
}

/// Whether the process is gone by the end.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reap_process(pid: libc::pid_t) -> bool {
    if signal_and_await_exit(pid, libc::SIGTERM, REAP_TERM_TIMEOUT) {
        return true;
    }
    if signal_and_await_exit(pid, libc::SIGKILL, REAP_KILL_TIMEOUT) {
        return true;
    }
    log::error!("daemon pid {pid} survived SIGKILL; leaving it behind");
    false
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn signal_and_await_exit(
    pid: libc::pid_t,
    sig: libc::c_int,
    timeout: Duration,
) -> bool {
    unsafe { libc::kill(pid, sig) };
    wait_for_recorded_exit(pid as u32, timeout)
}

/// Whether `pid` is a process that still exists — where a zombie does not
/// count. A zombie answers `kill(pid, 0)` like the living, but it holds no
/// lock, no endpoint and no image, and no signal can end it: counting it
/// alive made the reap SIGTERM-then-SIGKILL a corpse for the full eight
/// seconds of both timeouts before giving up on it. Older GUI builds did
/// not wait on their daemon children, so those can remain zombies.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn process_alive(pid: libc::pid_t) -> bool {
    let exists = unsafe { libc::kill(pid, 0) == 0 };
    exists && !is_zombie(pid)
}

#[cfg(target_os = "macos")]
pub(super) fn is_zombie(pid: libc::pid_t) -> bool {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if got == size {
        return info.pbi_status == libc::SZOMB;
    }
    // Measured, not assumed: for a zombie this call fails outright while
    // `kill(pid, 0)` still succeeds — unlike a live process with a deleted
    // executable, whose state (though not its path) stays readable. A pid we
    // may signal but cannot introspect is a corpse.
    true
}

#[cfg(target_os = "linux")]
pub(super) fn is_zombie(pid: libc::pid_t) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        // Readable state is the same boundary as on macOS: signalable but
        // not introspectable is a corpse, not a daemon.
        return true;
    };
    // The state field follows the comm's closing paren — the comm itself may
    // contain spaces and parens.
    stat.rsplit_once(')')
        .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
}

#[cfg(windows)]
pub(super) fn reap_recorded_daemon(recorded: Option<u32>) {
    use crate::daemon::winproc;

    let Some(pid) = recorded.or_else(pidfile::read) else {
        return;
    };
    if pid <= 4 || pid == std::process::id() {
        pidfile::remove();
        return;
    }
    let procs = winproc::snapshot();
    let matches = procs
        .iter()
        .find(|p| p.pid == pid)
        .is_some_and(|entry| is_reapable_daemon_name(&entry.name));
    if matches {
        log::warn!("reaping unreachable daemon (pid {pid}); its sessions will be hung up");
        // One deadline across the whole tree: this runs synchronously before
        // the first window exists, and a crash that left many hosts behind
        // must not multiply the wait by their count.
        let mut targets = winproc::descendants(&procs, pid);
        targets.push(pid);
        winproc::terminate_and_wait_all(&targets, Instant::now() + REAP_WAIT_TIMEOUT);
    }
    pidfile::remove();
}

#[cfg(windows)]
const REAP_WAIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Stops the daemon and then makes the installation directory actually
/// replaceable, which is more than `stop` alone can promise: a daemon that
/// died without cleaning up leaves its ConPTY hosts (OpenConsole.exe) running,
/// orphaned, each holding the installed image open — invisible to the pidfile
/// and fatal to any installer's `DeleteFile`.
///
/// Terminates every process whose executable lives under `install_dir`
/// (except the caller), then waits until the replaceable images there can be
/// opened for writing. An error names what is still locked, so the update log
/// finally says *why* an upgrade could not replace its files.
#[cfg(windows)]
pub fn stop_for_update(install_dir: &Path) -> Result<(), String> {
    use crate::daemon::winproc;

    super::stop();

    let deadline = Instant::now() + UPDATE_CLEAR_TIMEOUT;
    let holdouts = winproc::processes_running_from(install_dir);
    for &pid in &holdouts {
        log::warn!(
            "terminating pid {pid} still running from {}",
            install_dir.display()
        );
    }
    winproc::terminate_and_wait_all(&holdouts, deadline);

    wait_until_images_unlocked(install_dir, deadline)
}

#[cfg(windows)]
const UPDATE_CLEAR_TIMEOUT: Duration = Duration::from_secs(10);

/// Waits until every .exe and .dll directly in `dir` can be opened for
/// writing — the same access an installer needs to replace it. Only the top
/// level: that is where the locked images (anterm-app.exe, OpenConsole.exe,
/// conpty.dll) live, and a recursive sweep would stall on unrelated content.
#[cfg(windows)]
fn wait_until_images_unlocked(dir: &Path, deadline: Instant) -> Result<(), String> {
    // Never probe our own image: the legitimate callers run from a staged
    // copy outside `dir`, but if someone invokes the *installed* binary with
    // this flag, its own image can never open for writing and the wait would
    // only ever time out.
    let own = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    let images: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|error| format!("reading {}: {error}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("dll")
                })
        })
        .filter(|path| {
            // Excluding takes a positive identification: a candidate that
            // cannot be canonicalized (delete-pending, held by a scanner) is
            // a lock to wait out, not ours to skip.
            !own.as_deref()
                .is_some_and(|own| std::fs::canonicalize(path).is_ok_and(|path| path == own))
        })
        .collect();

    let mut locked: Vec<&PathBuf> = images.iter().collect();
    loop {
        locked.retain(|path| std::fs::OpenOptions::new().write(true).open(path).is_err());
        if locked.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let names: Vec<String> = locked
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect();
            return Err(format!(
                "these files in {} are still in use by another process: {}",
                dir.display(),
                names.join(", ")
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(super) fn reap_recorded_daemon(_recorded: Option<u32>) {}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn detect_parent_shell() -> Option<PathBuf> {
    process_path(unsafe { libc::getppid() }).filter(|path| is_supported_shell(path))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn detect_parent_shell() -> Option<PathBuf> {
    None
}

#[cfg(any(unix, test))]
pub(super) fn is_supported_shell(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "zsh"
            | "bash"
            | "fish"
            | "nu"
            | "nu.exe"
            | "pwsh"
            | "powershell"
            | "powershell.exe"
            | "pwsh.exe"
    )
}

#[cfg(target_os = "macos")]
pub(super) fn process_path(pid: libc::pid_t) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len =
        unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    Some(PathBuf::from(
        String::from_utf8_lossy(&buf[..len as usize]).into_owned(),
    ))
}

#[cfg(target_os = "linux")]
pub(super) fn process_path(pid: libc::pid_t) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(PathBuf::from(strip_deleted_marker(
        path.to_string_lossy().into_owned(),
    )))
}

/// A deleted executable's `/proc/<pid>/exe` reads as "/path/name (deleted)".
/// The marker is the kernel's, not part of the name — left in place it made a
/// replaced daemon read as foreign, which dropped its record without reaping
/// it (#667).
#[cfg(any(target_os = "linux", test))]
pub(super) fn strip_deleted_marker(path: String) -> String {
    match path.strip_suffix(" (deleted)") {
        Some(stripped) => stripped.to_string(),
        None => path,
    }
}

/// Configure a daemon `Command` to detach from the launcher's session/console.
#[cfg(unix)]
pub fn detach_command(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// How the daemon is created: no console of its own, and no window.
///
/// Deliberately without `CREATE_NEW_PROCESS_GROUP`. That flag disables Ctrl+C
/// for the whole new group, and Windows hands the resulting "ignore Ctrl+C"
/// process state down to every descendant — which for the daemon means every
/// ConPTY shell it spawns, and everything those shells run. It bought nothing
/// either: `DETACHED_PROCESS` already leaves the daemon with no console, so no
/// console control event could reach it in the first place. Keeping it cost
/// every pane its Ctrl+C (#451, #314).
///
/// Public because `anterm-cli` launches the headless `anterm-server` the same way,
/// and that server spawns panes too: the two detach paths agreeing is the whole
/// point of the constant.
#[cfg(windows)]
pub const DAEMON_CREATION_FLAGS: u32 = DETACHED_PROCESS | CREATE_NO_WINDOW;

#[cfg(windows)]
pub(super) const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
pub(super) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Named only so the tests can say which bit must stay out of the flags above.
#[cfg(all(windows, test))]
pub(super) const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Configure a daemon `Command` to detach from the launcher's session/console.
#[cfg(windows)]
pub fn detach_command(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;

    cmd.creation_flags(DAEMON_CREATION_FLAGS);
}

#[cfg(not(any(unix, windows)))]
pub fn detach_command(_cmd: &mut Command) {}
