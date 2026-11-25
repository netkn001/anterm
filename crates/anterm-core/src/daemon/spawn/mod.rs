mod probe;
mod process;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use probe::{
    HANDSHAKE_TIMEOUT, HandoffReturn, MismatchVerdict, ProbeVerdict, VersionProbe,
    control_dialect_answer, judge_handoff_return, judge_probe, query_daemon_version,
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use process::{ProcessIdentity, process_alive, process_identity};
use process::{
    detect_parent_shell, live_daemon_pid, reap_recorded_daemon, wait_for_recorded_exit,
};

use crate::config;
use crate::daemon::control::DialectRefusal;
use crate::daemon::protocol::{ClientMsg, DaemonMsg, DaemonVersion, PROTOCOL_VERSION};
use crate::daemon::{pidfile, transport};

#[cfg(windows)]
pub use process::DAEMON_CREATION_FLAGS;
#[cfg(windows)]
pub use process::stop_for_update;
pub use process::{detach_command, recorded_daemon_is_dead};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(50);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(6);
/// How long to wait for a refusal before assuming a handoff went through.
///
/// The daemon either execs — in which case this socket dies and the read fails
/// at once — or writes back why it did not. Neither takes long; the timeout is
/// only here so a daemon that hangs mid-handoff does not hang the window with
/// it.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(5);
/// How long after the endpoint disappears the daemon process itself gets to
/// finish exiting. Under a graceful shutdown this is milliseconds; the margin
/// covers the Windows descendant reap that runs before `exit`.
const PROCESS_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a seat holder that is not answering yet gets to be a daemon
/// mid-handoff or mid-startup rather than a stranded one.
const STRANDED_GRACE: Duration = Duration::from_secs(1);

/// Why the daemon that answered is not the one this build expects.
///
/// One process carries two version numbers — the pane protocol on `daemon.sock`
/// and the control dialect on the control socket — and a release can move one
/// without the other. They also fail differently, so a mismatch has to say
/// which it is: the first garbles pane traffic, the second leaves panes working
/// while every machine-tree call is refused, which costs the window its tabs.
pub enum DaemonMismatch {
    /// The pane protocol differs, or predates versioning entirely.
    Protocol(Option<DaemonVersion>),
    /// The pane protocol agrees but the control dialect does not.
    Dialect(DialectRefusal),
}

static MISMATCHED_DAEMON: std::sync::Mutex<Option<DaemonMismatch>> = std::sync::Mutex::new(None);

pub fn take_mismatched_daemon() -> Option<DaemonMismatch> {
    MISMATCHED_DAEMON.lock().ok()?.take()
}

/// Arms the launch prompt for the next window built.
///
/// Callers that meet the mismatch later than [`ensure_running`] — a control
/// link that comes back refused, say — record it here so the next window still
/// offers the restart rather than opening empty with no explanation.
pub fn note_daemon_mismatch(mismatch: DaemonMismatch) {
    settle_daemon_mismatch(Some(mismatch));
}

/// Record what a fresh probe found, overturning whatever the last one left.
///
/// The difference from [`note_daemon_mismatch`] is the `None`, and that is the
/// whole point of having it: a probe that finds the daemon ours is evidence,
/// and the record has to be able to *lose* a mismatch as much as gain one.
///
/// Until it could, the record only ever accumulated. The GUI arms the prompt
/// from [`ensure_running`], which is the first thing every control-link
/// reconnect attempt runs, and a mismatched daemon is one no connect succeeds
/// against — so the link backed off and retried, arming the prompt again each
/// time round, including in the seconds the user spent reading the dialog it
/// had already opened. Restarting the daemon then fixed the daemon and not the
/// record, and the next window built took that last arming and asked a second
/// time about a server that no longer existed.
fn settle_daemon_mismatch(found: Option<DaemonMismatch>) {
    if let Ok(mut slot) = MISMATCHED_DAEMON.lock() {
        *slot = found;
    }
}

/// Which daemon the records above are allowed to be about.
///
/// A probe is not one instant: it connects, asks two sockets, and only then
/// writes down what it found. In between, this build can stop the daemon, hand
/// it off, or spawn a new one — and the verdict landing afterwards describes a
/// process that no longer exists. Against a mismatched daemon that is not a
/// remote possibility but the normal case, because the control link retries on
/// a backoff and every one of those retries runs a probe: `land_handoff_return`
/// clears the record, and a probe that connected before the exec re-arms the
/// prompt about the daemon the user just replaced.
///
/// So the counter moves whenever this build deliberately changes which process
/// serves, a verdict carries the value it was taken under, and a landing whose
/// value is stale is dropped rather than written.
///
/// It does not order two probes against the *same* daemon — the last one wins,
/// which is what a record of "what is running now" should do.
static DAEMON_GENERATION: AtomicU64 = AtomicU64::new(0);

/// The stamp for a verdict about to be gathered. Read it before the probe's
/// first connect: reading it afterwards would defeat the point.
fn daemon_generation() -> u64 {
    DAEMON_GENERATION.load(Ordering::SeqCst)
}

/// This build is about to change which process is the daemon. Everything a
/// probe already in flight is going to report describes the outgoing one.
fn daemon_generation_moved() {
    DAEMON_GENERATION.fetch_add(1, Ordering::SeqCst);
}

static LOCAL_DAEMON: std::sync::Mutex<Option<DaemonVersion>> = std::sync::Mutex::new(None);

pub fn local_daemon_supports(feature: &str) -> bool {
    LOCAL_DAEMON
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|v| v.has_feature(feature)))
        .unwrap_or(false)
}

fn note_local_daemon(version: Option<DaemonVersion>) {
    if let Ok(mut slot) = LOCAL_DAEMON.lock() {
        *slot = version;
    }
}

/// The build string of a running daemon that speaks our protocol but was
/// compiled from different sources — the state an in-place upgrade leaves
/// behind, since the GUI is replaced while the daemon keeps serving every pane
/// from the old binary.
///
/// `PROTOCOL_VERSION` moves only when the wire format does, and it has not
/// moved since July, so this is the *common* case after an update, not a rare
/// one: the user sees a new version number while `pane.rs`,
/// `shell_integration.rs` and the whole ssh stack still run last release's
/// code.
///
/// Deliberately not a prompt and not an automatic restart. Restarting the
/// daemon ends every process it owns — the shells, the agents, the SSH
/// sessions — which is exactly what a user who just accepted an app update did
/// not ask for. Where the daemon can hand itself over in place with nothing
/// lost, the GUI does that on launch ([`adopt_this_build`]); otherwise
/// Settings surfaces it and lets them pick the moment.
///
/// A protocol mismatch is a different problem with its own prompt, so it is
/// excluded here rather than reported twice.
pub fn local_daemon_stale_build() -> Option<String> {
    let guard = LOCAL_DAEMON.lock().ok()?;
    let version = guard.as_ref()?;
    let ours = env!("CARGO_PKG_VERSION");
    (version.protocol == PROTOCOL_VERSION && !version.build.is_empty() && version.build != ours)
        .then(|| version.build.clone())
}

/// Whether the local daemon is known to be this build on both handshakes: the
/// pane protocol and build string it last reported match ours, and no dialect
/// or protocol mismatch is waiting to be put to the user.
///
/// "Known" is the point. A daemon nobody has probed yet, or one whose probe
/// never finished, is not proven current, and the answer for it is `false` —
/// the caller offers the restart rather than telling the user there is nothing
/// to do on the strength of a question never asked.
///
/// The build string is the crate version, so two nightlies of one version look
/// the same here. That is the same blind spot [`local_daemon_stale_build`] has,
/// and the one Settings already lives with.
pub fn local_daemon_is_this_build() -> bool {
    let pending = MISMATCHED_DAEMON
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or(true);
    if pending {
        return false;
    }
    LOCAL_DAEMON.lock().ok().is_some_and(|guard| {
        guard
            .as_ref()
            .is_some_and(|v| v.protocol == PROTOCOL_VERSION && v.build == env!("CARGO_PKG_VERSION"))
    })
}

/// Write a probe's verdict down: the daemon to remember for feature checks, and
/// what the restart prompt is left holding.
///
/// `generation` is the stamp taken before the probe started. A verdict whose
/// stamp is stale is about a daemon this build has since replaced, and writing
/// it would re-arm the prompt about a process that is already gone.
fn land_probe(generation: u64, verdict: ProbeVerdict) {
    if generation != daemon_generation() {
        log::debug!(
            "dropping a version verdict taken before the daemon was replaced underneath it"
        );
        return;
    }
    let ProbeVerdict { version, mismatch } = verdict;
    match &mismatch {
        MismatchVerdict::Found(DaemonMismatch::Protocol(Some(v))) => log::warn!(
            "daemon (build {}) speaks protocol {}, this build needs {}; \
             keeping it and deferring to the user",
            v.build,
            v.protocol,
            PROTOCOL_VERSION
        ),
        MismatchVerdict::Found(DaemonMismatch::Protocol(None)) => {
            log::warn!("daemon predates protocol versioning; keeping it and deferring to the user")
        }
        MismatchVerdict::Found(DaemonMismatch::Dialect(refusal)) => log::warn!(
            "daemon (build {}) speaks control v{}, this build speaks v{}; \
             its machine tree is out of reach until it restarts",
            refusal.peer_build,
            refusal.peer,
            refusal.ours
        ),
        MismatchVerdict::Clear | MismatchVerdict::Unchanged => {
            if let Some(v) = &version
                && !v.build.is_empty()
                && v.build != env!("CARGO_PKG_VERSION")
            {
                log::info!(
                    "daemon build {} differs from this build {}; keeping it so its panes \
                     survive the upgrade — the GUI hands it over on launch where it can, \
                     and Settings offers the restart",
                    v.build,
                    env!("CARGO_PKG_VERSION")
                );
            }
        }
    }
    note_local_daemon(version);
    // Settled, not merely noted: this probe is the current word on the daemon,
    // so a clean one has to clear what a mismatched one left behind. See
    // `settle_daemon_mismatch`.
    match mismatch {
        MismatchVerdict::Clear => settle_daemon_mismatch(None),
        MismatchVerdict::Found(found) => settle_daemon_mismatch(Some(found)),
        MismatchVerdict::Unchanged => {}
    }
}

pub fn ensure_running() -> anyhow::Result<()> {
    // Skip a probe of a dead recorded process. The replacement owns endpoint
    // cleanup only after it acquires the singleton lock.
    let mut stale = recorded_daemon_is_dead();
    if stale {
        log::warn!(
            "recorded daemon is dead; recorded_pid={:?}",
            pidfile::read()
        );
    }
    if !stale {
        // Stamped before the connect, so a daemon replaced while the two
        // handshakes are in flight takes this verdict down with it.
        let generation = daemon_generation();
        let connection = transport::connect();
        if let Ok(mut stream) = connection {
            match judge_probe(query_daemon_version(&mut stream), control_dialect_answer) {
                Some(verdict) => {
                    land_probe(generation, verdict);
                    return Ok(());
                }
                None => {
                    log::warn!(
                        "daemon version handshake failed; recorded_pid={:?} timeout={HANDSHAKE_TIMEOUT:?}",
                        pidfile::read()
                    );
                    note_local_daemon(None);
                    drop(stream);
                    // Check the singleton seat below before touching this process;
                    // silence alone must not cost every pane its session.
                    stale = true;
                }
            }
        } else {
            log::warn!(
                "daemon connection failed; recorded_pid={:?}: {}",
                pidfile::read(),
                connection.err().expect("failed connect")
            );
            // A refused connect is the other stale signal, and it is the only
            // one left when the pidfile cannot answer: it is missing (the
            // daemon died between `bind` and `write_current`, or never managed
            // to write it), or it records a pid the OS has since handed to an
            // unrelated process. `recorded_daemon_is_dead` says "not dead" to
            // both, so without this the endpoint file survives and the spawn
            // poll below pays the refusal delay on it — the very cost this
            // function was rewritten to avoid.
            stale = true;
        }
    }

    if stale {
        // A failed connection is not evidence that the process died. In
        // particular, an exec handoff or an overloaded daemon still owns its
        // sessions. Only an explicit restart may terminate a live seat holder.
        preserve_live_daemon(live_daemon_pid())?;
        log::info!(
            "recovering stale daemon endpoint; recorded_pid={:?}",
            pidfile::read()
        );
        // The new server clears stale endpoints only after claiming its
        // singleton lock. Do not call reap_stranded here: another server can
        // claim the seat between the check above and that destructive reap.
    }

    // While an installer is replacing the installation, spawning a daemon
    // would relock the very images it is clearing — the update would fail
    // with "files in use" caused by us. Connecting to a live daemon above is
    // fine; only creating a new one waits. The short patience first is for
    // the guard's holder being a Setup that is exiting right now — the
    // post-install "Launch anterm" click — where the launch deserves its
    // daemon, not an error.
    #[cfg(windows)]
    {
        const UPDATE_GUARD_PATIENCE: Duration = Duration::from_secs(5);
        let deadline = Instant::now() + UPDATE_GUARD_PATIENCE;
        while crate::daemon::update_guard::held() {
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "a anterm update is being installed right now; the daemon will return \
                     when the installer relaunches the app"
                );
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    spawn_detached()?;

    // After the spawn, because the spawn is itself a generation move.
    let generation = daemon_generation();
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(mut stream) = transport::connect() {
            // Judged by the same rules as a daemon found already running, and
            // for the same reason. `restart()` is `stop()` + this function, so
            // on that path the branch above never runs and this is the only
            // place a verdict can land. Without one the mismatch the user just
            // clicked Restart about outlives the daemon it described, and a
            // window opened before the control link's next probe asks all over
            // again about a server that is already gone — the gap
            // `land_handoff_return` closes on the other path.
            //
            // The dialect really can be asked this early: `run_daemon` spawns
            // the control listener before it binds the pane endpoint, so a
            // daemon answering this connect is already answering that one.
            match judge_probe(query_daemon_version(&mut stream), control_dialect_answer) {
                Some(verdict) => land_probe(generation, verdict),
                // A listening socket alone is not a successful startup.
                None => {
                    note_local_daemon(None);
                    log::warn!(
                        "spawned daemon is listening but its handshake failed; recorded_pid={:?}",
                        pidfile::read()
                    );
                    anyhow::bail!(
                        "daemon is listening but did not answer its startup handshake; left running"
                    );
                }
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            log::error!(
                "daemon startup timed out; recorded_pid={:?} timeout={STARTUP_TIMEOUT:?}",
                pidfile::read()
            );
            anyhow::bail!(
                "daemon did not start listening at {} within {:?}{}",
                transport::endpoint_display(),
                STARTUP_TIMEOUT,
                seat_holder_note()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Clears whatever is left of a server a connect probe already failed to
/// reach: reaps the recorded (or seat-holding, #667) process and removes the
/// endpoint files a fresh spawn would otherwise stand down against or pay a
/// refusal delay on.
///
/// A healthy server is the caller's to detect first — everything here acts on
/// the premise that nobody answered.
pub fn reap_stranded() {
    daemon_generation_moved();

    // A seat holder mid-handoff or mid-startup — claimed, not yet listening —
    // looks exactly like a stranded one from out here, and it may be carrying
    // every live session across an exec. Give it a moment to open its
    // endpoint before concluding it never will; a genuinely stranded holder
    // costs this wait once and then gets reaped.
    //
    // Health is an *answered handshake*, never a bare connect: a wedged
    // daemon's listener still completes connections out of the kernel's
    // backlog, and callers on the Unresponsive path have already proven that
    // connecting says nothing. A holder that connects but will not answer is
    // the reap's subject, not its exception — waiting out the rest of the
    // grace on it would only delay what its silence already decided.

    if crate::daemon::singleton::holder_pid().is_some() {
        let deadline = Instant::now() + STRANDED_GRACE;
        while Instant::now() < deadline {
            if let Ok(mut stream) = transport::connect() {
                match query_daemon_version(&mut stream) {
                    VersionProbe::Speaks(_) | VersionProbe::Legacy => return,
                    VersionProbe::Unresponsive => break,
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    reap_recorded_daemon(None);

    if transport::endpoint_exists() {
        transport::remove_stale_endpoint();
    }
    // The daemon's control listener probes control.port for a live
    // predecessor before binding; a stale file would cost it the same
    // refused-connect delay the reap above just made unnecessary.
    #[cfg(windows)]
    crate::host::server::remove_control_endpoint();
}

/// Names the process still holding the server seat, for the startup-timeout
/// errors: a spawned daemon that stood down against a survivor used to time
/// out with a message that pointed at nothing (#667). Empty when the seat is
/// free — the timeout is then genuinely about a slow or crashed start.
///
/// The kill advice is identity-gated: the recorded pid can outlive the
/// process it named (a pre-recording build holding the seat over an older
/// number), and an ungated message would be telling the user to kill
/// whatever process the OS reuses that pid for.
pub fn seat_holder_note() -> String {
    let Some(pid) = crate::daemon::singleton::holder_pid() else {
        return String::new();
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if !process_alive(pid as libc::pid_t) {
            return format!(
                "; the server seat is still held, but its recorded pid {pid} is no longer \
                 alive — find the holder of daemon.lock before killing anything"
            );
        }
        return match process_identity(pid as libc::pid_t) {
            ProcessIdentity::OurDaemon => format!(
                "; the server seat is still held by pid {pid}, which could not be reaped — \
                 `kill {pid}` and retry"
            ),
            ProcessIdentity::Foreign => format!(
                "; the server seat is still held, but its recorded pid {pid} now names an \
                 unrelated process — find the holder of daemon.lock before killing anything"
            ),
            // Alive, seat held, executable unreadable: most likely this *is*
            // the stranded server — saying otherwise would talk the user out
            // of the one action that frees the seat.
            ProcessIdentity::Unknown => format!(
                "; the server seat is still held by pid {pid}, whose executable can no \
                 longer be read — likely a stranded server; check it with `ps -p {pid}` \
                 before killing it"
            ),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    format!("; the server seat is still held (recorded pid {pid})")
}

fn preserve_live_daemon(holder: Option<u32>) -> anyhow::Result<()> {
    if let Some(pid) = holder {
        log::warn!(
            "automatic recovery refused to terminate live daemon pid={pid}; keeping sessions"
        );
        anyhow::bail!(
            "daemon pid {pid} is still running but not answering; its sessions were left intact. Retry, or explicitly restart the background server if it remains unresponsive"
        );
    }
    Ok(())
}

#[track_caller]
pub fn restart() -> anyhow::Result<()> {
    let caller = std::panic::Location::caller();
    log::warn!(
        "daemon restart requested; recorded_pid={:?} caller={caller}; sessions will end",
        pidfile::read()
    );
    stop();
    let result = ensure_running();
    match &result {
        Ok(()) => log::info!(
            "daemon restart completed; recorded_pid={:?}",
            pidfile::read()
        ),
        Err(e) => log::error!("daemon restart failed: {e:#}"),
    }
    result
}

/// Land what the handoff returned: the daemon to remember for feature checks,
/// and what the restart prompt is left holding.
///
/// A replacement clears the record, and that is the step this function exists
/// to make testable. The daemon the user was asked about is gone, so the reason
/// to ask is gone with it — but the control link re-arms the prompt on every
/// failed reconnect, and against a mismatched daemon every reconnect fails. By
/// the time the dialog is answered the record has usually been set again behind
/// it. Left standing, that arming outlives the restart it asked for, and the
/// next window built asks all over again about a server that is already gone.
///
/// The reconnect that follows would eventually settle the record clean by
/// itself, through `ensure_running`. Doing it here closes the gap in between,
/// which is exactly wide enough for a window to open in.
fn land_handoff_return(generation: u64, outcome: HandoffReturn) -> anyhow::Result<()> {
    if generation != daemon_generation() {
        anyhow::bail!("the daemon was replaced again while this handoff was landing");
    }
    match outcome {
        HandoffReturn::Replaced(v) => {
            note_local_daemon(Some(v));
            settle_daemon_mismatch(None);
            Ok(())
        }
        HandoffReturn::CarriedOn(v) => {
            note_local_daemon(Some(v));
            anyhow::bail!("the daemon is still running its old build")
        }
        HandoffReturn::Mute => {
            note_local_daemon(None);
            anyhow::bail!("the daemon that came back does not answer the version handshake")
        }
    }
}

/// Bring a daemon left behind by an in-place update onto this build, where
/// that costs nothing the user would notice — and ask first where it would.
///
/// An update replaces the app and relaunches the GUI; the daemon keeps running
/// from the old binary, by design, so the panes survive. On macOS that leaves
/// more than stale code behind: the daemon is its own responsible process (see
/// `responsibility`), and once the executable it started from has been
/// replaced on disk, the system stops attributing its shells to anterm. Every
/// Homebrew `node` or `python` in a pane gets `EHOSTUNREACH` on the local
/// network until the daemon starts again from the installed binary.
///
/// A handoff is that start without the cost: same pid, same ptys, same shells,
/// and the far side disclaims its responsibility afresh. So where the daemon
/// advertises one, it is asked for as a matter of course rather than left to
/// Settings — unless the daemon holds a pane the handoff would close, which is
/// a native-SSH pane (see `handoff`). Ending someone's session unasked at
/// launch is the one thing an update must not do, so that case is recorded
/// for the next window to put to the user ([`take_deferred_adoption`]), with
/// the same dialog Settings → Restart Server shows.
///
/// Everything else is left exactly as before — a daemon on this build already,
/// one that predates the handoff, Windows, or a handoff that was refused — and
/// that fallback is what Settings still offers the restart for. A protocol
/// mismatch is excluded like [`local_daemon_stale_build`] excludes it: that
/// one has its own prompt.
pub fn adopt_this_build() -> anyhow::Result<()> {
    // A debug build is someone's checkout, not an update: started against the
    // live config it would re-exec the daemon serving their real sessions
    // into `target/debug`, a binary the next build or worktree removal deletes.
    if cfg!(debug_assertions) {
        return Ok(());
    }
    let Some(build) = local_daemon_stale_build() else {
        return Ok(());
    };
    if !local_daemon_supports(crate::daemon::protocol::FEATURE_HANDOFF) {
        return Ok(());
    }
    if handoff_closes_panes() {
        log::info!(
            "daemon build {build} is not this build {}; it holds panes a handoff would close, \
             so the window asks first",
            env!("CARGO_PKG_VERSION")
        );
        if let Ok(mut slot) = DEFERRED_ADOPTION.lock() {
            *slot = Some(build);
        }
        return Ok(());
    }
    log::info!(
        "daemon build {build} is not this build {}; asking it to hand over in place",
        env!("CARGO_PKG_VERSION")
    );
    hand_off()
}

static DEFERRED_ADOPTION: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The daemon build [`adopt_this_build`] stepped back from, for the next window
/// to put to the user. Taken once: a dismissed dialog is an answer, and
/// Settings keeps offering the restart.
pub fn take_deferred_adoption() -> Option<String> {
    DEFERRED_ADOPTION.lock().ok()?.take()
}

/// Whether the running daemon holds a live pane a handoff cannot carry.
///
/// The daemon decides that at the exec (`hand_over`), by whether the pane has a
/// pty here; the client can see the same thing through `QueryProcs`, whose
/// context says so (`local_pty`). "Yes" when the daemon cannot be asked or
/// cannot say — a build from before the context existed — because a wrong
/// "no" costs a session, and a wrong "yes" costs a dialog.
fn handoff_closes_panes() -> bool {
    let panes = (|| -> Option<Vec<crate::daemon::protocol::PaneInfo>> {
        let mut stream = transport::connect().ok()?;
        ClientMsg::List.encode(&mut stream).ok()?;
        match DaemonMsg::read(&mut stream).ok()? {
            DaemonMsg::PaneList(panes) => Some(panes),
            _ => None,
        }
    })();
    let Some(panes) = panes else {
        return true;
    };
    panes.iter().filter(|pane| pane.alive).any(|pane| {
        let local_pty = (|| -> Option<bool> {
            let mut stream = transport::connect().ok()?;
            ClientMsg::QueryProcs {
                pane_id: pane.pane_id,
            }
            .encode(&mut stream)
            .ok()?;
            match DaemonMsg::read(&mut stream).ok()? {
                DaemonMsg::Procs(procs) => procs.context.map(|c| c.local_pty),
                _ => None,
            }
        })();
        local_pty != Some(true)
    })
}

/// Ask the running daemon to become this build without stopping.
///
/// It keeps its pid, its ptys and everything running on them; what it loses is
/// this connection, which its new image has never heard of. So the reply to a
/// handoff that worked is the socket closing, and an actual message back means
/// it did not happen.
///
/// Callers that only want the daemon to be on the current build should treat a
/// failure here as "fall back to [`restart`]": every reason this can fail — an
/// older daemon that does not know the message, a platform with no `execve`, an
/// exec that was refused — leaves the daemon exactly as it was, still serving.
pub fn hand_off() -> anyhow::Result<()> {
    use std::io::Write as _;

    if !local_daemon_supports(crate::daemon::protocol::FEATURE_HANDOFF) {
        anyhow::bail!("the running daemon cannot replace itself in place");
    }
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("could not locate own executable: {e}"))?;

    let mut stream = transport::connect()?;
    // Past this point the daemon may be replaced at any moment, so every probe
    // already in flight is describing the image being retired. Moving the
    // generation here rather than after the exec is what makes those verdicts
    // land before this one does harmless.
    daemon_generation_moved();
    let generation = daemon_generation();
    ClientMsg::Handoff { exe: exe.clone() }.encode(&mut stream)?;
    stream.flush()?;

    let _ = stream.set_read_timeout(Some(HANDOFF_TIMEOUT));
    match crate::daemon::protocol::DaemonMsg::read(&mut stream) {
        // The far end is the new image, which never had this socket.
        Err(_) => {}
        Ok(crate::daemon::protocol::DaemonMsg::Error(why)) => {
            anyhow::bail!("the daemon refused to hand over: {why}")
        }
        Ok(other) => anyhow::bail!("unexpected daemon reply to Handoff: {other:?}"),
    }
    drop(stream);

    // The socket is rebound by the new image a moment after the exec. Until it
    // is, connecting fails the same way it would against no daemon at all —
    // which is exactly what `ensure_running` would misread as "start another
    // one", so the wait belongs here rather than there.
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(mut stream) = transport::connect() {
            return land_handoff_return(
                generation,
                judge_handoff_return(query_daemon_version(&mut stream)),
            );
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "the daemon did not start listening again at {} within {:?}{}",
                transport::endpoint_display(),
                STARTUP_TIMEOUT,
                seat_holder_note()
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

pub fn stop() {
    use std::io::Write as _;

    daemon_generation_moved();

    // Read the pid before asking the daemon to die: the endpoint disappearing
    // is not the same event as the process releasing its image — the gap
    // between them is exactly where an installer starts replacing files that
    // are still locked — and an old build's shutdown still deletes the pidfile
    // before the process is gone, so this is the last moment the pid is
    // guaranteed readable. When the pidfile is already gone (#667), the pid
    // recorded in the singleton lock file is the remaining name for a
    // survivor that is still holding the seat.
    let recorded = pidfile::read()
        .or_else(crate::daemon::singleton::holder_pid)
        .filter(|&pid| pid > 4 && pid != std::process::id());

    let mut asked = false;
    if let Ok(mut stream) = transport::connect() {
        if ClientMsg::Shutdown.encode(&mut stream).is_ok() {
            let _ = stream.flush();
            asked = true;
            let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
            while Instant::now() < deadline && transport::connect().is_ok() {
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }

    // Only a shutdown that was actually delivered earns this wait: it is the
    // time a daemon that stopped listening gets to finish releasing its
    // image. A daemon nobody could even connect to was never asked to die —
    // waiting on it is five seconds spent watching a survivor not move
    // (#667); the reap below has its own graceful SIGTERM window.
    if asked
        && let Some(pid) = recorded
        && !wait_for_recorded_exit(pid, PROCESS_EXIT_TIMEOUT)
    {
        log::warn!("daemon pid {pid} released its endpoint but has not exited yet");
    }

    // With the pid read at the top, not from the pidfile: a shutdown that got
    // as far as its cleanup deleted nothing we rely on here, but a build that
    // still removes the pidfile mid-shutdown — or one whose exit stalled after
    // the cleanup — would otherwise leave the reap with no pid to act on, and
    // the survivor holding the singleton lock against every later launch
    // (#653).
    reap_recorded_daemon(recorded);

    if transport::endpoint_exists() {
        transport::remove_stale_endpoint();
    }
}

fn spawn_detached() -> anyhow::Result<()> {
    daemon_generation_moved();
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("could not locate own executable: {e}"))?;

    let config_dir = config::config_dir_path();

    // Some Windows shell brokers enforce Redirection Trust on processes they
    // launch. An ordinary child inherits it and cannot traverse Scoop's
    // user-created `current` junctions. The policy cannot be relaxed in place,
    // so create the daemon through the clean interactive desktop shell only
    // when the enforcing bit is actually present.
    #[cfg(windows)]
    if windows::redirection_trust_enforced() {
        let mut args = vec![std::ffi::OsString::from("--daemon")];
        if let Some(dir) = &config_dir {
            args.push(std::ffi::OsString::from("--config-dir"));
            args.push(dir.as_os_str().to_owned());
        }
        match windows::spawn_detached_with_clean_parent(&exe, &args) {
            Ok(()) => return Ok(()),
            // The clean parent is unavailable whenever there is no interactive
            // Explorer to borrow — it is restarting, the shell was replaced, or
            // the session has no desktop at all. Losing it only costs junction
            // traversal inside the shells, so degrade to the ordinary path
            // instead of refusing to start anterm at all.
            Err(error) => log::warn!(
                "could not spawn the daemon through the Windows desktop shell while \
                 Redirection Trust is enforced ({error}); falling back to the ordinary \
                 path, where Scoop-style junctions may be unreachable"
            ),
        }
    }

    let mut cmd = Command::new(exe);
    cmd.arg("--daemon");

    if let Some(dir) = config_dir {
        cmd.arg("--config-dir").arg(dir);
    }

    if let Some(shell) = detect_parent_shell() {
        cmd.env(crate::daemon::DETECTED_SHELL_ENV, shell);
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    detach_command(&mut cmd);

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            log::info!("daemon child spawned; child_pid={pid}");
            // Reap our child and preserve signal/exit status while this launcher
            // is alive. The daemon remains independent when the GUI quits.
            if let Err(e) = std::thread::Builder::new()
                .name("anterm-daemon-exit".into())
                .spawn(move || match child.wait() {
                    Ok(status) if status.success() => {
                        log::info!("daemon child exited; child_pid={pid} status={status}")
                    }
                    Ok(status) => log::error!(
                        "daemon child exited abnormally; child_pid={pid} status={status}"
                    ),
                    Err(e) => log::warn!("daemon child wait failed; child_pid={pid}: {e}"),
                })
            {
                log::warn!("could not monitor daemon child pid={pid}: {e}");
            }
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("failed to spawn daemon process: {e}")),
    }
}
