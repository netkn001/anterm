mod mirror;
mod reader;
mod rpc;
mod term_adapter;
#[cfg(test)]
mod tests;

pub use mirror::{EventReceiver, TermStateMirror};

use std::borrow::Cow;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use alacritty_terminal::event::{Event as AlacEvent, EventListener};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{self, CursorShape, CursorStyle};

use anterm_core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use anterm_ui_kit::config::CursorStyle as ConfigCursorStyle;
use anterm_core::osc::{Note, NoteBudget};
#[cfg(test)]
use anterm_core::daemon::protocol::NativeSshSpec;
use anterm_core::daemon::protocol::{
    AuthPromptKind, AuthResponse, ClientMsg, DaemonMsg, RemoteContext, RestoreFrom,
    ShellSpec, SshPhase, WinSize,
};
use anterm_core::daemon::transport::{self, Stream};
use crate::prompt::command_cursor::CommandMark;
use crate::prompt::parked_cursor::CursorCut;
use crate::prompt::prompt_reflow::PromptMark;

use crate::backend::size::TermSize;
use reader::OscNotes;

#[derive(Clone)]
pub struct EventProxy {
    tx: smol::channel::Sender<AlacEvent>,
    replaying: Arc<AtomicBool>,
    wakeup_queued: Arc<AtomicBool>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacEvent) {
        if self.replaying.load(Ordering::Relaxed)
            && matches!(
                event,
                AlacEvent::PtyWrite(_)
                    | AlacEvent::ColorRequest(..)
                    | AlacEvent::TextAreaSizeRequest(_)
                    | AlacEvent::ClipboardStore(..)
                    | AlacEvent::ClipboardLoad(..)
                    | AlacEvent::Bell
            )
        {
            return;
        }
        if matches!(event, AlacEvent::Wakeup)
            && self.wakeup_queued.swap(true, Ordering::AcqRel)
        {
            return;
        }
        if self.tx.try_send(event).is_err() {
            self.wakeup_queued.store(false, Ordering::Release);
        }
    }
}

#[derive(Default, Clone, Copy)]
struct ShellState {
    active: bool,
    at_prompt: bool,
    last_exit: Option<i32>,
    seq: u64,
    cycle: u64,
}

/// The pane's agent status as the client last heard it, plus how it heard it.
///
/// A reattach — the app restarting onto panes the daemon kept alive, or a
/// dropped link coming back — has the daemon replay the pane's *stored* status
/// as an ordinary `AgentStatus` frame ([`crate::daemon`]'s `replay_state`).
/// Nothing on the wire distinguishes it from a live transition, and a client
/// that reads it as one concludes that every restored agent finished its turn
/// in the instant the window opened. `replayed` is that distinction, kept in
/// the same lock as the value it describes so a reader can never observe the
/// status without also learning where it came from.
#[derive(Default)]
struct AgentSlot {
    state: Option<AgentSessionState>,
    /// `state` arrived as an attach replay and no one has adopted it yet.
    /// Cleared by the first taker — the view adopts it as a baseline rather
    /// than as an edge.
    replayed: bool,
}

struct ReaderSignals {
    cwd: Arc<Mutex<Option<PathBuf>>>,
    shell: Arc<Mutex<ShellState>>,
    remote: Arc<Mutex<Option<RemoteContext>>>,
    agent: Arc<Mutex<Option<CLIAgent>>>,
    agent_session: Arc<Mutex<AgentSlot>>,
    /// Whether this link still owes us the attach replay. The reader keeps it
    /// as a plain local: a link's replay is a property of that link's stream
    /// position, and nothing outside the reader thread ever needs to read it.
    awaiting_replay: bool,
    exited: Arc<AtomicBool>,
    child_exited: Arc<AtomicBool>,
    zle_reading: Arc<AtomicBool>,
    running_command: Arc<Mutex<String>>,
    submitted_commands: Arc<Mutex<Vec<(String, u64)>>>,
    auth: Arc<Mutex<VecDeque<(u64, AuthPromptKind)>>>,
    phase: Arc<Mutex<Option<SshPhase>>>,
    /// Kitty-graphics images the daemon lifted out of the stream (issue #213),
    /// anchored to the grid for the paint path to blit. Shared with the reader,
    /// which places/deletes them as `DaemonMsg::Image`/`DeleteImage` frames land.
    images: crate::render::images::ImageStore,
    clipboard_writes: Arc<Mutex<VecDeque<anterm_core::clipboard::ClipboardWrite>>>,
    osc_notes: OscNotes,
    clipboard_write_busy: Arc<AtomicBool>,
    lease: Arc<Mutex<Option<String>>>,
    /// Whether this pane's pty is one a conhost renders into, and so whether
    /// the reader puts back the cursor a repaint parked. Decided per pane from
    /// its [`PtySource`], and shared rather than copied because the reader can
    /// learn better mid-stream — see the `RemoteContext` arm.
    local_conpty: Arc<AtomicBool>,
    /// Whether the program in the pane switched on DEC mode 2031.
    color_scheme_updates: Arc<AtomicBool>,
    /// Lock-free state mirror updated after grid mutations.
    mirror: Arc<TermStateMirror>,
}

/// What kind of pty is at the far end of a pane's link, which is what decides
/// whether a conhost stands between the application and us.
///
/// The distinction is not the platform this client was built for. A Windows
/// client's panes are a mix: a local shell — or `wsl.exe`, or an `ssh` client,
/// which are ordinary programs inside the same ConPTY — is rendered by conhost,
/// while a pane on a remote `anterm-server` (Linux or macOS only, see
/// [`anterm_core::daemon::install::asset::asset_for_uname`]) or on a native-SSH
/// channel is a raw unix pty whose bytes reach us untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtySource {
    /// A pty this machine's Windows daemon opened: a ConPTY, with conhost
    /// painting frames into it.
    LocalConpty,
    /// A pty nothing repaints on our behalf — every pty on unix, and every pty
    /// reached over a link.
    Raw,
}

impl PtySource {
    /// The source a pane spawned or attached on `route` reads from. The
    /// platform enters here and nowhere else: a local route means a pty this
    /// machine opened, and only on Windows is that a ConPTY.
    ///
    /// `Unroutable` is a route that could not be resolved, so it never gets a
    /// pty at all; answering as if it were local costs nothing and keeps the
    /// match total.
    fn for_route(route: &PaneRoute) -> PtySource {
        match route {
            PaneRoute::Local | PaneRoute::Unroutable(_) if cfg!(windows) => PtySource::LocalConpty,
            _ => PtySource::Raw,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum PaneRoute {
    #[default]
    Local,
    #[allow(dead_code)]
    Remote {
        header: Box<anterm_core::daemon::router::RouteHeader>,
        resize_echo: bool,
        size_lease: bool,
        clear_scrollback: bool,
    },
    #[allow(dead_code)]
    Unroutable(String),
}

impl PaneRoute {
    pub fn header(&self) -> Option<&anterm_core::daemon::router::RouteHeader> {
        match self {
            PaneRoute::Remote { header, .. } => Some(header),
            PaneRoute::Local | PaneRoute::Unroutable(_) => None,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PaneRoute::Local)
    }

    fn allow_remote_clipboard_write(&self) -> bool {
        match self {
            PaneRoute::Remote { header, .. } => match &header.target {
                anterm_core::daemon::router::RouteTarget::Ssh(spec) => spec.remote_clipboard_write,
                _ => false,
            },
            PaneRoute::Local | PaneRoute::Unroutable(_) => false,
        }
    }
}

/// How much unsent input a pane holds before it calls the link lost. A link
/// that is draining never gets near this: the sender thread is parked waiting
/// for work, so the queue holds at most the frames of one burst. Reaching it
/// means nothing on the far side has taken a byte for as long as it takes to
/// type — or paste — four megabytes, which is a dead link, not a slow one.
const MAX_BACKLOG: usize = 4 << 20;

/// How long a teardown lets the sender finish what is already queued before it
/// cuts the socket out from under it. `Detach` is the last frame a pane sends
/// and it is worth a moment: on a draining link the sender is idle, so it goes
/// out in microseconds and this returns at once. On a link that has stopped
/// draining it will never go out at all, and closing the pane must not wait
/// around to discover that — the daemon reads the closed socket as a detach
/// anyway.
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(50);

/// How long a resize the daemon promised to echo is taken to still be on its
/// way. Past this, a grid that has not reached the requested geometry is not
/// waiting on anything: the request or its echo went missing, and the size is
/// sent again. The local daemon echoes in well under a millisecond and a
/// routed one within a round trip; a resend that beats a slow echo costs one
/// `TIOCSWINSZ` of the size the pty already has, which signals nobody.
const RESIZE_ECHO_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether `term`'s grid is `size`, as the emulator would size it — it never
/// goes below `MIN_COLUMNS` × `MIN_SCREEN_LINES`, so a sliver of a pane asking
/// for one column is already as narrow as it can get.
fn grid_holds(term: &Term<EventProxy>, size: TermSize) -> bool {
    use alacritty_terminal::grid::Dimensions as _;
    use alacritty_terminal::term::{MIN_COLUMNS, MIN_SCREEN_LINES};
    term.columns() == size.cols.max(MIN_COLUMNS)
        && term.screen_lines() == size.rows.max(MIN_SCREEN_LINES)
}

/// What the sender thread needs to report a link that stopped taking input.
/// The signals the pane holds, cloned out so the failure can be raised from the
/// thread that actually meets it.
#[derive(Clone)]
struct InputLoss {
    /// Set by the first frame *this* link refused, so the loss is said once
    /// rather than once per keystroke. One per link rather than one per pane: a
    /// relink hands the retired sender's last, doomed write and the new
    /// sender's first real one two different flags, so the dying link cannot
    /// spend the new link's one chance to speak.
    said: Arc<AtomicBool>,
    reader_quit: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    proxy: EventProxy,
}

impl InputLoss {
    fn new(reader_quit: Arc<AtomicBool>, exited: Arc<AtomicBool>, proxy: EventProxy) -> InputLoss {
        InputLoss {
            said: Arc::new(AtomicBool::new(false)),
            reader_quit,
            exited,
            proxy,
        }
    }

    /// The link refused a frame. Every keystroke after the first would say the
    /// same thing, so this side says it once; and unless the reader has been
    /// retired for a relink or a release, the pane is marked exited the way the
    /// reader marks it on EOF — it is the same socket, noticed from the writing
    /// side first — so the window shows the pane as gone instead of taking
    /// input into it that nothing will ever read. The reader still raises its
    /// own `Exit` when it finds the same socket closed; the handler is
    /// idempotent, so a link that is genuinely gone may be reported twice.
    ///
    /// This is hardening for a *closed* link, not the cure for #673: a socket
    /// some process holds open and never reads accepts writes into its send
    /// buffer, so they succeed and vanish until the buffer fills, and nothing
    /// here fires until the backlog bound does. What stops that pane existing
    /// at all is `attach_on` refusing to call a silent `Attach` attached.
    fn note(&self, err: &std::io::Error) {
        if self.said.swap(true, Ordering::SeqCst) {
            return;
        }
        log::warn!("the daemon link stopped taking this pane's input: {err}");
        if self.reader_quit.load(Ordering::SeqCst) {
            return;
        }
        self.exited.store(true, Ordering::SeqCst);
        self.proxy.send_event(AlacEvent::Wakeup);
        self.proxy.send_event(AlacEvent::Exit);
    }
}

#[derive(Default)]
struct SendQueue {
    frames: VecDeque<Vec<u8>>,
    /// Bytes queued but not yet on the wire — including the batch the sender
    /// currently holds, which is why this is not just `frames`' total. That
    /// batch is exactly what a stalled link parks in, so leaving it out would
    /// mean the backlog bound could never be reached.
    bytes: usize,
    /// How much of `bytes` belongs to frames that were over the whole bound on
    /// their own, and so were let through on their own terms. The bound is
    /// raised by exactly this while they are outstanding, and put back the
    /// moment the queue empties — otherwise one big paste would leave every
    /// keystroke behind it looking like a dead link.
    oversize: usize,
    /// Set by teardown, or by a sender that has given up: either way the queue
    /// takes nothing more. Teardown's sender writes what is left first.
    closing: bool,
    /// Set by the sender once there is nothing left to write. Teardown waits
    /// on this for `DRAIN_GRACE`, no longer.
    drained: bool,
}

/// A pane's writing half, moved onto a thread of its own.
///
/// The socket underneath is blocking and has no write timeout, so a peer that
/// stops reading parks `write(2)` in the kernel until it starts again. Every
/// caller of `RemoteTerminal::write` is a gpui event handler — a keystroke, a
/// paste, a mouse report, a focus change — and one UI thread draws every
/// window, so a park there is every window frozen. On macOS a unix stream gives
/// up after 8K of send buffer, which is about 1400 keystrokes: a single paste.
///
/// So nothing on the UI thread touches the socket. Frames are encoded, queued,
/// and handed to a sender thread that is welcome to park for as long as the far
/// end makes it.
struct LinkWriter {
    /// A second handle on the same socket, kept for `shutdown` alone.
    /// `shutdown(2)` returns at once even while another thread is parked in
    /// `write(2)` on that socket — which is exactly the state teardown has to
    /// be able to break, and exactly what a handle behind the sender's own lock
    /// could not do.
    closer: Stream,
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
    thread: Option<JoinHandle<()>>,
}

impl LinkWriter {
    fn new(stream: Stream, loss: InputLoss) -> std::io::Result<LinkWriter> {
        let closer = stream.try_clone()?;
        let queue = Arc::new((Mutex::new(SendQueue::default()), std::sync::Condvar::new()));
        let sending = Arc::clone(&queue);
        let sender_loss = loss.clone();
        let thread = std::thread::Builder::new()
            .name("anterm-pane-writer".into())
            .spawn(move || send_loop(stream, sending, sender_loss))?;
        Ok(LinkWriter {
            closer,
            queue,
            loss,
            thread: Some(thread),
        })
    }

    /// Queues a frame. Never blocks: the socket belongs to the sender thread,
    /// and the only thing that happens here is a push onto a `VecDeque`.
    fn send(&self, msg: ClientMsg) {
        let mut frame = Vec::new();
        if let Err(e) = msg.encode(&mut frame) {
            log::warn!("could not encode a frame for this pane's link: {e}");
            return;
        }
        let (lock, wake) = &*self.queue;
        let Ok(mut q) = lock.lock() else { return };
        if q.closing {
            return;
        }
        // A frame over the bound all by itself is not a backlog — a paste is
        // whatever the clipboard holds, and refusing a big one would kill a
        // perfectly healthy pane. Onto an empty queue it goes through anyway,
        // and lifts the bound by its own size for as long as it is outstanding,
        // so what piles up behind it is still held to the same four megabytes.
        // Onto a queue that already has something on it, the ordinary bound
        // applies: one such frame is a paste, a second one arriving before the
        // first has moved is a link that is not moving.
        let oversize = frame.len() > MAX_BACKLOG && q.bytes == 0;
        if !oversize && q.bytes + frame.len() > MAX_BACKLOG + q.oversize {
            // Dropped rather than queued: a backlog this deep is a link nothing
            // is reading, and growing it only trades a frozen window for an
            // exhausted heap. Reported in the same words, and once, as a write
            // the link refuses outright — the pane is gone either way.
            drop(q);
            self.loss.note(&std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!("nothing has drained this pane's link for {MAX_BACKLOG} bytes of input"),
            ));
            return;
        }
        if oversize {
            q.oversize = frame.len();
        }
        q.bytes += frame.len();
        q.frames.push_back(frame);
        q.drained = false;
        drop(q);
        wake.notify_one();
    }

    /// Retires the sender and cuts the link. Gives what is queued `DRAIN_GRACE`
    /// to go out — see the constant — and then shuts the socket down whether it
    /// went or not. Never joins: a sender parked on a dead link would take the
    /// UI thread down with it, the same trap `stop_reader` documents.
    ///
    /// Called twice on the way out — `stop_reader` closes the link, then the
    /// field drop closes it again — so the second call has to be free rather
    /// than another `DRAIN_GRACE` spent waiting for a sender that is already
    /// gone. A retired handle is one whose thread has been let go.
    fn close(&mut self) {
        if self.thread.is_none() {
            return;
        }
        let (lock, wake) = &*self.queue;
        if let Ok(mut q) = lock.lock() {
            q.closing = true;
            wake.notify_one();
            if let Ok((waited, _)) = wake.wait_timeout_while(q, DRAIN_GRACE, |q| !q.drained) {
                drop(waited);
            }
        }
        let _ = self.closer.shutdown(std::net::Shutdown::Both);
        drop(self.thread.take());
    }
}

impl Drop for LinkWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn send_loop(
    mut stream: Stream,
    queue: Arc<(Mutex<SendQueue>, std::sync::Condvar)>,
    loss: InputLoss,
) {
    use std::io::Write as _;
    let (lock, wake) = &*queue;

    // Abandons whatever is still queued and reports the link settled. Wakes a
    // teardown that is inside `DRAIN_GRACE` waiting to hear it: what is left
    // here is never going out, and there is nothing to be gained by making the
    // window sit out the rest of the grace period to find that out. Closes the
    // queue on the way, too — with no thread left to drain it, anything queued
    // after this is just a keystroke held onto until the pane drops.
    let give_up = || {
        if let Ok(mut q) = lock.lock() {
            q.frames.clear();
            q.bytes = 0;
            q.oversize = 0;
            q.closing = true;
            q.drained = true;
            wake.notify_all();
        }
    };

    loop {
        let batch = {
            let Ok(mut q) = lock.lock() else { return };
            loop {
                if !q.frames.is_empty() {
                    break;
                }
                // Nothing left to write, so the link is as flushed as this
                // thread can make it. Said before the `closing` check, so a
                // teardown racing a sender that has already finished hears it
                // rather than waiting out `DRAIN_GRACE` for nothing.
                q.drained = true;
                wake.notify_all();
                if q.closing {
                    return;
                }
                let Ok(next) = wake.wait(q) else { return };
                q = next;
            }
            std::mem::take(&mut q.frames)
        };
        // Counted against the backlog until it is actually out. Discounting it
        // at the moment it left the `VecDeque` would let a sender parked on the
        // first frame of a huge batch hold the whole thing off the books, and
        // the bound the batch is meant to enforce would never be reached.
        let taken: usize = batch.iter().map(Vec::len).sum();
        // Written with the lock released: parking here is the whole point, and
        // a sender holding the queue lock while it parked would put every
        // `send` — every keystroke — behind the same wait it exists to absorb.
        for frame in batch {
            if let Err(e) = stream.write_all(&frame) {
                loss.note(&e);
                give_up();
                return;
            }
        }
        if let Err(e) = stream.flush() {
            loss.note(&e);
            give_up();
            return;
        }
        if let Ok(mut q) = lock.lock() {
            q.bytes = q.bytes.saturating_sub(taken);
            if q.bytes == 0 {
                q.oversize = 0;
            }
        }
    }
}

pub struct RemoteTerminal {
    pub term: Arc<FairMutex<Term<EventProxy>>>,
    pub events: EventReceiver,
    mirror: Arc<TermStateMirror>,
    pub palette: [alacritty_terminal::vte::ansi::Rgb; 256],
    pub exited: bool,
    size: TermSize,
    synced_size: bool,
    /// This link reattached to a pane that was already running rather than
    /// spawning a fresh one, so whatever it reports first — an agent in the
    /// foreground included — predates this client.
    reattached: bool,
    /// The `(cell_w, cell_h)` last sent to the daemon, in device pixels. Tracked
    /// alongside `size` so a display-scale change still reaches the child even
    /// when the grid dimensions are unchanged.
    synced_cell: (u16, u16),
    /// When the last `ClientMsg::Resize` went down the link. On an echoing
    /// route it is what tells "the echo is still in flight" apart from "the
    /// echo is never coming" — see [`RESIZE_ECHO_GRACE`].
    resize_sent_at: Option<std::time::Instant>,
    link: LinkWriter,
    cwd: Arc<Mutex<Option<PathBuf>>>,
    shell_state: Arc<Mutex<ShellState>>,
    remote_context: Arc<Mutex<Option<RemoteContext>>>,
    exited_flag: Arc<AtomicBool>,
    child_exited: Arc<AtomicBool>,
    zle_reading: Arc<AtomicBool>,
    /// The line the shell said it is running, from the OSC 133;C mark, still
    /// percent-escaped as the integration sent it. Empty while at a prompt.
    ///
    /// This is the only place the *text* of a running command exists on the
    /// client: the frame the daemon sends says a command runs, not which one.
    /// The close confirmation has to name what it is about to end.
    running_command: Arc<Mutex<String>>,
    submitted_commands: Arc<Mutex<Vec<(String, u64)>>>,
    auth_prompts: Arc<Mutex<VecDeque<(u64, AuthPromptKind)>>>,
    ssh_phase: Arc<Mutex<Option<SshPhase>>>,
    ssh_endpoint: Option<(String, u16)>,
    /// The account the SSH connection authenticates as. `ssh_endpoint` is what
    /// the disconnect strip and the forward sheet need; the keychain files a
    /// password under the user as well, so the auth sheet needs this too.
    ssh_user: Option<String>,
    auto_supplied_password: bool,
    agent: Arc<Mutex<Option<CLIAgent>>>,
    agent_session: Arc<Mutex<AgentSlot>>,
    /// Kitty-graphics images placed on this pane's grid (issue #213).
    /// Written by the reader thread from out-of-band `Image`/`DeleteImage`
    /// frames, read by the paint path — only the client holds the grid the
    /// anchors are relative to, so the store lives here rather than in the daemon.
    images: crate::render::images::ImageStore,
    clipboard_writes: Arc<Mutex<VecDeque<anterm_core::clipboard::ClipboardWrite>>>,
    osc_notes: OscNotes,
    /// On the pane rather than its view so it survives a relink; only the
    /// view's poll touches it.
    note_budget: Mutex<NoteBudget>,
    clipboard_write_busy: Arc<AtomicBool>,
    /// Who runs this pane at their own size — a phone, by the name it paired
    /// under — while the daemon says so. See [`Self::take_back`].
    lease: Arc<Mutex<Option<String>>>,
    route: PaneRoute,
    proxy: EventProxy,
    reader_thread: Option<JoinHandle<()>>,
    /// Tells the *current* reader thread to bow out. Teardown must never wait
    /// for the reader: on Windows, `shutdown()` does not wake a thread parked
    /// in a blocking `read()` on the same socket, so joining it from the UI
    /// thread hangs the whole window whenever the peer has gone silent (the
    /// exact state a zombie SSH leg leaves behind). The reader re-checks this
    /// flag under the term lock before every grid mutation, so once it is set
    /// the abandoned thread can only exit, never write.
    reader_quit: Arc<AtomicBool>,
    /// Whether this pane's pty is a ConPTY, for input encoding and repairing
    /// the cursor a repaint parks. Held here so a relink hands the same answer
    /// to the reader it starts: a pane's pty does not change kind when the link
    /// to it is rebuilt, and the route a relink carries cannot tell a
    /// native-SSH pane from a local shell.
    local_conpty: Arc<AtomicBool>,
    /// Whether the program in the pane switched on DEC mode 2031.
    color_scheme_updates: Arc<AtomicBool>,
}

impl RemoteTerminal {
    pub fn spawn(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
    ) -> anyhow::Result<(Self, u64)> {
        Self::spawn_on(
            &PaneRoute::Local,
            size,
            cell_w,
            cell_h,
            cwd,
            shell,
            None,
            None,
        )
    }

    pub fn spawn_on(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
    ) -> anyhow::Result<(Self, u64)> {
        let retry_cwd = cwd.clone();
        let retry_shell = shell.clone();
        let retry_owner = owner.clone();
        let retry_restore = restore.clone();
        match Self::spawn_once(route, size, cell_w, cell_h, cwd, shell, owner, restore) {
            Ok(term) => Ok(term),
            Err(first_err) if route.is_local() && daemon_not_listening(&first_err) => {
                log::warn!(
                    "local daemon unavailable during spawn; attempting recovery: {first_err:#}"
                );
                if let Err(start_err) = anterm_core::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon not running ({first_err}); starting one failed: {start_err}"
                    ));
                }
                Self::spawn_once(route, size, cell_w, cell_h, retry_cwd, retry_shell, retry_owner, retry_restore)
                    .map_err(|second_err| {
                        anyhow::anyhow!(
                            "daemon not running ({first_err}); started one but Spawn still failed: {second_err}"
                        )
                    })
            }
            Err(first_err)
                if route.is_local() && daemon_disconnected_before_spawn_reply(&first_err) =>
            {
                log::warn!(
                    "spawn connection ended before reply; probing daemon without forcing a restart: {first_err:#}"
                );
                if let Err(restart_err) = anterm_core::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon disconnected before Spawn reply ({first_err}); recovery failed: {restart_err}"
                    ));
                }
                Self::spawn_once(route, size, cell_w, cell_h, retry_cwd, retry_shell, retry_owner, retry_restore).map_err(|second_err| {
                    anyhow::anyhow!(
                        "daemon disconnected before Spawn reply ({first_err}); recovery attempted but Spawn still failed: {second_err}"
                    )
                })
            }
            Err(err) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_once(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cwd: Option<PathBuf>,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
    ) -> anyhow::Result<(Self, u64)> {
        let mut stream = connect_routed(route)?;
        let win = win_size(size, cell_w, cell_h);

        let spawned_in = cwd.clone();

        let owner = owner.filter(|_| {
            route.is_local()
                && anterm_core::daemon::spawn::local_daemon_supports(
                    anterm_core::daemon::protocol::FEATURE_PANE_OWNER,
                )
        });

        // Only the local daemon's feature list is known here; a routed spawn
        // reaches a server whose build we have not asked about, and one that
        // predates the field would silently drop it. Sending it anyway would
        // cost nothing but would make the log claim a restore that never
        // happened, so the local case is the only one that asks.
        let restore = restore.and_then(|want| {
            let local = route.is_local();
            let supported = anterm_core::daemon::spawn::local_daemon_supports(
                anterm_core::daemon::protocol::FEATURE_RESTORE_SCROLLBACK,
            );
            if local && supported {
                return Some(want);
            }
            // Said out loud, because the symptom of dropping it here is a pane
            // that opens blank — which is also what a pane with nothing stored
            // looks like, and what a daemon that refused would produce. Three
            // causes, one appearance; without this line the only way to tell
            // them apart is to read the source.
            log::info!(
                "not asking to restore pane {}'s screen: local={local} \
                 daemon-supports-restore={supported}",
                want.pane_id
            );
            None
        });

        ClientMsg::Spawn {
            cwd,
            size: win,
            shell,
            owner,
            restore,
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let pane_id = match spawn_reply(&mut stream, attach_reply_wait(route), "Spawn")? {
            DaemonMsg::Spawned { pane_id } => pane_id,
            // Passed through, not wrapped: the caller already logs which
            // spawn this was, and the window shows only this text.
            DaemonMsg::Error(msg) => return Err(anyhow::anyhow!(msg)),
            other => {
                return Err(anyhow::anyhow!(
                    "unexpected daemon reply to Spawn: {other:?}"
                ));
            }
        };

        let mut term =
            Self::from_stream_with(stream, size, Vec::new(), PtySource::for_route(route))?;
        // The daemon just created this PTY at `win_size(size, cell_w, cell_h)`.
        // Recording that initial size avoids a redundant first-frame `Resize`
        // (and spurious `SIGWINCH` during zsh startup `PROMPT_SP`) when layout
        // lands on the same grid dimensions.
        term.synced_size = true;
        term.synced_cell = (cell_w, cell_h);
        term.route = route.clone();
        term.seed_cwd(spawned_in);
        term.watch_leases();
        Ok((term, pane_id))
    }

    /// Remember the directory the daemon was asked to spawn in, so everything
    /// that keys off a pane's cwd — the sidebar's repo grouping above all —
    /// has an answer before the shell gets far enough to report its own via
    /// OSC 7. The reader thread is already running, so a report that beat us
    /// here wins: it describes where the shell actually landed, which is not
    /// always where we asked (a missing directory sends the daemon home, an
    /// rc file may `cd` on its own).
    pub fn seed_cwd(&self, cwd: Option<PathBuf>) {
        let Some(cwd) = cwd else { return };
        if let Ok(mut guard) = self.cwd.lock() {
            guard.get_or_insert(cwd);
        }
    }

    pub fn attach(size: TermSize, cell_w: u16, cell_h: u16, pane_id: u64) -> anyhow::Result<Self> {
        Self::attach_on(&PaneRoute::Local, size, cell_w, cell_h, pane_id)
    }

    pub fn attach_on(
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        pane_id: u64,
    ) -> anyhow::Result<Self> {
        let mut stream = connect_routed(route)?;
        let win = win_size(size, cell_w, cell_h);

        ClientMsg::Attach {
            pane_id,
            size: win,
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let buffered = match attach_reply_prefix(&mut stream, pane_id, attach_reply_wait(route)) {
            Ok(buffered) => buffered,
            Err(e) if route.is_local() && attach_unanswered(&e) => {
                // Silence on the attach socket has two readings, and only one
                // of them is safe to act on. Ask the daemon on a fresh
                // connection: if it answers `Version` there, it is up and
                // serving, and the attach connection is one it will never
                // serve — a socket some process holds open, or one from a
                // listener no longer drained — so the verdict stands and the
                // caller spawns fresh. If it does not answer there either, it
                // is not serving anyone yet — mid-restart, mid-handoff — and
                // a fresh pane spawned now would land on a live one the moment
                // it comes up (its history carried across, its agent session
                // resumed twice). There is no third path from a synchronous
                // UI-thread call, so the attach still fails, but says which
                // silence it was: the log line someone reads while diagnosing
                // an orphaned shell must not claim the pane was gone.
                //
                // Only local routes probe: a remote attach already waits 15 s
                // and a second routed connection is a second bridge process.
                if local_daemon_answers() {
                    return Err(e);
                }
                return Err(e.context(
                    "the daemon answered nothing on a fresh connection either — it is not \
                     serving yet (restarting?), so this pane may still be alive",
                ));
            }
            Err(e) => return Err(e),
        };
        let mut term =
            Self::from_stream_parts(stream, size, buffered, PtySource::for_route(route), true)?;
        term.route = route.clone();
        term.watch_leases();
        Ok(term)
    }

    /// Opens the daemon connection a relink will adopt, and waits for the
    /// daemon's verdict on the `Attach`. Bytes read past the verdict are
    /// returned so the adopted reader loses none of the replay. Waiting here
    /// is what makes a relink's failure classifiable: fire-and-forget handed
    /// a "no such pane" refusal to the reader thread, which could only die
    /// with it — indistinguishable from the link dropping again.
    pub fn open_relink(
        route: &PaneRoute,
        pane_id: u64,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
    ) -> anyhow::Result<(Stream, Vec<u8>)> {
        let mut stream = connect_routed(route)?;
        ClientMsg::Attach {
            pane_id,
            size: win_size(size, cell_w, cell_h),
            allow_remote_clipboard_write: route.allow_remote_clipboard_write(),
        }
        .encode(&mut stream)?;
        let buffered = attach_reply_prefix(&mut stream, pane_id, attach_reply_wait(route))?;
        Ok((stream, buffered))
    }

    pub fn adopt_relink(
        &mut self,
        stream: Stream,
        buffered: Vec<u8>,
        route: &PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
    ) -> anyhow::Result<()> {
        self.stop_reader();
        while self.events.try_recv().is_ok() {}
        self.events.clear_wakeup_flag();
        if let Ok(mut pending) = self.clipboard_writes.lock() {
            pending.clear();
        }
        self.clipboard_write_busy.store(false, Ordering::Release);

        let read_half = stream.try_clone()?;

        // A relink keeps the view, and the view keeps the status it last saw
        // before the link dropped. That is already a baseline, and the better
        // one: if the agent finished its turn while the link was down, the
        // daemon's replayed `Done` against the view's `Working` is exactly the
        // edge the reader must be told about. So the relink's replay is read
        // as a live report, and a cold-attach mark nobody took yet is dropped
        // rather than left to swallow that edge.
        if let Ok(mut guard) = self.agent_session.lock() {
            guard.replayed = false;
        }

        self.exited_flag.store(false, Ordering::SeqCst);
        self.exited = false;
        {
            use alacritty_terminal::vte::ansi::Handler as _;
            let mut term = self.term.lock();
            term.reset_state();
            self.mirror.publish(&term);
        }
        // The grid was just reset, so every image anchor now points nowhere.
        // Drop them; the daemon does not replay out-of-band image frames, so a
        // browser redraws on its next transmit (see issue #213's reattach note).
        self.images.clear();

        let quit = Arc::new(AtomicBool::new(false));
        let reader = Self::spawn_reader(
            self.term.clone(),
            self.proxy.clone(),
            read_half,
            buffered,
            quit.clone(),
            ReaderSignals {
                cwd: self.cwd.clone(),
                shell: self.shell_state.clone(),
                remote: self.remote_context.clone(),
                agent: self.agent.clone(),
                agent_session: self.agent_session.clone(),
                // The daemon does replay the stored status down this link, but
                // the view already has a baseline from before the drop — see
                // above.
                awaiting_replay: false,
                exited: self.exited_flag.clone(),
                child_exited: self.child_exited.clone(),
                zle_reading: self.zle_reading.clone(),
                running_command: self.running_command.clone(),
                submitted_commands: self.submitted_commands.clone(),
                auth: self.auth_prompts.clone(),
                phase: self.ssh_phase.clone(),
                images: self.images.clone(),
                clipboard_writes: self.clipboard_writes.clone(),
                osc_notes: self.osc_notes.clone(),
                clipboard_write_busy: self.clipboard_write_busy.clone(),
                lease: self.lease.clone(),
                // Deliberately the pane's existing answer rather than one
                // rebuilt from `route`: the pty on the far side is the same pty
                // it was before the link dropped, and only this value still
                // remembers what a `RemoteContext` taught the old reader.
                local_conpty: self.local_conpty.clone(),
                color_scheme_updates: self.color_scheme_updates.clone(),
                mirror: self.mirror.clone(),
            },
        );
        self.reader_thread = Some(reader);
        self.reader_quit = quit;
        // Installed after `reader_quit`, so the sender reports a refusal
        // against the reader this link actually has. Assigning retires the old
        // `LinkWriter` through its `Drop`, which is what closes the old socket.
        self.link = LinkWriter::new(stream, self.input_loss())?;
        self.route = route.clone();
        // The new link has told us nothing yet; the daemon repeats a lease
        // that still stands when we ask again.
        if let Ok(mut guard) = self.lease.lock() {
            *guard = None;
        }
        self.watch_leases();
        self.synced_size = false;
        self.resize(size, cell_w, cell_h);
        Ok(())
    }

    /// A pane the client *reattached* to rather than spawned — the shape the
    /// app restores last session's tabs in, where the head of the stream is the
    /// daemon replaying state the pane already had.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_stream_reattached(stream: Stream, size: TermSize) -> anyhow::Result<Self> {
        Self::from_stream_parts(
            stream,
            size,
            Vec::new(),
            PtySource::for_route(&PaneRoute::Local),
            true,
        )
    }

    /// A pane on a pty of this machine's own — what the tests build, and what
    /// `spawn_on` narrows with the route it dialled.
    pub fn from_stream(stream: Stream, size: TermSize) -> anyhow::Result<Self> {
        Self::from_stream_with(
            stream,
            size,
            Vec::new(),
            PtySource::for_route(&PaneRoute::Local),
        )
    }

    pub(super) fn from_stream_with(
        stream: Stream,
        size: TermSize,
        buffered: Vec<u8>,
        pty: PtySource,
    ) -> anyhow::Result<Self> {
        Self::from_stream_parts(stream, size, buffered, pty, false)
    }

    /// `awaiting_replay` says this link is an attach rather than a spawn, and
    /// so that the frames at the head of its stream describe a pane that was
    /// already running — see [`AgentSlot`]. It has to be decided here rather
    /// than set on the returned terminal: the reader starts inside this
    /// function, and against a daemon that answers promptly the replay can be
    /// parsed before the caller gets its value back.
    fn from_stream_parts(
        stream: Stream,
        size: TermSize,
        buffered: Vec<u8>,
        pty: PtySource,
        awaiting_replay: bool,
    ) -> anyhow::Result<Self> {
        let read_half = stream.try_clone()?;
        let write_half = stream;

        let (tx, rx) = smol::channel::unbounded();
        let wakeup_queued = Arc::new(AtomicBool::new(false));
        let proxy = EventProxy {
            tx,
            replaying: Arc::new(AtomicBool::new(false)),
            wakeup_queued: wakeup_queued.clone(),
        };
        let events = EventReceiver::new(rx, wakeup_queued);

        let user_config = anterm_ui_kit::config::Config::load();
        let config = terminal_config_from_user(&user_config);
        let term = Term::new(config, &size, proxy.clone());
        let mirror = Arc::new(TermStateMirror::from_term(&term));
        let term = Arc::new(FairMutex::new(term));

        let cwd: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let shell_state: Arc<Mutex<ShellState>> = Arc::new(Mutex::new(ShellState::default()));
        let remote_context: Arc<Mutex<Option<RemoteContext>>> = Arc::new(Mutex::new(None));
        let agent: Arc<Mutex<Option<CLIAgent>>> = Arc::new(Mutex::new(None));
        let agent_session: Arc<Mutex<AgentSlot>> = Arc::new(Mutex::new(AgentSlot::default()));
        let exited_flag = Arc::new(AtomicBool::new(false));
        let child_exited = Arc::new(AtomicBool::new(false));
        let zle_reading = Arc::new(AtomicBool::new(false));
        let running_command: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let submitted_commands: Arc<Mutex<Vec<(String, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let auth_prompts: Arc<Mutex<VecDeque<(u64, AuthPromptKind)>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let ssh_phase: Arc<Mutex<Option<SshPhase>>> = Arc::new(Mutex::new(None));
        let images = crate::render::images::ImageStore::new();
        let clipboard_writes = Arc::new(Mutex::new(VecDeque::new()));
        let osc_notes = OscNotes::default();
        let clipboard_write_busy = Arc::new(AtomicBool::new(false));
        let lease = Arc::new(Mutex::new(None));

        let reader_quit = Arc::new(AtomicBool::new(false));
        let local_conpty = Arc::new(AtomicBool::new(pty == PtySource::LocalConpty));
        let color_scheme_updates = Arc::new(AtomicBool::new(false));
        let reader_thread = Self::spawn_reader(
            term.clone(),
            proxy.clone(),
            read_half,
            buffered,
            reader_quit.clone(),
            ReaderSignals {
                cwd: cwd.clone(),
                shell: shell_state.clone(),
                remote: remote_context.clone(),
                agent: agent.clone(),
                agent_session: agent_session.clone(),
                awaiting_replay,
                exited: exited_flag.clone(),
                child_exited: child_exited.clone(),
                zle_reading: zle_reading.clone(),
                running_command: running_command.clone(),
                submitted_commands: submitted_commands.clone(),
                auth: auth_prompts.clone(),
                phase: ssh_phase.clone(),
                images: images.clone(),
                clipboard_writes: clipboard_writes.clone(),
                osc_notes: osc_notes.clone(),
                clipboard_write_busy: clipboard_write_busy.clone(),
                lease: lease.clone(),
                local_conpty: local_conpty.clone(),
                color_scheme_updates: color_scheme_updates.clone(),
                mirror: mirror.clone(),
            },
        );

        let link = LinkWriter::new(
            write_half,
            InputLoss::new(reader_quit.clone(), exited_flag.clone(), proxy.clone()),
        )?;

        Ok(Self {
            term,
            events,
            mirror,
            palette: crate::render::palette::build(),
            exited: false,
            size,
            synced_size: false,
            reattached: awaiting_replay,
            synced_cell: (0, 0),
            resize_sent_at: None,
            link,
            cwd,
            shell_state,
            remote_context,
            exited_flag,
            child_exited,
            zle_reading,
            running_command,
            submitted_commands,
            auth_prompts,
            ssh_phase,
            ssh_endpoint: None,
            ssh_user: None,
            auto_supplied_password: false,
            agent,
            agent_session,
            images,
            clipboard_writes,
            osc_notes,
            note_budget: Mutex::default(),
            clipboard_write_busy,
            lease,
            route: PaneRoute::Local,
            proxy,
            reader_thread: Some(reader_thread),
            reader_quit,
            local_conpty,
            color_scheme_updates,
        })
    }

    /// The signals the sender thread raises a refused frame through, bundled
    /// for the `LinkWriter` about to be installed. Read after `reader_quit` has
    /// been swapped, so a relink's new sender answers to the new reader.
    fn input_loss(&self) -> InputLoss {
        InputLoss::new(
            self.reader_quit.clone(),
            self.exited_flag.clone(),
            self.proxy.clone(),
        )
    }

    pub fn detach_link(&mut self) {
        self.link.send(ClientMsg::Detach);
        self.stop_reader();
        self.poll_exited();
    }

    /// Retires the current reader thread without waiting for it. Joining is
    /// not an option here: the callers run on the UI thread, and on Windows
    /// `shutdown()` does not wake a reader parked in a blocking `read()`, so
    /// against a silent peer the join would hang the whole window (the reader
    /// only unblocks when the peer eventually closes — which a zombie SSH leg
    /// never does). Instead the reader is told to quit and abandoned; it wakes
    /// within one read-timeout tick, sees the flag, and exits without ever
    /// touching the grid again.
    fn stop_reader(&mut self) {
        self.reader_quit.store(true, Ordering::SeqCst);
        // The sender owns the socket now, and closing it is its job: `close`
        // gives whatever is queued a brief moment to go out and then shuts the
        // socket down regardless, which is also what wakes the reader.
        self.link.close();
        drop(self.reader_thread.take());
    }

    pub fn mirror(&self) -> &TermStateMirror {
        &self.mirror
    }

    pub fn grid_revision(&self) -> u64 {
        self.mirror.grid_revision()
    }

    /// Reads the terminal's current [`alacritty_terminal::term::TermMode`]
    /// without ever blocking the UI thread behind the reader's `FairMutex`.
    ///
    /// When the lock is uncontended, reads live state and refreshes the mirror
    /// (preserving exact semantics for tests that mutate `term.lock()` directly);
    /// when the reader holds the lock during streaming output, returns the
    /// latest atomically published mirror immediately.
    pub fn mode_nonblocking(&self) -> alacritty_terminal::term::TermMode {
        match self.term.try_lock_unfair() {
            Some(term) => {
                let mode = *term.mode();
                self.mirror.store_mode(mode);
                mode
            }
            None => self.mirror.mode(),
        }
    }

    pub fn history_size_nonblocking(&self) -> usize {
        use alacritty_terminal::grid::Dimensions as _;
        match self.term.try_lock_unfair() {
            Some(term) => {
                let h = term.grid().history_size();
                self.mirror.store_history_size(h);
                h
            }
            None => self.mirror.history_size(),
        }
    }

    pub fn display_offset_nonblocking(&self) -> usize {
        match self.term.try_lock_unfair() {
            Some(term) => {
                self.mirror.sync_from_term(&term);
                term.grid().display_offset()
            }
            None => self.mirror.display_offset(),
        }
    }

    pub fn has_selection_nonblocking(&self) -> bool {
        match self.term.try_lock_unfair() {
            Some(term) => {
                let has = term.selection.is_some();
                self.mirror.store_has_selection(has);
                has
            }
            None => self.mirror.has_selection(),
        }
    }

    pub fn publish_term_state(&self, term: &Term<EventProxy>) {
        self.mirror.publish(term);
    }

    pub fn sync_term_state(&self, term: &Term<EventProxy>) {
        self.mirror.sync_from_term(term);
    }

    pub fn apply_user_config(&self, user_config: &anterm_ui_kit::config::Config) {
        let mut term = self.term.lock();
        term.set_options(terminal_config_from_user(user_config));
    }

    pub fn poll_exited(&mut self) {
        if self.exited_flag.load(Ordering::SeqCst) {
            self.exited = true;
        }
    }

    pub fn child_exited(&self) -> bool {
        self.child_exited.load(Ordering::SeqCst)
    }

    pub(super) fn is_local_conpty(&self) -> bool {
        self.local_conpty.load(Ordering::Relaxed)
    }

    pub(super) fn color_scheme_updates(&self) -> bool {
        self.color_scheme_updates.load(Ordering::Relaxed)
    }

    /// Queues a keystroke — or a paste, or a mouse report — for the link.
    ///
    /// Callers are gpui event handlers on the UI thread, so this returns
    /// without touching the socket. A link that has stopped draining is a
    /// problem for the sender thread, not for the window.
    pub fn write<B: Into<Cow<'static, [u8]>>>(&self, bytes: B) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        self.link.send(ClientMsg::Input(bytes.into_owned()));
    }

    /// Whether the daemon behind this pane echoes a `DaemonMsg::Size` into the
    /// output stream when it applies our `ClientMsg::Resize`. When it does, the
    /// local grid reflow is deferred to that echo on the reader thread: any
    /// backlog still queued between daemon and client was produced at the old
    /// geometry, and reflowing before it drains parses old-width bytes into a
    /// new-width grid (maximize during a burst of output garbled the pane).
    ///
    /// The local daemon is probed directly. A routed pane consults the answer
    /// its route carries, read off the host's control hello when the route was
    /// built — the daemon serves one `ClientMsg` per connection, so asking
    /// `Version` per pane would cost a whole routed connection each time. The
    /// remaining cases never defer: an older server does not advertise the
    /// echo, an unroutable route reaches no daemon at all, and deferral must
    /// not hang the grid on an echo nobody promised.
    fn resize_echoed(&self) -> bool {
        match &self.route {
            PaneRoute::Local => anterm_core::daemon::spawn::local_daemon_supports(
                anterm_core::daemon::protocol::FEATURE_RESIZE_ECHO,
            ),
            PaneRoute::Remote { resize_echo, .. } => *resize_echo,
            PaneRoute::Unroutable(_) => false,
        }
    }

    /// Whether the daemon behind this pane knows size leases, asked the way
    /// [`Self::resize_echoed`] asks: never over the network from here.
    fn leases_supported(&self) -> bool {
        match &self.route {
            PaneRoute::Local => anterm_core::daemon::spawn::local_daemon_supports(
                anterm_core::daemon::protocol::FEATURE_SIZE_LEASE,
            ),
            PaneRoute::Remote { size_lease, .. } => *size_lease,
            PaneRoute::Unroutable(_) => false,
        }
    }

    /// Whether the daemon behind this pane knows `ClientMsg::ClearScrollback`.
    fn clear_scrollback_supported(&self) -> bool {
        match &self.route {
            PaneRoute::Local => anterm_core::daemon::spawn::local_daemon_supports(
                anterm_core::daemon::protocol::FEATURE_CLEAR_SCROLLBACK,
            ),
            PaneRoute::Remote {
                clear_scrollback, ..
            } => *clear_scrollback,
            PaneRoute::Unroutable(_) => false,
        }
    }

    /// Ask the daemon to wipe this pane's replay ring and persisted scrollback
    /// snapshot so a later reattach or restart cannot resurrect cleared output.
    pub fn clear_scrollback(&self) {
        if self.clear_scrollback_supported() {
            self.link.send(ClientMsg::ClearScrollback);
        }
    }

    /// Asks to be told when a phone runs this pane at its own size. Only
    /// where the daemon knows leases: an older one drops a link that sends it.
    fn watch_leases(&self) {
        if self.leases_supported() {
            self.link.send(ClientMsg::Lease(
                anterm_core::daemon::protocol::LeaseRequest::Watch,
            ));
        }
    }

    /// Who runs this pane at their own size right now, if anyone.
    pub fn leased_by(&self) -> Option<String> {
        self.lease.lock().ok().and_then(|guard| guard.clone())
    }

    /// Ends a phone's lease: the pane goes back to this window's size. The
    /// daemon confirms with the lease ending, which clears [`Self::leased_by`].
    pub fn take_back(&self) {
        self.link.send(ClientMsg::Lease(
            anterm_core::daemon::protocol::LeaseRequest::TakeBack,
        ));
    }

    pub fn resize(&mut self, size: TermSize, cell_w: u16, cell_h: u16) {
        let echoed = self.resize_echoed();
        // The cell size has to be part of the early-out, not just cols/rows: it
        // is reported in *device* pixels, so moving the window between a 2x and
        // a 1x display changes `ws_xpixel`/`ws_ypixel` while the grid stays
        // exactly the same. Comparing only `size` there would skip the resize
        // and leave a pixel-aware child rendering for the old framebuffer.
        let cell = (cell_w, cell_h);
        if self.synced_size && size == self.size && cell == self.synced_cell {
            // Already asked for, so the only question is whether the grid got
            // there. It is not enough to trust the request (#893): on an
            // echoing route the grid moves only when the daemon's `Size` comes
            // back, and a request or an echo that went missing left a pane
            // painting a grid shorter than its bounds — the bottom rows blank,
            // the child still drawing for the old size — with every later
            // frame asking for the same size and being skipped here, until a
            // divider drag asked for a different one.
            if echoed {
                // Disagreement while an echo can still be on its way is just
                // the echo (or a replay segment mid-apply) not having landed
                // yet. Asked first because it needs no lock.
                if self
                    .resize_sent_at
                    .is_some_and(|at| at.elapsed() < RESIZE_ECHO_GRACE)
                {
                    return;
                }
                // Tried rather than waited for, as the painter does: this runs
                // every frame, and a held lock is the reader mid-batch — the
                // next frame asks again.
                match self.term.try_lock_unfair() {
                    Some(term) if !grid_holds(&term, size) => {}
                    _ => return,
                }
            } else if grid_holds(&self.term.lock(), size) {
                return;
            }
        }
        self.synced_size = true;
        self.size = size;
        self.synced_cell = cell;
        if !echoed {
            let mode = if self.is_local_conpty() {
                crate::prompt::prompt_reflow::ResizeMode::Conpty
            } else {
                crate::prompt::prompt_reflow::ResizeMode::Standard {
                    shell_redraws: self.at_prompt(),
                }
            };
            let mut term = self.term.lock();
            crate::prompt::prompt_reflow::resize(&mut term, size, mode);
            self.mirror.publish(&term);
        }

        let win = win_size(size, cell_w, cell_h);
        self.link.send(ClientMsg::Resize(win));
        self.resize_sent_at = Some(std::time::Instant::now());
    }

    pub fn foreground_cwd(&self) -> Option<PathBuf> {
        self.cwd.lock().ok().and_then(|g| g.clone())
    }

    pub fn remote_context(&self) -> Option<RemoteContext> {
        self.remote_context.lock().ok().and_then(|g| g.clone())
    }

    /// See the field: an attach to a running pane, not a fresh spawn.
    pub fn reattached(&self) -> bool {
        self.reattached
    }

    pub fn at_prompt(&self) -> bool {
        self.shell_state
            .lock()
            .map(|s| s.active && s.at_prompt)
            .unwrap_or(false)
    }

    pub fn prompt_seq(&self) -> u64 {
        self.shell_state.lock().map(|s| s.seq).unwrap_or(0)
    }

    pub fn prompt_cycle(&self) -> u64 {
        self.shell_state.lock().map(|s| s.cycle).unwrap_or(0)
    }

    pub fn last_exit_code(&self) -> Option<i32> {
        self.shell_state.lock().ok().and_then(|s| s.last_exit)
    }

    pub fn shell_active(&self) -> bool {
        self.shell_state.lock().map(|s| s.active).unwrap_or(false)
    }

    pub fn foreground_agent(&self) -> Option<CLIAgent> {
        self.agent.lock().ok().and_then(|g| *g)
    }

    /// The kitty-graphics image store for this pane. Cheap handle clone — the
    /// store is an `Arc<Mutex<..>>` shared with the reader thread, which places
    /// and deletes images as out-of-band frames arrive from the daemon.
    pub fn images(&self) -> crate::render::images::ImageStore {
        self.images.clone()
    }

    /// Desktop notifications the program wrote (OSC 9, 99, 777), oldest
    /// first; the view decides whether each is shown.
    pub fn take_osc_notes(&self) -> Vec<Note> {
        self.osc_notes
            .lock()
            .map(|mut notes| notes.drain(..).collect())
            .unwrap_or_default()
    }

    /// Of `notes` about to be shown, the ones this pane's [`NoteBudget`] lets
    /// through, and whether notes it dropped earlier are due to be said.
    pub fn pace_osc_notes(
        &self,
        mut notes: Vec<Note>,
        now: std::time::Instant,
    ) -> (Vec<Note>, bool) {
        let Ok(mut budget) = self.note_budget.lock() else {
            return (notes, false);
        };
        let (show, dropped) = budget.admit(now, notes.len());
        notes.truncate(show);
        (notes, dropped)
    }

    pub fn pop_clipboard_write(&self) -> Option<anterm_core::clipboard::ClipboardWrite> {
        self.clipboard_writes
            .lock()
            .ok()
            .and_then(|mut pending| pending.pop_front())
    }

    pub fn finish_clipboard_write(&self) {
        self.clipboard_write_busy.store(false, Ordering::Release);
    }

    pub fn agent_session(&self) -> Option<AgentSessionState> {
        self.agent_session.lock().ok().and_then(|g| g.state.clone())
    }

    /// The status the daemon replayed when this link attached, handed out once.
    ///
    /// `Some(status)` means what [`Self::agent_session`] reports right now is
    /// stored state from before this client existed — the caller should take it
    /// as its starting point, not as something that just happened. Answering
    /// only once is what keeps the very next live transition an edge again.
    pub fn take_replayed_agent_status(&self) -> Option<Option<AgentStatus>> {
        let mut guard = self.agent_session.lock().ok()?;
        if !guard.replayed {
            return None;
        }
        guard.replayed = false;
        Some(guard.state.as_ref().map(|s| s.status))
    }

    pub fn zle_reading(&self) -> bool {
        self.zle_reading.load(Ordering::Relaxed)
    }

    /// The line the shell reported running, still percent-escaped. Empty at a
    /// prompt, and empty for a shell whose integration marks the start of a
    /// command without naming it.
    pub fn running_command(&self) -> String {
        self.running_command
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    pub fn take_submitted_commands(&self) -> Vec<(String, u64)> {
        self.submitted_commands
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    pub fn size(&self) -> TermSize {
        self.size
    }

    pub fn take_auth_prompt(&self) -> Option<(u64, AuthPromptKind)> {
        self.auth_prompts
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
    }

    pub fn take_auth_banner(&self) -> Option<String> {
        let mut q = self.auth_prompts.lock().ok()?;
        if matches!(q.front(), Some((_, AuthPromptKind::Banner { .. }))) {
            if let Some((_, AuthPromptKind::Banner { text })) = q.pop_front() {
                return Some(text);
            }
        }
        None
    }

    pub fn has_pending_auth(&self) -> bool {
        self.auth_prompts
            .lock()
            .map(|q| !q.is_empty())
            .unwrap_or(false)
    }

    pub fn ssh_phase(&self) -> Option<SshPhase> {
        self.ssh_phase.lock().ok().and_then(|g| g.clone())
    }

    pub fn ssh_endpoint(&self) -> Option<(String, u16)> {
        self.ssh_endpoint.clone()
    }

    pub fn ssh_user(&self) -> Option<String> {
        self.ssh_user.clone()
    }

    pub fn auto_supplied_password(&self) -> bool {
        self.auto_supplied_password
    }

    pub fn respond_auth(&self, request_id: u64, response: AuthResponse) {
        self.link.send(ClientMsg::AuthResponse {
            request_id,
            response,
        });
    }
}

fn daemon_not_listening(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            )
        })
    })
}

/// How long an `Attach` may stay silent before it is called unanswered.
///
/// Only a silent connection ever pays this: a dead pane answers `Error` at
/// once and a live one answers `Size` at once, so the budget is not a cost in
/// the common case. What bounds it is the caller — a local restore attaches
/// synchronously on the UI thread, one pane after another, so N silent panes
/// hold the window still for N times this. What argues for more is the other
/// side of the same silence: a daemon merely slow to serve — an execve handoff
/// keeps the listener and its backlog across the exec, and a fresh daemon is
/// adopting panes and seeding ids before it can take an `Attach` — would have
/// served this connection a moment later, and calling it unanswered spawns a
/// fresh pane over a live one, carries the live pane's history onto it and
/// starts an agent resume against a session the old process still holds. The
/// [`AttachUnanswered`] verdict is confirmed against that case in `attach_on`
/// rather than by waiting longer here.
fn attach_reply_wait(route: &PaneRoute) -> std::time::Duration {
    match route.is_local() {
        true => std::time::Duration::from_secs(2),
        false => std::time::Duration::from_secs(15),
    }
}

/// Read the daemon's answer to a spawn request, under the deadline `Attach`
/// uses for the same route.
///
/// A daemon caught mid-restart accepts the connection and then never serves
/// it. `Attach` has been guarded against that silence since #673, and core's
/// `PaneSession::spawn_over` bounds the identical exchange, but this path read
/// with no deadline at all — and the local route spawns synchronously on the
/// UI thread (`ui::app`'s `PaneRoute::Local` branch), so a daemon that went
/// quiet froze the whole window on "new tab".
///
/// The timeout is reported as a plain message with no `io::Error` in its
/// chain, which is what keeps `daemon_disconnected_before_spawn_reply` from
/// claiming it: silence is not a hangup, and retrying it would only wait
/// again. `what` names the request, since the window shows only this text.
fn spawn_reply(
    stream: &mut Stream,
    wait: std::time::Duration,
    what: &str,
) -> anyhow::Result<DaemonMsg> {
    let _ = stream.set_read_timeout(Some(wait));
    let reply = DaemonMsg::read(stream);
    let _ = stream.set_read_timeout(None);
    match reply {
        Ok(msg) => Ok(msg),
        Err(e) if would_block(&e) => {
            log::warn!("daemon request timed out; request={what} timeout={wait:?}");
            Err(anyhow::anyhow!("no answer to {what} within {wait:?}"))
        }
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("reading the daemon's answer to {what}")))
        }
    }
}

/// An `Attach` that produced no bytes within its wait — nobody served the
/// connection. Distinct from a refusal (`Error` frame) and from a hangup so
/// the caller can say the true thing: the pane may well still exist.
#[derive(Debug)]
struct AttachUnanswered {
    pane_id: u64,
    wait: std::time::Duration,
}

impl std::fmt::Display for AttachUnanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the daemon did not answer Attach for pane {} within {:?} (an attached pane replays \
             its screen at once, so nobody is serving this connection)",
            self.pane_id, self.wait
        )
    }
}

impl std::error::Error for AttachUnanswered {}

/// Whether `err` is an `Attach` that went unanswered, as opposed to refused or
/// hung up on. The pane behind an unanswered attach may still be alive.
pub fn attach_unanswered(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AttachUnanswered>().is_some())
}

/// An `Attach` the daemon answered with an `Error` frame — it heard us and
/// said no, which for a relink means the pane is gone on its machine. Typed so
/// a retry loop can tell this apart from the failures worth retrying: silence
/// and transport trouble pass, a refusal repeats identically forever.
#[derive(Debug)]
struct AttachRefused {
    message: String,
}

impl std::fmt::Display for AttachRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "daemon refused Attach: {}", self.message)
    }
}

impl std::error::Error for AttachRefused {}

/// Whether `err` is an `Attach` the daemon explicitly refused. Retrying one of
/// these can only produce the same refusal.
pub fn attach_refused(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<AttachRefused>().is_some())
}

/// Reads far enough into the daemon's answer to an `Attach` to classify it, and
/// hands back every byte read so the reader thread loses none of the replay.
///
/// Silence is a verdict, not a quiet pane. A daemon that attached replays the
/// pane's ring before it reads a byte of our input, and the ring always holds
/// a segment, so the first frame on a good attach is a `Size` — a quiet pane
/// still sends that. An `Attach` that produced nothing within `wait` is one
/// nobody is serving: a daemon still mid-restart, a socket held open by a
/// process that will never read it. Taken for success it gave the window a
/// pane drawn as restored whose every keystroke went into that socket — no
/// fresh shell, no restored-screen banner, no agent resume, and Ctrl-C did
/// nothing (#673). Taken for the failure it is, the caller spawns fresh and
/// asks for the old screen back.
fn attach_reply_prefix(
    stream: &mut Stream,
    pane_id: u64,
    wait: std::time::Duration,
) -> anyhow::Result<Vec<u8>> {
    use std::io::Read as _;

    let _ = stream.set_read_timeout(Some(wait));
    let mut buffered: Vec<u8> = Vec::new();
    let mut scratch = [0u8; 4096];
    let mut kind = None;
    while kind.is_none() {
        match stream.read(&mut scratch) {
            Ok(0) => {
                let _ = stream.set_read_timeout(None);
                return Err(anyhow::anyhow!(
                    "the daemon closed the connection without answering Attach for pane {pane_id}"
                ));
            }
            Ok(n) => buffered.extend_from_slice(&scratch[..n]),
            Err(e) if would_block(&e) => break,
            Err(e) => {
                let _ = stream.set_read_timeout(None);
                return Err(anyhow::Error::new(e).context(format!(
                    "reading the daemon's answer to Attach for pane {pane_id}"
                )));
            }
        }
        kind = anterm_core::daemon::protocol::peek_frame_kind(&buffered);
    }
    let _ = stream.set_read_timeout(None);
    if buffered.is_empty() {
        return Err(anyhow::Error::new(AttachUnanswered { pane_id, wait }));
    }
    if !kind.is_some_and(anterm_core::daemon::protocol::is_error_kind) {
        return Ok(buffered);
    }
    let message = read_error_frame(stream, &mut buffered, wait)
        .unwrap_or_else(|| format!("no such pane {pane_id}"));
    Err(anyhow::Error::new(AttachRefused { message }))
}

fn read_error_frame(
    stream: &mut Stream,
    buffered: &mut Vec<u8>,
    wait: std::time::Duration,
) -> Option<String> {
    use std::io::Read as _;

    let _ = stream.set_read_timeout(Some(wait));
    let mut scratch = [0u8; 1024];
    let message = loop {
        match anterm_core::daemon::protocol::take_frame(buffered) {
            Ok(Some(frame)) => match DaemonMsg::from_frame(frame.0, frame.1) {
                Ok(DaemonMsg::Error(message)) => break Some(message),
                _ => break None,
            },
            Ok(None) => match stream.read(&mut scratch) {
                Ok(0) => break None,
                Ok(n) => buffered.extend_from_slice(&scratch[..n]),
                Err(_) => break None,
            },
            Err(_) => break None,
        }
    };
    let _ = stream.set_read_timeout(None);
    message
}

fn would_block(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn daemon_disconnected_before_spawn_reply(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
            )
        })
    })
}

impl Drop for RemoteTerminal {
    fn drop(&mut self) {
        self.link.send(ClientMsg::Detach);
        self.stop_reader();
    }
}

fn connect() -> anyhow::Result<Stream> {
    transport::connect().map_err(|e| {
        anyhow::Error::new(e).context(format!(
            "connect to daemon at {}",
            transport::endpoint_display()
        ))
    })
}

/// Whether the local daemon answers `Version` on a fresh connection right now.
///
/// The one question a silent `Attach` leaves open — is the daemon serving and
/// this socket orphaned, or is nobody serving yet? A daemon answers `Version`
/// before it touches any state, so this is the cheapest thing it can say. One
/// second is the same budget `spawn` gives the same handshake; a healthy
/// daemon answers in microseconds.
fn local_daemon_answers() -> bool {
    use std::io::Write as _;

    let Ok(mut stream) = transport::connect() else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
    if ClientMsg::Version
        .encode(&mut stream)
        .and_then(|()| stream.flush())
        .is_err()
    {
        return false;
    }
    matches!(DaemonMsg::read(&mut stream), Ok(DaemonMsg::Version(_)))
}

fn connect_routed(route: &PaneRoute) -> anyhow::Result<Stream> {
    if let PaneRoute::Unroutable(reason) = route {
        return Err(anyhow::anyhow!("{reason}"));
    }
    let Some(header) = route.header() else {
        return connect();
    };

    anterm_core::host::guard_off_ui();

    // No `ensure_wsl_server` here on purpose. The daemon runs exactly the same
    // probe inside `router::open_link` before it opens the link, so asking from
    // this side too bought nothing and cost a second full round of `wsl.exe`
    // invocations — five of them, serially, on every single pane. On a machine
    // where a `wsl.exe` round trip is slow (issue #454 measured 3.3s) that
    // duplicate was half of the wait before a new tab could take a key.
    //
    // Nothing is lost by dropping it: the returned path was discarded, the
    // failure is reported just as well through the route ack below, and the
    // first-install consent question still reaches this process — the daemon
    // runs its probe under `RouteSetup::blocking`, which installs the relay
    // that turns the question into a frame on this very connection.
    let mut stream = connect()?;
    let ack = anterm_core::daemon::router::negotiate(&mut stream, header)
        .map_err(|e| anyhow::anyhow!("route this pane to {}: {e}", header.describe()))?;
    log::debug!(
        "pane routed to {} over {}",
        header.describe(),
        ack.link.as_deref().unwrap_or("?")
    );
    Ok(stream)
}

fn terminal_config_from_user(user_config: &anterm_ui_kit::config::Config) -> Config {
    Config {
        scrolling_history: user_config.scrollback_limit,
        default_cursor_style: alacritty_cursor_style(user_config.cursor_style),
        semantic_escape_chars: user_config.word_separators.clone(),
        kitty_keyboard: true,
        ..Config::default()
    }
}

/// A point in a batch of pty output where the reader stops advancing the
/// emulator to act on the state the bytes before it left behind.
enum ReaderCut {
    Parked(CursorCut),
    Command(CommandMark),
    Prompt(PromptMark),
}

/// How much output the reader parses per hold of the grid lock. Every UI-thread
/// handler that touches the grid — a scroll, a selection drag, a keystroke —
/// queues on this lock, and one UI thread serves every window, so a hold is how
/// long all of them can stall. Alacritty's own reader lets go at the same size
/// (`MAX_LOCKED_READ`).
const MAX_LOCKED_FEED: usize = 64 * 1024;

/// Parses `bytes` into the emulator, stopping at each of `cuts` for `on_cut` to
/// act on the state the bytes before it left behind — and letting go of the
/// grid every [`MAX_LOCKED_FEED`] bytes, so a waiting handler gets its turn
/// between slices rather than after the whole batch. A relink replays up to
/// 8 MB in one frame; parsed in one hold, that froze every window for as long
/// as it took.
///
/// The lock is fair, so re-taking it queues behind whoever arrived meanwhile.
/// vte keeps its own state across `advance` calls, so where a slice ends — mid
/// escape sequence, mid UTF-8 — makes no difference to what is parsed.
///
/// Returns how long it spent waiting for the lock, or `None` if `quit` was
/// raised while it waited: a retired reader must not write into a grid its
/// successor now owns.
#[cfg(test)]
fn feed_grid<T: EventListener>(
    term: &FairMutex<Term<T>>,
    processor: &mut ansi::Processor,
    keyboard_guard: &mut term_adapter::KeyboardStackGuard,
    bytes: &[u8],
    cuts: Vec<(usize, ReaderCut)>,
    quit: &AtomicBool,
    on_cut: impl FnMut(&mut Term<T>, &mut term_adapter::KeyboardStackGuard, ReaderCut),
) -> Option<std::time::Duration> {
    feed_grid_mirrored(
        term,
        None,
        None,
        processor,
        keyboard_guard,
        bytes,
        cuts,
        quit,
        on_cut,
    )
    .map(|(waited, _)| waited)
}

#[allow(clippy::too_many_arguments)]
fn feed_grid_mirrored<T: EventListener>(
    term: &FairMutex<Term<T>>,
    mirror: Option<&TermStateMirror>,
    listener: Option<&T>,
    processor: &mut ansi::Processor,
    keyboard_guard: &mut term_adapter::KeyboardStackGuard,
    bytes: &[u8],
    cuts: Vec<(usize, ReaderCut)>,
    quit: &AtomicBool,
    mut on_cut: impl FnMut(&mut Term<T>, &mut term_adapter::KeyboardStackGuard, ReaderCut),
) -> Option<(std::time::Duration, bool)> {
    let mut waited = std::time::Duration::ZERO;
    let mut mutated = false;
    let mut lock = || {
        let t = std::time::Instant::now();
        let grid = term.lock();
        waited += t.elapsed();
        (!quit.load(Ordering::SeqCst)).then_some(grid)
    };
    let mut grid = lock()?;
    let mut held = 0usize;
    let mut at = 0usize;
    let mut cuts = cuts.into_iter();
    let mut next = cuts.next();
    loop {
        let stop = next.as_ref().map_or(bytes.len(), |(off, _)| *off);
        while at < stop {
            if held == MAX_LOCKED_FEED {
                if mutated
                    && let Some(mirror) = mirror
                {
                    mirror.publish(&grid);
                }
                drop(grid);
                grid = lock()?;
                held = 0;
            }
            let end = stop.min(at + MAX_LOCKED_FEED - held);
            mutated |= term_adapter::advance_with_listener(
                processor,
                &mut *grid,
                keyboard_guard,
                listener,
                &bytes[at..end],
            );
            held += end - at;
            at = end;
        }
        let Some((_, cut)) = next else { break };
        on_cut(&mut grid, keyboard_guard, cut);
        mutated = true;
        next = cuts.next();
    }
    if mutated
        && let Some(mirror) = mirror
    {
        mirror.publish(&grid);
    }
    drop(grid);
    Some((waited, mutated))
}

fn alacritty_cursor_style(style: ConfigCursorStyle) -> CursorStyle {
    let shape = match style {
        ConfigCursorStyle::Block => CursorShape::Block,
        ConfigCursorStyle::Bar => CursorShape::Beam,
        ConfigCursorStyle::Underline => CursorShape::Underline,
    };
    CursorStyle {
        shape,
        blinking: false,
    }
}

fn win_size(size: TermSize, cell_w: u16, cell_h: u16) -> WinSize {
    WinSize {
        cols: size.cols as u16,
        rows: size.rows as u16,
        cell_w,
        cell_h,
    }
}
