#[cfg(unix)]
mod adopt;
mod fanout;
mod restore;
mod ring;
mod signals;
mod spawn_env;
#[cfg(test)]
mod tests;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{Child, MasterPty, PtySize, native_pty_system};

use crate::clipboard::{
    ClipboardSniffer, Event as ClipboardEvent, Segment as ClipboardSegment,
    Sniffed as ClipboardSniffed,
};
use crate::kitty_graphics::{GraphicsSniffer, Segment, Sniffed};
use crate::term_modes::TerminalModes;
use crate::daemon::protocol::{
    AuthResponse, DaemonMsg, LeaseRequest, MAX_FRAME, NativeSshSpec, PaneInfo, RemoteContext,
    RemoteKind, ShellSpec, WinSize,
};

#[cfg(unix)]
pub use adopt::Carried;
pub use fanout::OutputGate;
pub(crate) use fanout::gated_len;
#[cfg(test)]
use fanout::{OBSERVER_BEHIND, OBSERVER_BUDGET};
pub use restore::{INPUT_MODE_RESETS, Restore, SCROLL_RESTORED_AWAY, restore_preamble, retitle};
pub(crate) use ring::HeadCut;
pub use signals::{CodexCandidate, codex_report_target};
#[cfg(test)]
pub(crate) use signals::parse_osc7;
#[cfg(test)]
use signals::AGENT_STALE_AFTER;
pub(crate) use spawn_env::ANTERM_PANE_ENV;

#[cfg(unix)]
use adopt::{AdoptedChild, AdoptedMaster};
use fanout::{
    Observer, SizeLease, attach_subscriber_with_permissions, controller_resize, fan_out_output,
    lease_end, lease_release, lease_take, notify, observe_subscriber, observe_subscriber_since,
    set_clipboard_permission, tell_controller_lease,
};
use restore::{
    REDRAW_NUDGE_SETTLE, answer_queries, detached_queries, record_output, redraw_nudge,
    run_redraw_nudge,
};
use ring::ReplayRing;
use signals::{
    AgentClock, OscSniffer, ShellState, agent_state_snapshot, apply_agent, apply_agent_signals,
    apply_probed_cwd, apply_remote_context, apply_signals, arm_interrupt, expire_stale_agent,
    facts_changed, foreground_agent, foreground_command_running, foreground_cwd,
    foreground_remote_context, interrupts, latch_remote_prompt, observed_facts,
    owns_detached_report, pty_foreground_pgid, publish_facts, suppress_relayed_prompt_marks,
};
use spawn_env::build_spawn_config;

const REMOTE_CONTEXT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Burst-gated throttle for `tcgetpgrp` (`foreground_group()`): allows the
/// first [`FOREGROUND_GROUP_BURST_BUDGET`] reads per window to detect a newly
/// launched foreground program immediately (0 ms latency), then suppresses
/// redundant per-read syscalls for the rest of the window during continuous
/// output floods.
const FOREGROUND_GROUP_WINDOW: Duration = Duration::from_millis(50);
const FOREGROUND_GROUP_BURST_BUDGET: u32 = 4;

/// How long the death path will chase a child's exit status before giving up
/// and reporting `Exited { code: None }`. The pty hits EOF when the last slave
/// fd closes, which can land a few milliseconds ahead of the child becoming
/// reapable — that race is all this window exists to cover. Everything past it
/// is a pane that looks frozen to every attached client, so it stays short.
const EXIT_CODE_PROBE_WINDOW: Duration = Duration::from_millis(500);

const EXIT_CODE_PROBE_INTERVAL: Duration = Duration::from_millis(10);

struct PaneState {
    id: u64,
    ring: ReplayRing,
    subscriber: Option<Sender<DaemonMsg>>,
    subscriber_epoch: u64,
    allow_remote_clipboard_write: bool,
    /// A native ssh pane's answer belongs to the profile that dialled the
    /// host, and the client attaching to it never sees that spec: a window
    /// reopening onto a pane it outlived attaches by id and sends `false`
    /// because it has nothing better to send. Pinning the spec's answer keeps
    /// the permission in one place. `None` means "no spec of our own", which
    /// is every pane on a remote `anterm-server` — there the controlling client
    /// is the only one holding the profile's answer, so its word is taken.
    clipboard_write_from_spec: Option<bool>,
    observers: Vec<Observer>,
    observer_seq: u64,
    lease: Option<SizeLease>,
    /// The controller asked to hear about leases: it can say
    /// [`DaemonMsg::Lease`], which a client from before them cannot decode.
    /// Reset with every new controller.
    lease_watch: bool,
    cwd: Option<PathBuf>,
    /// The last title the pane reported over OSC 0/2, for the machine tree to
    /// record. See [`crate::machine::PaneRecord::osc_title`].
    osc_title: Option<String>,
    shell: ShellState,
    /// Whether a prompt mark has arrived since `remote` was last set.
    ///
    /// The near shell cannot be at a prompt while a connection owns its pty,
    /// so a mark that says "at a prompt" on a remote pane can only be the far
    /// shell's — and that is the proof that the far side runs anterm's shell
    /// integration. Until it lands, the newest mark on the pane is the near
    /// shell's own "I started `ssh`", which says nothing about the far side
    /// and will never be superseded. Cleared whenever `remote` changes, so a
    /// second hop is proved on its own terms.
    remote_prompt_seen: bool,
    /// A native SSH pane's connection phase, as last sent. Status frames go
    /// only to whoever is attached at the time, so a window reattaching to a
    /// live session learned nothing and drew it as an unknown remote — no
    /// "connected" dot, and no warning before closing it.
    ssh_phase: Option<crate::daemon::protocol::SshPhase>,
    /// The private modes the pane's output has switched on — the alternate
    /// screen and mouse reporting above all. Folded from the same bytes the
    /// ring gets, because the ring cannot be trusted to still hold them: a
    /// full-screen tool sets them once at startup and the ring drops its front
    /// (#774). See [`TerminalModes`].
    modes: TerminalModes,
    /// What this pane is running, for the machine tree to record. Distinct from
    /// `shell` above, which is the shell-integration state.
    shell_spec: Option<ShellSpec>,
    remote: Option<RemoteContext>,
    agent: Option<crate::cli_agent::CLIAgent>,
    agent_argv: Option<Vec<String>>,
    agent_session: Option<crate::cli_agent::AgentSessionState>,
    agent_clock: AgentClock,
    alive: bool,
    exit_code: Option<i32>,
    /// When a reattach last made the program on this pane repaint; see
    /// [`redraw_nudge`].
    last_redraw_nudge: Option<std::time::Instant>,
}

enum PaneBackend {
    Pty(PtyBackend),
    NativeSsh(NativeSshBackend),
}

struct ForegroundProbes {
    /// The pty's foreground process group: one ioctl, cheap enough to ask on
    /// every read, and a change in it is what says the probes below would now
    /// answer differently.
    group: Box<dyn Fn() -> Option<i32> + Send>,
    remote: Box<dyn Fn() -> Option<RemoteContext> + Send>,
    agent: Box<dyn Fn() -> Option<Option<(crate::cli_agent::CLIAgent, Vec<String>)>> + Send>,
    cwd: Box<dyn Fn() -> Option<PathBuf> + Send>,
}

struct PtyBackend {
    /// Owns the PTY master until the pane is closed.
    ///
    /// Windows takes this value when the shell process exits. Dropping the
    /// ConPTY master calls `ClosePseudoConsole`, which closes the output side
    /// only after its pending bytes can be consumed by the dedicated reader.
    /// Keeping the slot optional lets that monitor end the stream without
    /// racing the reader for ownership of the exit notification.
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    #[cfg_attr(windows, allow(dead_code))]
    shell_pid: Option<u32>,
    integration_dir: Option<PathBuf>,
}

/// The pty half of a pane, before it is wired up: opened and spawned into by
/// [`DaemonPane::spawn`], inherited across an `exec` by [`DaemonPane::adopt`].
struct PtyParts {
    master: Box<dyn MasterPty + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    shell_pid: Option<u32>,
    integration_dir: Option<PathBuf>,
    reader_handle: Box<dyn Read + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

struct NativeSshBackend {
    handle: Arc<crate::daemon::ssh::SshSessionHandle>,
    connection: crate::daemon::ssh::SharedConnection,
}

pub struct DaemonPane {
    pub id: u64,
    owner: Option<String>,
    backend: PaneBackend,
    /// The input side (keyboard input / pasted text): the PTY writer, or the
    /// native-SSH channel writer. Behind a `Mutex` because writes can arrive from
    /// different connection threads. `Arc`-shared so the reader thread can write
    /// kitty-graphics query replies (`\x1b_G…;OK\x1b\\`) back to the PTY inline
    /// with the sniff, without routing through `write_input`.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Set during teardown so the reader doesn't emit a spurious exit.
    shutting_down: Arc<AtomicBool>,
    /// Set when the pane is being hibernated or stopped for a graceful daemon
    /// restart: skip `daemon::history::retire` on Drop so `history::carry` can
    /// hand the pane's history file straight to its replacement.
    preserve_history: AtomicBool,
    gate: Arc<OutputGate>,
    state: Arc<Mutex<PaneState>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    broker: Option<Arc<crate::daemon::ssh::PromptBroker>>,
}

struct DeathReporter {
    reported: AtomicBool,
    on_dead: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    exit_code: Mutex<Option<Box<dyn FnMut() -> Option<i32> + Send>>>,
}

impl DeathReporter {
    fn new(on_dead: impl FnOnce() + Send + 'static) -> Self {
        Self {
            reported: AtomicBool::new(false),
            on_dead: Mutex::new(Some(Box::new(on_dead))),
            exit_code: Mutex::new(None),
        }
    }

    fn probe_exit_code(&self, probe: impl FnMut() -> Option<i32> + Send + 'static) {
        *self.exit_code.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(probe));
    }

    #[cfg(windows)]
    fn has_reported(&self) -> bool {
        self.reported.load(Ordering::SeqCst)
    }

    fn report(&self, state: &Mutex<PaneState>, shutting_down: &AtomicBool) {
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        let code = self
            .exit_code
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .and_then(|probe| probe());
        let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
        st.alive = false;
        st.exit_code = code;
        let pane = st.id;
        log::info!(
            "pane {pane} exited; exit_code={code:?} shutdown_requested={}",
            shutting_down.load(Ordering::SeqCst)
        );
        if shutting_down.load(Ordering::SeqCst) {
            drop(st);
            crate::machine::observe_pane(pane, |p| p.live = false);
            return;
        }
        let subscribed = st.subscriber.is_some();
        notify(&mut st, DaemonMsg::Exited { code });
        drop(st);
        crate::machine::observe_pane(pane, |p| p.live = false);
        if subscribed {
            return;
        }
        if let Some(on_dead) = self.on_dead.lock().unwrap_or_else(|e| e.into_inner()).take() {
            on_dead();
        }
    }
}

/// Releases the PTY master without holding its slot mutex during destruction.
///
/// `ClosePseudoConsole` may wait while the output pipe is drained, and the same
/// slot is what `resize` locks. Dropping the master only after releasing the
/// mutex keeps a concurrent resize from parking behind a close that is itself
/// waiting on the reader.
#[cfg(windows)]
fn close_pty_master(master: &Mutex<Option<Box<dyn MasterPty + Send>>>) {
    let owned = master.lock().ok().and_then(|mut slot| slot.take());
    drop(owned);
}

/// How long the Windows exit monitor lets the reader announce the death on its
/// own before doing it itself.
///
/// The reader is the better reporter: it publishes `Exited` only after it has
/// forwarded every byte that preceded EOF, which is what keeps a short-lived
/// command's final frame ahead of its exit. But EOF is not guaranteed. A
/// grandchild that inherited the ConPTY output pipe holds it open after the
/// shell is gone — `cmd /c start …` is enough — and `ClosePseudoConsole` then
/// never completes. Without this window such a pane would read as alive
/// forever to every attached client.
#[cfg(windows)]
const EXIT_DRAIN_WINDOW: Duration = Duration::from_secs(2);

#[cfg(windows)]
const EXIT_DRAIN_POLL: Duration = Duration::from_millis(10);

/// Releases the pseudoconsole, then reports the pane's death — preferring the
/// reader's EOF-ordered report and falling back to its own after `window`.
///
/// Must run on a background thread: both halves block.
#[cfg(windows)]
fn drain_then_report(
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    state: Arc<Mutex<PaneState>>,
    shutting_down: Arc<AtomicBool>,
    death: Arc<DeathReporter>,
    window: Duration,
) {
    // `ClosePseudoConsole` can block until the reader drains the pipe, so it
    // cannot run on the thread that owns the deadline below.
    std::thread::Builder::new()
        .name("anterm-daemon-pane-pty-close".to_string())
        .spawn(move || close_pty_master(&master))
        .expect("spawn daemon pane pty close thread");

    let deadline = std::time::Instant::now() + window;
    while !death.has_reported() && std::time::Instant::now() < deadline {
        std::thread::sleep(EXIT_DRAIN_POLL);
    }
    // `DeathReporter` is idempotent, so this is a no-op whenever the reader
    // already got there — which is the ordinary case.
    death.report(&state, &shutting_down);
}

/// One out-of-band frame the reader forwards to the subscriber, kept in stream
/// order so a kitty image lands at the cursor cell the sender drew it at: a chunk
/// carrying graphics splits into `Output` runs interleaved with `Image`/`Delete`
/// frames, and they must reach the client in that same order. `Output` becomes a
/// `DaemonMsg::Output`, `Image` an encoded image frame, `Delete` a compact
/// selector — see the reader loop's `sniff` handling.
enum GraphicsFrame {
    Output(Vec<u8>),
    Image(Vec<u8>),
    Delete(Vec<u8>),
    ClipboardWrite(Vec<u8>),
}

/// Queue an encoded image frame for the subscriber, dropping any frame larger
/// than [`MAX_FRAME`]. The writer's `write_frame` rejects an oversize payload
/// with an error the writer loop treats as *fatal* — it would tear the client
/// off an otherwise-healthy pane. A single image that won't fit is not worth
/// that: drop it here so the rest of the stream keeps flowing. The inline
/// `t=d` path is bounded by `MAX_TRANSMISSION_BASE64` but that still admits
/// ~72 MiB of raw pixels, and the local shm/file path has no cap of its own,
/// so both can land here over the limit.
fn push_image_frame(frames: &mut Vec<GraphicsFrame>, frame: Vec<u8>) {
    if frame.len() <= MAX_FRAME {
        frames.push(GraphicsFrame::Image(frame));
    }
}

fn process_graphics_output(
    sniffed: Sniffed<'_>,
    ordered: bool,
    pass: &mut Vec<u8>,
    frames: &mut Vec<GraphicsFrame>,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
) {
    match sniffed {
        Sniffed::Plain(bytes) => {
            pass.extend_from_slice(bytes);
            if ordered && !bytes.is_empty() {
                frames.push(GraphicsFrame::Output(bytes.to_vec()));
            }
        }
        Sniffed::Segments(segments) => {
            for segment in segments {
                match segment {
                    Segment::Output(bytes) => {
                        pass.extend_from_slice(&bytes);
                        frames.push(GraphicsFrame::Output(bytes));
                    }
                    Segment::Query(reply) => {
                        if let Ok(mut writer) = writer.lock() {
                            let _ = writer.write_all(&reply);
                            let _ = writer.flush();
                        }
                    }
                    Segment::Image(image) => {
                        push_image_frame(frames, image.encode_frame());
                    }
                    Segment::ImageFromMedium(transfer) => {
                        if let Some(image) = transfer.resolve() {
                            push_image_frame(frames, image.encode_frame());
                        }
                    }
                    Segment::Delete(delete) => {
                        frames.push(GraphicsFrame::Delete(delete.encode()));
                    }
                }
            }
        }
    }
}

/// Strip OSC 5522 clipboard writes and Kitty graphics out of one PTY read
/// before anything else sees it. On the common path the original slice is
/// borrowed unchanged; protocol payloads are decoded into out-of-band frames
/// and never enter the replay ring.
///
/// `frames` receives the ordered list of out-of-band frames to forward *in
/// stream position*: a kitty image anchors to the cursor cell as it stood when
/// its command appeared, so the client must apply the text before an image,
/// then the image, then the text after it, in that order. On the no-graphics
/// fast path `frames` stays empty and the whole chunk is sent as one `Output`;
/// only a chunk with graphics splits into interleaved frames.
fn lift_out_of_band<'a>(
    raw: &'a [u8],
    clipboard: &mut ClipboardSniffer,
    graphics: &mut GraphicsSniffer,
    has_controller: bool,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    frames: &mut Vec<GraphicsFrame>,
) -> std::borrow::Cow<'a, [u8]> {
    match clipboard.sniff(raw) {
        ClipboardSniffed::Plain(b) => match graphics.sniff(b) {
            Sniffed::Plain(b) => std::borrow::Cow::Borrowed(b),
            sniffed @ Sniffed::Segments(_) => {
                let mut pass = Vec::new();
                process_graphics_output(sniffed, false, &mut pass, frames, writer);
                std::borrow::Cow::Owned(pass)
            }
        },
        ClipboardSniffed::Segments(segs) => {
            let mut pass = Vec::new();
            for seg in segs {
                match seg {
                    ClipboardSegment::Output(b) => {
                        let sniffed = graphics.sniff(&b);
                        process_graphics_output(sniffed, true, &mut pass, frames, writer);
                    }
                    ClipboardSegment::Event(ClipboardEvent::Reply(reply)) => {
                        if let Ok(mut w) = writer.lock() {
                            let _ = w.write_all(&reply);
                            let _ = w.flush();
                        }
                    }
                    ClipboardSegment::Event(ClipboardEvent::Write(write)) => {
                        if has_controller {
                            frames.push(GraphicsFrame::ClipboardWrite(write.encode_frame()));
                        } else if let Ok(mut w) = writer.lock() {
                            let reply =
                                crate::clipboard::response(write.id.as_deref(), "EBUSY");
                            let _ = w.write_all(&reply);
                            let _ = w.flush();
                        }
                    }
                }
            }
            std::borrow::Cow::Owned(pass)
        }
    }
}

impl DaemonPane {
    pub fn spawn(
        id: u64,
        cwd: Option<PathBuf>,
        size: WinSize,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<Restore>,
        allow_remote_clipboard_write: bool,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let pty_size = pty_size(size);

        // Before the shell exists, not after: a ConPTY child inherits the
        // "ignore Ctrl+C" state of whoever created it, and the daemon can be
        // carrying it from its own launch. Leaving it on costs the pane every
        // Ctrl+C it will ever be sent (#451, #314).
        #[cfg(windows)]
        crate::daemon::winproc::allow_ctrl_c_in_children();

        let pair = native_pty_system().openpty(pty_size)?;
        let spawn = build_spawn_config(id, cwd, shell)?;

        let child = pair.slave.spawn_command(spawn.cmd)?;
        let shell_pid = child.process_id();
        log::info!("pane {id} child started; child_pid={shell_pid:?}");
        let child = Arc::new(Mutex::new(child));

        drop(pair.slave);

        let reader_handle = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

        let (ring, restored_title) = match restore {
            Some(restore) => {
                let mut ring = ReplayRing::seeded(restore.segments, size);
                if let Some(title) = restore.title.as_deref() {
                    ring.append(&retitle(title));
                }
                ring.append(&restore_preamble(restore.banner.as_deref()));
                (ring, restore.title)
            }
            None => (ReplayRing::new(size), None),
        };

        Ok(Self::over_pty(
            PtyParts {
                master: pair.master,
                child,
                shell_pid,
                integration_dir: spawn.integration_dir,
                reader_handle,
                writer,
            },
            PaneState {
                id,
                ring,
                subscriber: None,
                subscriber_epoch: 0,
                allow_remote_clipboard_write,
                clipboard_write_from_spec: None,
                observers: Vec::new(),
                observer_seq: 0,
                lease: None,
                lease_watch: false,
                cwd: spawn.initial_cwd,
                osc_title: restored_title,
                shell: ShellState::default(),
                remote_prompt_seen: false,
                ssh_phase: None,
                modes: TerminalModes::default(),
                shell_spec: spawn.shell.clone(),
                remote: spawn.remote.clone(),
                agent: None,
                agent_session: None,
                agent_argv: None,
                agent_clock: AgentClock::default(),
                alive: true,
                exit_code: None,
                last_redraw_nudge: None,
            },
            owner,
            on_dead,
        ))
    }

    /// Wire a pty, a child and a starting state into a running pane.
    ///
    /// Shared by the two ways a pty-backed pane comes to exist — one that opens
    /// a pty and starts a shell in it, and one that inherits both from the
    /// image it replaced. Everything below this line is identical for them, and
    /// it is the part where getting it subtly wrong shows up as a pane that
    /// never reports its death or never sees its own output.
    fn over_pty(
        parts: PtyParts,
        state: PaneState,
        owner: Option<String>,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> Arc<Self> {
        let PtyParts {
            master,
            child,
            shell_pid,
            integration_dir,
            reader_handle,
            writer,
        } = parts;

        let id = state.id;
        let state = Arc::new(Mutex::new(state));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(OutputGate::new());
        let master = Arc::new(Mutex::new(Some(master)));

        let pane = Arc::new(Self {
            id,
            owner,
            backend: PaneBackend::Pty(PtyBackend {
                master: master.clone(),
                child: child.clone(),
                shell_pid,
                integration_dir,
            }),
            writer: writer.clone(),
            shutting_down: shutting_down.clone(),
            preserve_history: AtomicBool::new(false),
            gate: gate.clone(),
            state: state.clone(),
            reader: Mutex::new(None),
            broker: None,
        });

        let death = Arc::new(DeathReporter::new(on_dead));
        death.probe_exit_code({
            let child = child.clone();
            move || {
                let deadline = std::time::Instant::now() + EXIT_CODE_PROBE_WINDOW;
                loop {
                    // try_lock, never lock: DaemonPane::drop holds this very
                    // mutex across a blocking child.wait(), and this probe runs
                    // on the reader thread on its way to announcing the death.
                    // Blocking for the lock would park the Exited notification
                    // behind a wait() with no bound of its own; the deadline
                    // below is the only thing clients should ever wait on.
                    if let Ok(mut child) = child.try_lock() {
                        match child.try_wait() {
                            Ok(Some(status)) => return Some(status.exit_code() as i32),
                            Ok(None) => {}
                            Err(_) => return None,
                        }
                    }
                    if std::time::Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(EXIT_CODE_PROBE_INTERVAL);
                }
            }
        });

        #[cfg(windows)]
        Self::spawn_exit_monitor(
            shell_pid,
            master.clone(),
            state.clone(),
            pane.shutting_down.clone(),
            death.clone(),
        );

        let fg_master = master.clone();
        let group_master = master.clone();
        let remote_master = master.clone();
        let agent_master = master.clone();
        let cwd_master = master.clone();
        let reader = Self::spawn_reader(
            state,
            shutting_down,
            gate,
            reader_handle,
            writer.clone(),
            move || foreground_command_running(&fg_master, shell_pid),
            ForegroundProbes {
                group: Box::new(move || pty_foreground_pgid(&group_master)),
                remote: Box::new(move || foreground_remote_context(&remote_master)),
                agent: Box::new(move || foreground_agent(&agent_master)),
                cwd: Box::new(move || foreground_cwd(&cwd_master, shell_pid)),
            },
            death,
        );
        *pane.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(reader);

        pane
    }

    /// Reduce a live pane to what can be handed to another program image.
    ///
    /// Nothing is taken: the descriptor number is copied out, not the
    /// descriptor, and the pane keeps serving exactly as before. That is what
    /// lets the caller stage a whole handoff and then abandon it — an `exec`
    /// that fails costs a log line, not a machine's worth of shells.
    ///
    /// `None` for a native-SSH pane. Its session lives in this process's
    /// memory, not in a descriptor, and no amount of copying descriptor numbers
    /// would let the new image speak on the wire the old one had encrypted.
    #[cfg(unix)]
    pub fn carry(&self) -> Option<crate::daemon::pane::Carried> {
        let PaneBackend::Pty(pty) = &self.backend else {
            return None;
        };
        let master_fd = pty
            .master
            .lock()
            .ok()?
            .as_ref()
            .and_then(|master| master.as_raw_fd())?;
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Some(Carried {
            id: self.id,
            owner: self.owner.clone(),
            master_fd,
            child_pid: pty.shell_pid?,
            integration_dir: pty.integration_dir.clone(),
            size: st.ring.tail_size(),
            ring: st.ring.snapshot(),
            cwd: st.cwd.clone(),
            osc_title: st.osc_title.clone(),
            shell_spec: st.shell_spec.clone(),
            shell_active: st.shell.active,
            at_prompt: st.shell.at_prompt,
            last_exit: st.shell.last_exit_code,
            remote: st.remote.clone(),
            agent: st.agent,
            agent_argv: st.agent_argv.clone(),
            agent_session: st.agent_session.clone(),
            head_modes: st.ring.head_modes.restore_bytes(),
        })
    }

    /// Rebuild a pane around a pty this process already holds.
    ///
    /// The descriptor and the child pid came through an `exec`, so both are
    /// still ours in the only sense that matters: the kernel still has us down
    /// as the pty's owner and the shell's parent. What is gone is everything
    /// that was in memory, which is why the ring and the pane's status come
    /// back from the handoff blob rather than from the shell.
    #[cfg(unix)]
    pub fn adopt(
        carried: crate::daemon::pane::Carried,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let master = AdoptedMaster::from_fd(carried.master_fd)?;
        let reader_handle = master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(master.take_writer()?));
        let shell_pid = carried.child_pid;
        // The size the pty is actually at is the kernel's to answer, and it is
        // the truth: nobody resized it while we were not running. The carried
        // size only says where the ring's tail was cut.
        let size = master
            .get_size()
            .map(|s| WinSize {
                cols: s.cols,
                rows: s.rows,
                cell_w: carried.size.cell_w,
                cell_h: carried.size.cell_h,
            })
            .unwrap_or(carried.size);

        let (mut ring, modes) =
            ReplayRing::adopted(carried.ring, size, carried.head_modes.as_deref());
        // Not a restore banner: nothing was lost, so there is nothing to
        // announce and nothing to reset. The shell below these bytes is the
        // same shell that wrote them.
        ring.resize(size);

        Ok(Self::over_pty(
            PtyParts {
                master: Box::new(master),
                child: Arc::new(Mutex::new(Box::new(AdoptedChild::new(shell_pid)))),
                shell_pid: Some(shell_pid),
                integration_dir: carried.integration_dir,
                reader_handle,
                writer,
            },
            PaneState {
                id: carried.id,
                ring,
                subscriber: None,
                subscriber_epoch: 0,
                allow_remote_clipboard_write: false,
                clipboard_write_from_spec: None,
                observers: Vec::new(),
                observer_seq: 0,
                lease: None,
                lease_watch: false,
                cwd: carried.cwd,
                osc_title: carried.osc_title,
                shell_spec: carried.shell_spec,
                shell: ShellState {
                    active: carried.shell_active,
                    at_prompt: carried.at_prompt,
                    last_exit_code: carried.last_exit,
                    command: None,
                    // Not carried: the handoff record is a wire format shared
                    // with older images, and a pane mid-`ssh` that comes back
                    // claiming a prompt it cannot vouch for would be worse
                    // than one that says it does not know. It re-latches on
                    // the far side's next prompt.
                    mark_at_prompt: false,
                },
                remote_prompt_seen: false,
                ssh_phase: None,
                modes,
                remote: carried.remote,
                agent: carried.agent,
                agent_session: carried.agent_session,
                agent_argv: carried.agent_argv,
                agent_clock: AgentClock::default(),
                alive: true,
                exit_code: None,
                last_redraw_nudge: None,
            },
            carried.owner,
            on_dead,
        ))
    }

    /// A pane bridged to a shell channel on `spec`'s host.
    ///
    /// `remote_start_dir` is a directory on that host for the shell to start
    /// in. The pane's own `cwd` is deliberately left unknown until the far
    /// shell reports one: the `cd` that takes it there can quietly fail, and a
    /// guess recorded now would be persisted as fact.
    pub fn spawn_native_ssh(
        id: u64,
        size: WinSize,
        spec: Box<NativeSshSpec>,
        remote_start_dir: Option<String>,
        on_dead: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let allow_remote_clipboard_write = spec.remote_clipboard_write;
        let bridge = crate::daemon::ssh::session::make_bridge();
        let reader_handle: Box<dyn Read + Send> = Box::new(bridge.reader);
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(bridge.writer)));
        // The connect task fills this once authenticated; the pane exposes it to
        // WS4/WS5 via `ssh_connection()`.
        let connection: crate::daemon::ssh::SharedConnection = Arc::new(Mutex::new(Weak::new()));

        let target = spec
            .display_name
            .clone()
            .unwrap_or_else(|| format!("{}@{}", spec.user, spec.host));
        let remote = RemoteContext {
            kind: RemoteKind::NativeSsh,
            argv: Vec::new(),
            target,
        };

        let state = Arc::new(Mutex::new(PaneState {
            id,
            ring: ReplayRing::new(size),
            subscriber: None,
            subscriber_epoch: 0,
            allow_remote_clipboard_write,
            clipboard_write_from_spec: Some(allow_remote_clipboard_write),
            observers: Vec::new(),
            observer_seq: 0,
            lease: None,
            lease_watch: false,
            // A native ssh pane is not running a shell of this machine's; what
            // it is, `ssh_spec` already says.
            shell_spec: None,
            cwd: None,
            osc_title: None,
            shell: ShellState::default(),
            remote_prompt_seen: false,
            ssh_phase: None,
            modes: TerminalModes::default(),
            remote: Some(remote),
            agent: None,
            agent_session: None,
            agent_argv: None,
            agent_clock: AgentClock::default(),
            alive: true,
            exit_code: None,
            last_redraw_nudge: None,
        }));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(OutputGate::new());

        let broker = {
            let state = state.clone();
            crate::daemon::ssh::PromptBroker::new(Box::new(move |msg: DaemonMsg| {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if let DaemonMsg::SshStatus { phase } = &msg {
                    st.ssh_phase = Some(phase.clone());
                }
                match &st.subscriber {
                    Some(sub) => sub.send(msg).is_ok(),
                    None => false,
                }
            }))
        };

        let pane = Arc::new(Self {
            id,
            owner: None,
            backend: PaneBackend::NativeSsh(NativeSshBackend {
                handle: bridge.handle,
                connection: connection.clone(),
            }),
            writer: writer.clone(),
            shutting_down: shutting_down.clone(),
            preserve_history: AtomicBool::new(false),
            gate: gate.clone(),
            state: state.clone(),
            reader: Mutex::new(None),
            broker: Some(broker.clone()),
        });

        let death = Arc::new(DeathReporter::new(on_dead));

        let reader = Self::spawn_reader(
            state,
            shutting_down,
            gate,
            reader_handle,
            writer.clone(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| None),
                cwd: Box::new(|| None),
            },
            death,
        );
        *pane.reader.lock().unwrap_or_else(|e| e.into_inner()) = Some(reader);

        crate::daemon::ssh::SshManager::global().spawn_native_session(
            id,
            spec,
            remote_start_dir,
            size,
            broker,
            bridge.data_tx,
            bridge.cmd_rx,
            connection,
        );

        Ok(pane)
    }

    pub fn deliver_auth_response(&self, request_id: u64, response: AuthResponse) {
        if let Some(broker) = &self.broker {
            broker.deliver(request_id, response);
        }
    }

    #[allow(dead_code)]
    pub fn ssh_connection(&self) -> Option<Arc<crate::daemon::ssh::SshConnection>> {
        match &self.backend {
            PaneBackend::NativeSsh(b) => b.connection.lock().unwrap_or_else(|e| e.into_inner()).upgrade(),
            PaneBackend::Pty(_) => None,
        }
    }

    fn spawn_reader(
        state: Arc<Mutex<PaneState>>,
        shutting_down: Arc<AtomicBool>,
        gate: Arc<OutputGate>,
        mut reader: Box<dyn Read + Send>,
        // The pane's input side, shared so the reader can write kitty-graphics
        // query replies (`\x1b_G…;OK\x1b\\`) straight back to the PTY — see the
        // graphics sniff in the loop below.
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        foreground_running: impl Fn() -> bool + Send + 'static,
        probes: ForegroundProbes,
        death: Arc<DeathReporter>,
    ) -> JoinHandle<()> {
        let ForegroundProbes {
            group: foreground_group,
            remote: foreground_remote,
            agent: foreground_agent_fn,
            cwd: foreground_cwd_fn,
        } = probes;
        std::thread::Builder::new()
            .name("anterm-daemon-pane-reader".to_string())
            .spawn(move || {
                crate::threads::promote_to_user_interactive();
                let mut sniffer = OscSniffer::new();
                let mut clipboard = ClipboardSniffer::default();
                // Kitty graphics interception (issue #213): lifts image
                // sequences out of the stream *before* the ring/subscriber see
                // them, so the base64 pixels never enter replay and the client's
                // VT parser never chews through them. Zero-copy on the common
                // no-graphics chunk — see [`GraphicsSniffer::sniff`].
                //
                // File/shm transfer (`t=s`/`t=f`/`t=t`) is honored only while the
                // pane is *local*: the object/path names resolve on this host, and
                // reading them can't leak across an SSH tunnel. A pane that starts
                // local can `ssh` out mid-session, so the flag is refreshed from
                // the same remote-context poll below. Seed it from the pane's
                // current context so the first probe answers correctly.
                let starts_local = state.lock().unwrap_or_else(|e| e.into_inner()).remote.is_none();
                let mut graphics = GraphicsSniffer::new_local(starts_local);
                let mut buf = [0u8; 65536];

                let trace = std::env::var("ANTERM_TRACE").is_ok_and(|v| !v.is_empty() && v != "0");
                let mut tr_last = std::time::Instant::now();
                let mut tr_bytes: u64 = 0;
                let mut tr_reads: u32 = 0;
                let mut tr_read_t = std::time::Duration::ZERO;
                let mut tr_disp_t = std::time::Duration::ZERO;
                let mut next_remote_check = std::time::Instant::now();
                let mut group_window_end = next_remote_check;
                let mut group_checks_in_window: u32 = 0;
                let mut last_group = None;

                loop {
                    if trace && tr_last.elapsed() >= std::time::Duration::from_secs(1) {
                        // Not `eprintln!`: a daemon can be running without any
                        // standard error at all (see `daemon::server`), and a
                        // failed write there would panic the reader thread.
                        let _ = writeln!(
                            std::io::stderr(),
                            "[trace daemon] {:.1} MB/s | {} reads ({} B/read) | pty wait {:?} dispatch {:?}",
                            tr_bytes as f64 / tr_last.elapsed().as_secs_f64() / 1e6,
                            tr_reads,
                            if tr_reads > 0 { tr_bytes / tr_reads as u64 } else { 0 },
                            tr_read_t,
                            tr_disp_t,
                        );
                        tr_last = std::time::Instant::now();
                        tr_bytes = 0;
                        tr_reads = 0;
                        tr_read_t = std::time::Duration::ZERO;
                        tr_disp_t = std::time::Duration::ZERO;
                    }
                    gate.wait_below_high_water();
                    let tr0 = trace.then(std::time::Instant::now);
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Some(tr0) = tr0 {
                                tr_read_t += tr0.elapsed();
                                tr_reads += 1;
                                tr_bytes += n as u64;
                            }
                            let raw = &buf[..n];
                            let (controller, allowed) = state
                                .lock()
                                .map(|st| {
                                    (
                                        st.subscriber.as_ref().map(|_| st.subscriber_epoch),
                                        st.allow_remote_clipboard_write,
                                    )
                                })
                                .unwrap_or((None, false));
                            clipboard.set_controller(controller, allowed);
                            let mut frames: Vec<GraphicsFrame> = Vec::new();
                            let passthrough = lift_out_of_band(
                                raw,
                                &mut clipboard,
                                &mut graphics,
                                controller.is_some(),
                                &writer,
                                &mut frames,
                            );
                            let bytes: &[u8] = &passthrough;
                            // Sniff first (cheap, over the same bytes); collect any
                            // cwd/prompt change to emit while we hold the lock.
                            let mut signals = sniffer.feed(bytes);

                            // `any` first: `foreground_running` is a syscall,
                            // and most reads carry no prompt mark at all.
                            if signals.shell.iter().any(|s| s.at_prompt) && foreground_running() {
                                suppress_relayed_prompt_marks(&mut signals.shell);
                            }

                            // A prompt mark that survived the suppression above
                            // is the shell saying the command it ran is over, so
                            // the foreground has just gone back to being the
                            // shell itself. Probe right then rather than waiting
                            // out the interval: the probe is what clears the
                            // remote context an `ssh` left behind, and the
                            // interval alone can miss it forever. Polling only
                            // runs on output, and the prompt the shell just drew
                            // is the last output a pane produces until the user
                            // types again — so a pane whose `ssh` exited inside
                            // the interval kept reporting itself as remote, and
                            // everything keyed off that (the history scope ↑
                            // reads, most visibly) stayed on the far end until
                            // some unrelated output arrived (#817).
                            let back_at_prompt = signals.shell.iter().any(|s| s.at_prompt);
                            // A new foreground group is a new program in front
                            // — probe for it on its first output. During
                            // sustained output floods, burst-gate `tcgetpgrp`
                            // to at most `FOREGROUND_GROUP_BURST_BUDGET` checks
                            // per `FOREGROUND_GROUP_WINDOW` while keeping
                            // shell transitions and initial launches immediate.
                            let now = std::time::Instant::now();
                            if now >= group_window_end || !signals.shell.is_empty() {
                                group_window_end = now + FOREGROUND_GROUP_WINDOW;
                                group_checks_in_window = 0;
                            }
                            let group_changed = if back_at_prompt
                                || group_checks_in_window < FOREGROUND_GROUP_BURST_BUDGET
                            {
                                group_checks_in_window += 1;
                                let group = foreground_group();
                                let changed = group != last_group;
                                last_group = group;
                                changed
                            } else {
                                false
                            };
                            let poll_now =
                                back_at_prompt || group_changed || now >= next_remote_check;
                            if poll_now {
                                next_remote_check = now + REMOTE_CONTEXT_POLL_INTERVAL;
                            }
                            let remote = if poll_now {
                                let managed = {
                                    let st = state.lock().unwrap_or_else(|e| e.into_inner());
                                    st.remote
                                        .as_ref()
                                        .is_some_and(|remote| remote.kind != RemoteKind::Ssh)
                                };
                                (!managed).then(&foreground_remote)
                            } else {
                                None
                            };
                            let agent = poll_now.then(&foreground_agent_fn).flatten();
                            let probed_cwd = poll_now.then(&foreground_cwd_fn).flatten();

                            let tr1 = trace.then(std::time::Instant::now);
                            let may_change_facts = signals.cwd.is_some()
                                || signals.title.is_some()
                                || !signals.agent_events.is_empty()
                                || signals.notification.is_some()
                                || !signals.shell.is_empty()
                                || remote.is_some()
                                || agent.is_some()
                                || probed_cwd.is_some();
                            // Read off `signals` before `apply_signals` consumes
                            // it; spent after the hop below — see the function.
                            let saw_prompt_mark =
                                signals.shell.iter().any(|s| s.mark_at_prompt);
                            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                            let facts_before = may_change_facts.then(|| observed_facts(&st));
                            record_output(&mut st, bytes);
                            fan_out_output(&mut st, bytes, frames, &gate);
                            let owed = detached_queries(&mut st);
                            apply_signals(&mut st, signals);
                            if let Some(remote) = remote {
                                apply_remote_context(&mut st, remote);
                            }
                            latch_remote_prompt(&mut st, saw_prompt_mark);
                            // Keep kitty file/shm transfer gated on the pane's
                            // *current* locality: an `ssh` that just took the PTY
                            // must stop us honoring host-local object names. Cheap
                            // and only meaningful when a probe follows.
                            graphics.set_local(st.remote.is_none());
                            if let Some(agent) = agent {
                                apply_agent(&mut st, agent);
                            }
                            apply_probed_cwd(&mut st, probed_cwd);
                            if let Some(tr1) = tr1 {
                                tr_disp_t += tr1.elapsed();
                            }
                            let pane = st.id;
                            let alive = st.alive;
                            let facts_after = may_change_facts.then(|| observed_facts(&st));
                            drop(st);
                            answer_queries(&writer, &owed);
                            if !shutting_down.load(Ordering::SeqCst)
                                && let (Some(before), Some(after)) = (facts_before, facts_after)
                                && facts_changed(&before, &after)
                            {
                                publish_facts(pane, alive, after);
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }

                death.report(&state, &shutting_down);
            })
            .expect("spawn daemon pane reader thread")
    }

    pub fn attach(&self, subscriber: Sender<DaemonMsg>) -> u64 {
        self.attach_with_permissions(subscriber, false)
    }

    pub fn attach_with_permissions(
        &self,
        subscriber: Sender<DaemonMsg>,
        allow_remote_clipboard_write: bool,
    ) -> u64 {
        // Asked before the state lock, not under it: the probe takes the pty
        // master's lock, and every other caller that holds both takes them in
        // this order.
        let foreground_command = self.has_foreground_command();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let epoch = attach_subscriber_with_permissions(
            &mut st,
            subscriber,
            &self.gate,
            allow_remote_clipboard_write,
            foreground_command,
        );
        let nudge = redraw_nudge(
            &mut st,
            self.runs_foreground_program(foreground_command),
            std::time::Instant::now(),
        );
        drop(st);
        if let Some(transient) = nudge {
            self.nudge_redraw(transient);
        }
        epoch
    }

    /// Whether a program other than the shell is in the foreground, for
    /// [`redraw_nudge`]. A native ssh pane has no local pty to ask, and its
    /// alternate screen can only be the far program's.
    fn runs_foreground_program(&self, foreground_command: bool) -> bool {
        match &self.backend {
            PaneBackend::Pty(_) => foreground_command,
            PaneBackend::NativeSsh(_) => true,
        }
    }

    /// Puts the pty at `transient` now and back at the pane's size a moment
    /// later, which the program in the foreground hears as two `SIGWINCH`es
    /// and answers with a full repaint. See [`redraw_nudge`].
    ///
    /// Only the pty moves: the ring keeps its segment and no client is told a
    /// new size, so nothing a client or a lease holds changes. The size put
    /// back is read when it is put back, so a real resize that lands in
    /// between is what the pane ends up at.
    fn nudge_redraw(&self, transient: WinSize) {
        let resize: Box<dyn Fn(WinSize) + Send> = match &self.backend {
            PaneBackend::Pty(p) => {
                let master = Arc::clone(&p.master);
                Box::new(move |size| {
                    if let Ok(master) = master.lock()
                        && let Some(master) = master.as_ref()
                    {
                        let _ = master.resize(pty_size(size));
                    }
                })
            }
            PaneBackend::NativeSsh(b) => {
                let handle = Arc::clone(&b.handle);
                Box::new(move |size| handle.resize(size))
            }
        };
        let state = Arc::clone(&self.state);
        let _ = run_redraw_nudge(transient, REDRAW_NUDGE_SETTLE, resize, move || {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            st.alive.then(|| st.ring.size())
        });
    }

    /// Whether something other than the pane's own shell owns the terminal.
    ///
    /// Answered by the kernel (`tcgetpgrp`), not by the pane's stored shell
    /// state, which is why it can contradict `st.shell.at_prompt` — see
    /// [`replayed_at_prompt`]. A pane with no pty to ask (native ssh, and
    /// every pane on Windows, where the pty has no foreground process group)
    /// answers "no", which is what the live suppression already assumes.
    fn has_foreground_command(&self) -> bool {
        match &self.backend {
            PaneBackend::Pty(pty) => foreground_command_running(&pty.master, pty.shell_pid),
            PaneBackend::NativeSsh(_) => false,
        }
    }

    pub fn detach(&self, epoch: u64) -> bool {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.subscriber_epoch == epoch {
            st.subscriber = None;
            set_clipboard_permission(&mut st, false);
            self.gate.reset();
        }
        !st.alive && st.subscriber.is_none()
    }

    pub fn observe(&self, observer: Sender<DaemonMsg>, gate: Arc<OutputGate>) -> u64 {
        let foreground_command = self.has_foreground_command();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let id = observe_subscriber(&mut st, observer, gate, foreground_command);
        let nudge = redraw_nudge(
            &mut st,
            self.runs_foreground_program(foreground_command),
            std::time::Instant::now(),
        );
        drop(st);
        if let Some(transient) = nudge {
            self.nudge_redraw(transient);
        }
        id
    }

    pub fn observe_since(
        &self,
        observer: Sender<DaemonMsg>,
        gate: Arc<OutputGate>,
        since_offset: Option<u64>,
    ) -> u64 {
        let foreground_command = self.has_foreground_command();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let did_delta = since_offset.is_some_and(|off| st.ring.can_replay_since(off));
        let id = observe_subscriber_since(
            &mut st,
            observer,
            gate,
            foreground_command,
            since_offset,
        );
        // Only nudge on a full replay; a delta-resumed observer already has the
        // pre-disconnect alternate screen in its local emulator.
        let nudge = if did_delta {
            None
        } else {
            redraw_nudge(
                &mut st,
                self.runs_foreground_program(foreground_command),
                std::time::Instant::now(),
            )
        };
        drop(st);
        if let Some(transient) = nudge {
            self.nudge_redraw(transient);
        }
        id
    }

    /// A message for one observer alone, such as a refusal of what it sent.
    /// False once the pane has let it go.
    pub fn tell_observer(&self, observer_id: u64, msg: DaemonMsg) -> bool {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.observers
            .iter()
            .find(|obs| obs.id == observer_id)
            .is_some_and(|obs| obs.tx.send(msg).is_ok())
    }

    pub fn unobserve(&self, observer_id: u64) {
        let restore = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let restore = lease_release(&mut st, observer_id);
            st.observers.retain(|obs| obs.id != observer_id);
            restore
        };
        if let Some(size) = restore {
            self.resize_pty(size);
        }
    }

    /// An observer's [`LeaseRequest`]: take the pane at its size, or let go.
    pub fn observer_lease(&self, observer_id: u64, request: LeaseRequest) {
        let apply = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match request {
                LeaseRequest::Take { size, by } => lease_take(&mut st, observer_id, size, by),
                LeaseRequest::Release => lease_release(&mut st, observer_id),
                LeaseRequest::Watch | LeaseRequest::TakeBack => None,
            }
        };
        if let Some(size) = apply {
            self.resize_pty(size);
        }
    }

    /// The controller's [`LeaseRequest`]: hear about leases, or end one.
    pub fn controller_lease(&self, request: LeaseRequest) {
        let apply = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match request {
                LeaseRequest::Watch => {
                    st.lease_watch = true;
                    if st.lease.is_some() {
                        tell_controller_lease(&st);
                    }
                    None
                }
                LeaseRequest::TakeBack => lease_end(&mut st),
                LeaseRequest::Take { .. } | LeaseRequest::Release => None,
            }
        };
        if let Some(size) = apply {
            self.resize_pty(size);
        }
    }

    pub fn controls(&self, epoch: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).subscriber_epoch == epoch
    }

    pub fn agent_state(&self) -> Option<crate::daemon::control::PaneAgentState> {
        agent_state_snapshot(&self.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// See [`expire_stale_agent`].
    pub fn expire_stale_agent(&self) {
        expire_stale_agent(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()), std::time::Instant::now());
    }

    /// Apply an agent hook's report that came over the socket rather than
    /// through the pane's output (`ClientMsg::AgentEvent`), exactly as if the
    /// reader had found it there — once `pid`, the hook, is shown to run
    /// under this pane's shell. The pane id the hook named came from its
    /// environment, and a detached `tmux` server or the like carries that
    /// into processes this pane does not own.
    pub fn report_agent_event(&self, pid: u32, body: &str) -> Result<(), String> {
        let shell = self
            .pty()
            .and_then(|pty| pty.shell_pid)
            .ok_or_else(|| format!("pane {} runs no local process", self.id))?;
        let event = crate::cli_agent::parse_agent_event_body(body.as_bytes())
            .ok_or_else(|| "not an agent event".to_string())?;
        let running = self.state.lock().unwrap_or_else(|e| e.into_inner()).agent;
        if !crate::daemon::procinfo::descends_from(pid, shell)
            && !owns_detached_report(event.agent, running)
        {
            return Err(format!("process {pid} does not run in pane {}", self.id));
        }
        self.apply_reported_agent_event(event)
    }

    /// Apply a hook's report this pane was found to own some other way than
    /// by where the hook runs — see [`codex_report_target`].
    pub fn apply_reported_agent_event(
        &self,
        event: crate::cli_agent::AgentEvent,
    ) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !st.alive {
            return Err(format!("pane {} is not running", self.id));
        }
        let before = observed_facts(&st);
        apply_agent_signals(&mut st, vec![event], None);
        let after = observed_facts(&st);
        drop(st);
        if !self.shutting_down.load(Ordering::SeqCst) && facts_changed(&before, &after) {
            publish_facts(self.id, true, after);
        }
        Ok(())
    }

    /// This pane as a report from `agent`'s shared server sees it, for
    /// [`codex_report_target`].
    pub fn codex_candidate(&self, agent: crate::cli_agent::CLIAgent) -> CodexCandidate {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        CodexCandidate {
            pane: self.id,
            runs_codex: st.alive && st.agent == Some(agent),
            cwd: st.cwd.clone(),
            session: st.agent_session.as_ref().and_then(|s| s.session_id.clone()),
        }
    }

    pub fn gate(&self) -> Arc<OutputGate> {
        self.gate.clone()
    }

    pub fn write_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let agent = self.state.lock().unwrap_or_else(|e| e.into_inner()).agent;
        if interrupts(agent, bytes) {
            arm_interrupt(&self.state);
        }
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    pub fn procs(&self) -> crate::daemon::protocol::PaneProcs {
        let mut out = match self.pty().and_then(|pty| {
            pty.shell_pid
                .map(|pid| crate::daemon::procinfo::snapshot(pid, pty_foreground_pgid(&pty.master)))
        }) {
            Some(procs) => procs,
            None => Default::default(),
        };
        out.context = Some(self.context());
        out
    }

    /// What the pane knows about itself beyond its process list.
    ///
    /// Always filled, including on the branches above that have no tree to
    /// walk — a native-SSH pane's empty `procs` is exactly the answer this has
    /// to qualify, and returning `Default::default()` there would have said
    /// "nothing is running" in the same shape as "we could not look".
    fn context(&self) -> crate::daemon::protocol::PaneContext {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        crate::daemon::protocol::PaneContext {
            // The cached value only. `remote_context()` falls back to probing
            // the pty, and this is answered on a poll: paying a `/proc` walk
            // per tick for a fact the reader already refreshes on every prompt
            // would put the cost on the wrong side.
            remote: st.remote.clone(),
            local_pty: matches!(self.backend, PaneBackend::Pty(_)),
            at_prompt: st.shell.active.then_some(st.shell.mark_at_prompt),
            remote_prompt_seen: st.remote_prompt_seen,
            bracketed_paste: Some(st.modes.is_on(crate::term_modes::BRACKETED_PASTE)),
        }
    }

    fn pty(&self) -> Option<&PtyBackend> {
        match &self.backend {
            PaneBackend::Pty(p) => Some(p),
            PaneBackend::NativeSsh(_) => None,
        }
    }

    /// The controller's resize; see [`controller_resize`] for what a lease
    /// makes of it.
    pub fn resize(&self, size: WinSize) {
        let pty_ignores_pixels = cfg!(windows) && matches!(self.backend, PaneBackend::Pty(_));
        if let Some(size) = controller_resize(
            &mut self.state.lock().unwrap_or_else(|e| e.into_inner()),
            size,
            pty_ignores_pixels,
        ) {
            self.resize_pty(size);
        }
    }

    fn resize_pty(&self, size: WinSize) {
        match &self.backend {
            PaneBackend::Pty(p) => {
                if let Ok(master) = p.master.lock() {
                    if let Some(master) = master.as_ref() {
                        let _ = master.resize(pty_size(size));
                    }
                }
            }
            PaneBackend::NativeSsh(b) => b.handle.resize(size),
        }
    }

    pub fn alive(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).alive
    }

    pub fn info(&self) -> PaneInfo {
        let (cwd, osc_title, alive) = {
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (st.cwd.clone(), st.osc_title.clone(), st.alive)
        };
        PaneInfo {
            pane_id: self.id,
            cwd: cwd.or_else(|| self.foreground_cwd()),
            title: self.foreground_title(),
            osc_title,
            alive,
            owner: self.owner.clone(),
        }
    }

    pub(crate) fn remote_context(&self) -> Option<RemoteContext> {
        let cached = self.state.lock().unwrap_or_else(|e| e.into_inner()).remote.clone();
        cached.or_else(|| self.foreground_remote_context())
    }

    /// The pane's screen, capped for storage, with the mark that says how much
    /// output it had produced when the copy was taken.
    ///
    /// Both come from one acquisition of the state lock. Reading them apart
    /// would let output land in between, and the writer would then record a
    /// mark that claims to cover bytes its snapshot does not have — a pane
    /// that stopped producing at that moment would keep the stale copy for
    /// good, because its mark would never move again.
    /// The mark alone, for deciding whether a snapshot is worth taking:
    /// [`scrollback_snapshot`](Self::scrollback_snapshot) clones the whole
    /// ring under the state lock, which is a lot to pay every tick for the
    /// answer "nothing changed". A pane that moves between this read and the
    /// snapshot is fine — the snapshot returns its own mark, and that pair is
    /// what gets recorded.
    pub fn scrollback_mark(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).ring.appended
    }

    pub fn scrollback_snapshot(
        &self,
    ) -> (Vec<crate::daemon::scrollback::Segment>, Option<String>, u64) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let segments = st.ring.snapshot_tail(crate::daemon::scrollback::SNAPSHOT_CAP);
        (segments, st.osc_title.clone(), st.ring.appended)
    }

    pub fn clear_scrollback(&self) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let modes = st.modes.clone();
        st.ring.clear(&modes);
    }

    /// Keep this pane's per-pane shell history file on disk across Drop (used
    /// for hibernation and graceful daemon shutdown so `history::carry` can
    /// inherit it on restore).
    pub fn preserve_history(&self) {
        self.preserve_history.store(true, Ordering::SeqCst);
    }

    pub fn kill(&self) {
        self.hangup();
    }

    fn hangup(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        match &self.backend {
            PaneBackend::Pty(p) => {
                #[cfg(unix)]
                {
                    Self::signal_group(p, libc::SIGHUP);
                    // Give interactive shells a brief window to flush their
                    // history and exit cleanly on SIGHUP before escalating to
                    // SIGKILL. An idle zsh/bash typically exits in <15 ms.
                    let deadline = std::time::Instant::now() + Duration::from_millis(150);
                    let mut exited = false;
                    while std::time::Instant::now() < deadline {
                        if let Ok(mut child) = p.child.try_lock()
                            && matches!(child.try_wait(), Ok(Some(_)))
                        {
                            exited = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    if !exited {
                        if let Ok(mut child) = p.child.lock() {
                            let _ = child.kill();
                        }
                        Self::signal_group(p, libc::SIGKILL);
                    }
                }
                #[cfg(windows)]
                {
                    Self::kill_descendants(p);
                    if let Ok(mut child) = p.child.lock() {
                        let _ = child.kill();
                    }
                }
            }
            PaneBackend::NativeSsh(b) => b.handle.close(),
        }
    }

    #[cfg(windows)]
    fn kill_descendants(pty: &PtyBackend) {
        if let Some(pid) = pty.shell_pid {
            let procs = crate::daemon::winproc::snapshot();
            for target in crate::daemon::winproc::descendants(&procs, pid) {
                crate::daemon::winproc::terminate(target);
            }
        }
    }

    #[cfg(windows)]
    fn spawn_exit_monitor(
        shell_pid: Option<u32>,
        master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
        state: Arc<Mutex<PaneState>>,
        shutting_down: Arc<AtomicBool>,
        death: Arc<DeathReporter>,
    ) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };

        let Some(pid) = shell_pid else { return };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            // The process may have exited before OpenProcess ran, so go
            // straight to the close-and-report path — off this thread, since
            // pane construction is what calls us.
            std::thread::Builder::new()
                .name("anterm-daemon-pane-exit-fallback".to_string())
                .spawn(move || {
                    drain_then_report(master, state, shutting_down, death, EXIT_DRAIN_WINDOW);
                })
                .expect("spawn daemon pane exit fallback thread");
            return;
        }
        let handle = handle as isize;
        std::thread::Builder::new()
            .name("anterm-daemon-pane-exit-monitor".to_string())
            .spawn(move || {
                let handle = handle as windows_sys::Win32::Foundation::HANDLE;
                unsafe {
                    WaitForSingleObject(handle, INFINITE);
                    CloseHandle(handle);
                }
                drain_then_report(master, state, shutting_down, death, EXIT_DRAIN_WINDOW);
            })
            .expect("spawn daemon pane exit monitor thread");
    }

    #[cfg(unix)]
    fn signal_group(pty: &PtyBackend, sig: libc::c_int) {
        if let Some(pid) = pty.shell_pid {
            unsafe {
                libc::killpg(pid as libc::pid_t, sig);
            }
        }
        let fg = pty
            .master
            .lock()
            .ok()
            .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()));
        if let Some(fg) = fg {
            if Some(fg as u32) != pty.shell_pid {
                unsafe {
                    libc::killpg(fg, sig);
                }
            }
        }
    }

    fn foreground_cwd(&self) -> Option<PathBuf> {
        let pty = self.pty()?;
        foreground_cwd(&pty.master, pty.shell_pid)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn foreground_title(&self) -> String {
        let Some(pty) = self.pty() else {
            return String::new();
        };
        pty.master
            .lock()
            .ok()
            .and_then(|m| m.as_ref().and_then(|m| m.process_group_leader()))
            .and_then(super::procinfo::proc_name)
            .unwrap_or_default()
    }

    #[cfg(windows)]
    fn foreground_title(&self) -> String {
        let Some(pty) = self.pty() else {
            return String::new();
        };
        let Some(pid) = pty.shell_pid else {
            return String::new();
        };
        let procs = crate::daemon::winproc::snapshot();
        crate::daemon::winproc::foreground_name(&procs, pid).unwrap_or_default()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    fn foreground_title(&self) -> String {
        String::new()
    }

    fn foreground_remote_context(&self) -> Option<RemoteContext> {
        match &self.backend {
            PaneBackend::Pty(p) => foreground_remote_context(&p.master),
            PaneBackend::NativeSsh(_) => None,
        }
    }
}

impl Drop for DaemonPane {
    fn drop(&mut self) {
        if matches!(self.backend, PaneBackend::NativeSsh(_)) {
            crate::daemon::ssh::SshManager::global().teardown_pane_forwards(self.id);
        }
        self.hangup();
        if let PaneBackend::Pty(p) = &self.backend {
            if let Ok(mut child) = p.child.lock() {
                match child.wait() {
                    Ok(status) => log::info!(
                        "pane {} child exited; child_pid={:?} status={status}",
                        self.id,
                        p.shell_pid
                    ),
                    Err(e) => log::warn!("pane {} child wait failed: {e}", self.id),
                }
            }
        }
        if let Some(handle) = self.reader.lock().unwrap_or_else(|e| e.into_inner()).take() {
            join_bounded(handle, Duration::from_secs(2));
        }
        if let PaneBackend::Pty(p) = &mut self.backend {
            if let Some(dir) = p.integration_dir.take() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
        // The pane is over — by a kill or by the shell exiting — so whatever it
        // typed goes back to the history file it was seeded from. Hibernation
        // and graceful daemon shutdown set `preserve_history` so the file stays
        // intact for `history::carry` when the replacement pane restores. A
        // handoff does not come through here at all.
        if !self.preserve_history.load(Ordering::SeqCst) {
            crate::daemon::history::retire(self.id);
        }
    }
}

fn join_bounded(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    if std::thread::Builder::new()
        .name("anterm-daemon-pane-join".to_string())
        .spawn(move || {
            let _ = handle.join();
            let _ = tx.send(());
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(timeout).is_ok()
}

fn pty_size(size: WinSize) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.cols.max(1),
        pixel_width: size.cols.saturating_mul(size.cell_w),
        pixel_height: size.rows.saturating_mul(size.cell_h),
    }
}
