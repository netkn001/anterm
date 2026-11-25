use std::collections::VecDeque;
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AlacEvent, EventListener as _};
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi;
use anterm_core::term_modes::{COLOR_SCHEME_UPDATES, Query, TerminalModes};

use anterm_core::osc::{Note, OscTokenizer, TitleEffect, TitleLifetime};
use anterm_core::daemon::protocol::{DaemonMsg, RemoteKind, WinSize};
use anterm_core::daemon::transport::Stream;
use crate::prompt::command_cursor::{CommandCursorStyle, CommandMark};
use crate::prompt::parked_cursor::{ParkedCursorRepair, ParkedCursorScanner};
use crate::prompt::prompt_reflow::PromptBreak;
use crate::backend::size::TermSize;

use super::term_adapter::{self, KeyboardStackGuard};
use super::{
    EventProxy, ReaderCut, ReaderSignals, RemoteTerminal, ShellState, feed_grid_mirrored,
};

pub(super) type OscNotes = Arc<Mutex<VecDeque<Note>>>;

/// Notes queued for a view that has not polled: a burst keeps only its
/// newest few, so it shows as a few rather than a spray.
pub(super) const MAX_OSC_NOTES: usize = 3;

#[cfg(test)]
pub(super) struct OscNotifyScanner {
    tok: OscTokenizer,
    notes: anterm_core::osc::Notifications,
}

#[cfg(test)]
impl Default for OscNotifyScanner {
    fn default() -> Self {
        Self {
            tok: OscTokenizer::new(&[b"9", b"99", b"777"]),
            notes: Default::default(),
        }
    }
}

#[cfg(test)]
impl OscNotifyScanner {
    pub(super) fn feed(&mut self, bytes: &[u8], out: &mut VecDeque<Note>) {
        let notes = &mut self.notes;
        self.tok.feed(bytes, |payload| {
            if let Some(note) = parse_osc_notification(notes, payload) {
                if out.len() >= MAX_OSC_NOTES {
                    out.pop_front();
                }
                out.push_back(note);
            }
        });
    }
}

enum ZleAction {
    PromptStart,
    CommandStart(String),
    Other,
}

#[derive(Default)]
struct OscScanResult {
    notes: Vec<Note>,
    zle: Vec<ZleAction>,
    retire_title: bool,
}

/// Single-pass OSC tokenizer combining command/prompt cuts (`133`), ZLE reading
/// marks (`133`), title retirement (`0`, `2`, `133`), and desktop notifications
/// (`9`, `99`, `777`). Replaces four separate byte-scanning passes per chunk.
struct OscStreamScanner {
    tok: OscTokenizer,
    notes: anterm_core::osc::Notifications,
    title_life: TitleLifetime,
}

impl Default for OscStreamScanner {
    fn default() -> Self {
        Self {
            tok: OscTokenizer::new(&[b"0", b"2", b"9", b"99", b"133", b"777"]),
            notes: Default::default(),
            title_life: TitleLifetime::default(),
        }
    }
}

impl OscStreamScanner {
    fn scan(
        &mut self,
        bytes: &[u8],
        live: bool,
        cuts: &mut Vec<(usize, ReaderCut)>,
    ) -> OscScanResult {
        let before = cuts.len();
        let mut out = OscScanResult::default();
        let notes_parser = &mut self.notes;
        let title_life = &mut self.title_life;
        self.tok.feed_at(bytes, |off, payload| {
            if let Some(mark) = payload.strip_prefix(b"133;") {
                if let Some(cmd_mark) = CommandMark::parse(payload) {
                    cuts.push((off, ReaderCut::Command(cmd_mark)));
                } else if let Some(prompt_mark) = crate::prompt::prompt_reflow::PromptMark::parse(payload) {
                    cuts.push((off, ReaderCut::Prompt(prompt_mark)));
                }
                if live {
                    match mark.first() {
                        Some(b'B') => out.zle.push(ZleAction::PromptStart),
                        Some(b'C') => {
                            let line = mark
                                .strip_prefix(b"C;")
                                .map(|c| String::from_utf8_lossy(c).into_owned())
                                .unwrap_or_default();
                            out.zle.push(ZleAction::CommandStart(line));
                        }
                        _ => out.zle.push(ZleAction::Other),
                    }
                }
                match title_life.saw(payload) {
                    TitleEffect::Retire => out.retire_title = true,
                    TitleEffect::Set => out.retire_title = false,
                    TitleEffect::None => {}
                }
                return;
            }
            if payload.starts_with(b"0;") || payload.starts_with(b"2;") {
                match title_life.saw(payload) {
                    TitleEffect::Retire => out.retire_title = true,
                    TitleEffect::Set => out.retire_title = false,
                    TitleEffect::None => {}
                }
                return;
            }
            if live && let Some(note) = parse_osc_notification(notes_parser, payload) {
                out.notes.push(note);
            }
        });
        if before > 0 && cuts.len() > before {
            cuts.sort_by_key(|(off, _)| *off);
        }
        out
    }

    fn joined_mid_command(&mut self) {
        self.title_life.joined_mid_command();
    }
}

fn apply_zle_actions(actions: Vec<ZleAction>, signals: &ReaderSignals) {
    for action in actions {
        match action {
            ZleAction::PromptStart => {
                signals.zle_reading.store(true, Ordering::Relaxed);
                if let Ok(mut cmd) = signals.running_command.lock() {
                    cmd.clear();
                }
            }
            ZleAction::CommandStart(line) => {
                signals.zle_reading.store(false, Ordering::Relaxed);
                if !line.is_empty() {
                    let seq = signals.shell.lock().map(|s| s.seq).unwrap_or(0);
                    if let Ok(mut q) = signals.submitted_commands.lock() {
                        q.push((line.clone(), seq));
                    }
                }
                if let Ok(mut cmd) = signals.running_command.lock() {
                    *cmd = line;
                }
            }
            ZleAction::Other => {
                signals.zle_reading.store(false, Ordering::Relaxed);
            }
        }
    }
}

pub(super) fn parse_osc_notification(
    notes: &mut anterm_core::osc::Notifications,
    payload: &[u8],
) -> Option<Note> {
    if anterm_core::cli_agent::parse_agent_event(payload).is_some() {
        return None;
    }
    let (title, body) = notes.parse(payload)?;
    if title.as_deref() == Some(anterm_core::cli_agent::AGENT_EVENT_SENTINEL) {
        return None;
    }
    Some((title, body))
}

pub(super) fn stale_mode_resets(mode: TermMode) -> Vec<u8> {
    let mut seq = Vec::new();
    if mode.contains(TermMode::ALT_SCREEN) {
        seq.extend_from_slice(b"\x1b[?1049l");
    }
    if !mode.contains(TermMode::SHOW_CURSOR) {
        seq.extend_from_slice(b"\x1b[?25h");
    }
    if mode.intersects(TermMode::MOUSE_MODE) {
        seq.extend_from_slice(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        seq.extend_from_slice(b"\x1b[?1006l");
    }
    if mode.contains(TermMode::UTF8_MOUSE) {
        seq.extend_from_slice(b"\x1b[?1005l");
    }
    if mode.contains(TermMode::FOCUS_IN_OUT) {
        seq.extend_from_slice(b"\x1b[?1004l");
    }
    // The kitty keyboard flags are not reset here: no sequence clears their
    // stacks, so the caller does it through `Term::reset_keyboard_modes`.
    seq
}

/// Optional throughput/latency counters printed when `ANTERM_TRACE` is set.
struct ReaderTrace {
    enabled: bool,
    last: Instant,
    bytes: u64,
    reads: u32,
    read_t: Duration,
    lock_t: Duration,
    adv_t: Duration,
    frames: u32,
}

impl ReaderTrace {
    fn from_env() -> Self {
        Self {
            enabled: std::env::var("ANTERM_TRACE").is_ok_and(|v| !v.is_empty() && v != "0"),
            last: Instant::now(),
            bytes: 0,
            reads: 0,
            read_t: Duration::ZERO,
            lock_t: Duration::ZERO,
            adv_t: Duration::ZERO,
            frames: 0,
        }
    }

    fn maybe_report(&mut self) {
        if self.enabled && self.last.elapsed() >= Duration::from_secs(1) {
            eprintln!(
                "[trace client] {:.1} MB/s | {} reads ({} B/read) {} frames | read wait {:?} lock wait {:?} advance {:?}",
                self.bytes as f64 / self.last.elapsed().as_secs_f64() / 1e6,
                self.reads,
                if self.reads > 0 {
                    self.bytes / self.reads as u64
                } else {
                    0
                },
                self.frames,
                self.read_t,
                self.lock_t,
                self.adv_t,
            );
            self.last = Instant::now();
            self.bytes = 0;
            self.reads = 0;
            self.frames = 0;
            self.read_t = Duration::ZERO;
            self.lock_t = Duration::ZERO;
            self.adv_t = Duration::ZERO;
        }
    }
}

enum TimeoutStep {
    Read(Duration),
    Continue,
    Quit,
}

struct ReaderLoop {
    term: Arc<FairMutex<Term<EventProxy>>>,
    proxy: EventProxy,
    stream: Stream,
    pending: Vec<u8>,
    quit: Arc<AtomicBool>,
    signals: ReaderSignals,
    processor: ansi::Processor,
    keyboard_guard: KeyboardStackGuard,
    /// Single-pass scanner for OSC 0/2/9/99/133/777 (command cuts, ZLE marks,
    /// title retirement, and desktop notifications).
    osc_stream: OscStreamScanner,
    // No live `Output` frame yet: whatever arrives now is the daemon's replay
    // (an attach or a relink), which it sends entirely as `Snapshot`s followed
    // by the stored state.
    replaying_state: bool,
    cursor_scan: ParkedCursorScanner,
    parked_cursor: ParkedCursorRepair,
    command_cursor: CommandCursorStyle,
    prompt_break: PromptBreak,
    // A new link replays the pane from scratch, 2031 included (`term_modes`
    // restores it ahead of the ring).
    modes: TerminalModes,
    // Set by a replay, sent by the first frame after it: a replay is a modes
    // Snapshot plus one per ring segment.
    report_scheme: bool,
    // Kitty-graphics decode runs on its own thread with newest-frame coalescing
    // (issue #213): inflating a full-window browser frame is ~42 ms, and doing
    // it inline here would block PTY output and scrolling for that long every
    // frame. The worker owns the inflate + BGRA swap + placement; the reader
    // only captures the grid anchor and hands off the still-compressed frame.
    // Dropped when the loop ends, which joins the thread.
    image_decoder: crate::render::images::DecodeWorker,
    out_batch: Vec<u8>,
    trace: ReaderTrace,
}

impl ReaderLoop {
    fn new(
        term: Arc<FairMutex<Term<EventProxy>>>,
        proxy: EventProxy,
        stream: Stream,
        buffered: Vec<u8>,
        quit: Arc<AtomicBool>,
        signals: ReaderSignals,
    ) -> Self {
        signals.color_scheme_updates.store(false, Ordering::Relaxed);
        let image_decoder = {
            let proxy = proxy.clone();
            crate::render::images::DecodeWorker::spawn(signals.images.clone(), move || {
                proxy.send_event(AlacEvent::Wakeup);
            })
        };
        Self {
            term,
            proxy,
            stream,
            pending: buffered,
            quit,
            signals,
            processor: ansi::Processor::new(),
            keyboard_guard: KeyboardStackGuard::default(),
            osc_stream: OscStreamScanner::default(),
            replaying_state: true,
            cursor_scan: ParkedCursorScanner::new(),
            parked_cursor: ParkedCursorRepair::default(),
            command_cursor: CommandCursorStyle::default(),
            prompt_break: PromptBreak::default(),
            modes: TerminalModes::new(),
            report_scheme: false,
            image_decoder,
            out_batch: Vec::new(),
            trace: ReaderTrace::from_env(),
        }
    }

    fn run(mut self) {
        anterm_core::threads::promote_to_user_interactive();
        let mut scratch = vec![0u8; 256 * 1024];

        loop {
            if !self.drain_frames() {
                return;
            }
            if !self.flush_batch() {
                return;
            }

            let timeout = match self.next_read_timeout() {
                TimeoutStep::Read(timeout) => timeout,
                TimeoutStep::Continue => continue,
                TimeoutStep::Quit => return,
            };
            let _ = self.stream.set_read_timeout(Some(timeout));
            self.trace.maybe_report();

            let tr0 = self.trace.enabled.then(Instant::now);
            match self.stream.read(&mut scratch) {
                Ok(0) => {
                    self.teardown(Some("the far end closed it (EOF)"));
                    return;
                }
                Ok(n) => {
                    if let Some(tr0) = tr0 {
                        self.trace.read_t += tr0.elapsed();
                        self.trace.reads += 1;
                        self.trace.bytes += n as u64;
                    }
                    self.pending.extend_from_slice(&scratch[..n]);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.teardown(Some(&format!("read failed: {e}")));
                    return;
                }
            }
        }
    }

    fn teardown(&self, reason: Option<&str>) {
        // A retired reader exits silently: the pane is being relinked or
        // released, not dying — marking it exited would wrongly kill the
        // freshly adopted link.
        if self.quit.load(Ordering::SeqCst) {
            return;
        }
        // Said out loud because every way a link dies looks the same from the
        // window — a pane suddenly "disconnected" — and this line is the only
        // record of which way it was.
        if let Some(reason) = reason {
            log::warn!("a pane's link died: {reason}");
        }
        self.term.lock().exit();
        self.signals.exited.store(true, Ordering::SeqCst);
        self.proxy.send_event(AlacEvent::Wakeup);
        self.proxy.send_event(AlacEvent::Exit);
    }

    fn next_read_timeout(&mut self) -> TimeoutStep {
        // The read always times out within QUIT_POLL so a reader parked on a
        // silent link still notices `quit` promptly — on Windows, `shutdown()`
        // alone would never wake it.
        const QUIT_POLL: Duration = Duration::from_millis(500);
        match self.processor.sync_timeout().sync_timeout() {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    let mut term = self.term.lock();
                    if self.quit.load(Ordering::SeqCst) {
                        return TimeoutStep::Quit;
                    }
                    let flushed = term_adapter::stop_sync_with_listener(
                        &mut self.processor,
                        &mut *term,
                        &mut self.keyboard_guard,
                        Some(&self.proxy),
                    );
                    if flushed {
                        self.signals.mirror.publish(&term);
                    }
                    drop(term);
                    if flushed {
                        self.proxy.send_event(AlacEvent::Wakeup);
                    }
                    return TimeoutStep::Continue;
                }
                TimeoutStep::Read(left.min(QUIT_POLL))
            }
            None => TimeoutStep::Read(QUIT_POLL),
        }
    }

    /// Parses any complete frames currently buffered in `self.pending`.
    /// Returns `false` if the reader should exit.
    fn drain_frames(&mut self) -> bool {
        loop {
            // Checked per frame, not just per read: a retired reader may still
            // hold complete frames in `pending`, and every arm below writes
            // shared state that outlives a relink (cwd, prompt, agent — and
            // `Exited`, which would close the freshly adopted pane via
            // `child_exited`).
            if self.quit.load(Ordering::SeqCst) {
                return false;
            }
            let frame = match anterm_core::daemon::protocol::take_frame(&mut self.pending) {
                Ok(Some(frame)) => frame,
                Ok(None) => return true,
                Err(e) => {
                    self.teardown(Some(&format!("unframeable bytes on the link: {e}")));
                    return false;
                }
            };
            let msg = match DaemonMsg::from_frame(frame.0, frame.1) {
                Ok(msg) => msg,
                Err(e) => {
                    self.teardown(Some(&format!("undecodable frame on the link: {e}")));
                    return false;
                }
            };
            // Once per replay (a modes Snapshot, then one per ring segment), at
            // the first frame past the ring — not when what was read runs out:
            // an 8 MiB ring takes many reads, and a read that ends between the
            // segment holding a dead program's `?2031h` and the one holding the
            // prompt that ended it would report into the shell. A live Output
            // reports once folded.
            if !matches!(
                msg,
                DaemonMsg::Size(_) | DaemonMsg::Snapshot(_) | DaemonMsg::Output(_)
            ) && std::mem::take(&mut self.report_scheme)
                && self.modes.is_on(COLOR_SCHEME_UPDATES)
            {
                self.proxy
                    .send_event(crate::render::color_scheme::query_reply());
            }
            if !self.handle_msg(msg) {
                return false;
            }
        }
    }

    fn flush_batch(&mut self) -> bool {
        if self.out_batch.is_empty() {
            return true;
        }
        // The scanner reports an offset one past each sequence it matched, in
        // ascending order, so the batch splits at each of them: advance the
        // emulator to the cut, act on the state that sequence left behind,
        // carry on.
        let mut cuts: Vec<(usize, ReaderCut)> = Vec::new();
        if self.signals.local_conpty.load(Ordering::Relaxed) {
            self.cursor_scan
                .feed(&self.out_batch, |off, c| cuts.push((off, ReaderCut::Parked(c))));
        }
        let scan = self.osc_stream.scan(&self.out_batch, true, &mut cuts);
        let mutated = {
            let t0 = self.trace.enabled.then(Instant::now);
            let parked_cursor = &mut self.parked_cursor;
            let command_cursor = &mut self.command_cursor;
            let prompt_break = &mut self.prompt_break;
            let Some((waited, mutated)) = feed_grid_mirrored(
                &self.term,
                Some(&self.signals.mirror),
                Some(&self.proxy),
                &mut self.processor,
                &mut self.keyboard_guard,
                &self.out_batch,
                cuts,
                &self.quit,
                |term, _guard, cut| match cut {
                    ReaderCut::Parked(cut) => parked_cursor.apply(term, cut),
                    ReaderCut::Command(mark) => command_cursor.apply(term, mark),
                    ReaderCut::Prompt(mark) => prompt_break.apply(term, mark),
                },
            ) else {
                return false;
            };
            if let Some(t0) = t0 {
                self.trace.lock_t += waited;
                self.trace.adv_t += t0.elapsed() - waited;
            }
            mutated
        };
        let has_notes = !scan.notes.is_empty();
        if has_notes && let Ok(mut notes) = self.signals.osc_notes.lock() {
            for note in scan.notes {
                if notes.len() >= MAX_OSC_NOTES {
                    notes.pop_front();
                }
                notes.push_back(note);
            }
        }
        let has_zle = !scan.zle.is_empty();
        if has_zle {
            apply_zle_actions(scan.zle, &self.signals);
        }
        if scan.retire_title {
            self.proxy.send_event(AlacEvent::ResetTitle);
        }
        // DEC ?2026 silence: while an application holds a synchronized update
        // open across multiple socket reads, `vte::Processor` buffers the bytes
        // without touching `Term` (`mutated == false`). Suppressing `Wakeup`
        // until `?2026l` (or the 150 ms deadline) avoids redundant full-window
        // repaints of an unchanged grid.
        if mutated || has_notes || has_zle || scan.retire_title {
            self.proxy.send_event(AlacEvent::Wakeup);
        }
        self.out_batch.clear();
        true
    }

    fn handle_msg(&mut self, msg: DaemonMsg) -> bool {
        match msg {
            // Geometry, applied at this exact stream position. During replay
            // each ring segment is preceded by its Size; live, the daemon
            // echoes one when it applies our Resize (FEATURE_RESIZE_ECHO),
            // which is the point in the stream where the bytes stop being
            // old-width — everything still queued before this frame must be
            // parsed into the old grid.
            DaemonMsg::Size(ws) => self.handle_size(ws),
            DaemonMsg::Snapshot(bytes) => self.handle_snapshot(bytes),
            DaemonMsg::Output(bytes) => {
                self.handle_output(bytes);
                true
            }
            // Kitty graphics (issue #213): the daemon lifted an image out of
            // the stream and forwarded it out-of-band, interleaved *in stream
            // order* with the Output frames around it. Flush the pending text
            // first so the grid cursor sits where the sender drew the image,
            // then anchor the placement to that cell in scroll-stable
            // absolute-row coordinates (`history_size - display_offset +
            // cursor_line`), so it tracks scrolling.
            DaemonMsg::Image(frame) => self.handle_image(frame),
            // An `a=d` delete, lifted out the same way. Order with the
            // surrounding output does not matter for a delete (it targets by
            // id/placement, not cursor position), but flushing keeps a
            // delete-then-retransmit in the same read from racing its own
            // replacement.
            DaemonMsg::DeleteImage(sel) => {
                if !self.flush_batch() {
                    return false;
                }
                if let Some(del) = anterm_core::kitty_graphics::ImageDelete::decode(&sel) {
                    self.image_decoder.delete(&del);
                    self.proxy.send_event(AlacEvent::Wakeup);
                }
                true
            }
            DaemonMsg::ClipboardWrite(frame) => self.handle_clipboard_write(frame),
            DaemonMsg::Cwd(path) => {
                if !self.flush_batch() {
                    return false;
                }
                if let Ok(mut guard) = self.signals.cwd.lock() {
                    *guard = Some(path);
                }
                true
            }
            DaemonMsg::Prompt {
                active,
                at_prompt,
                last_exit,
            } => self.handle_prompt(active, at_prompt, last_exit),
            DaemonMsg::RemoteContext(ctx) => {
                if !self.flush_batch() {
                    return false;
                }
                if let Ok(mut guard) = self.signals.cwd.lock() {
                    *guard = None;
                }
                // A native-SSH pane's pty is the far host's, however local the
                // daemon that dialled it: the daemon bridges an ssh channel
                // straight through and opens no ConPTY of its own. The route
                // cannot say so — such a pane is spawned and attached through
                // the local daemon like any other — so this frame is where a
                // client that reopened onto an existing one finds out. A
                // one-way latch: the far end of an ssh channel never becomes a
                // local pty later, while an `ssh` *command* (RemoteKind::Ssh)
                // is a program inside a ConPTY and keeps the repair.
                if ctx
                    .as_ref()
                    .is_some_and(|c| c.kind == RemoteKind::NativeSsh)
                {
                    self.signals.local_conpty.store(false, Ordering::Relaxed);
                }
                if let Ok(mut guard) = self.signals.remote.lock() {
                    *guard = ctx;
                }
                true
            }
            DaemonMsg::AuthPrompt { request_id, prompt } => {
                if !self.flush_batch() {
                    return false;
                }
                if let Ok(mut guard) = self.signals.auth.lock() {
                    guard.push_back((request_id, prompt));
                }
                self.proxy.send_event(AlacEvent::Wakeup);
                true
            }
            DaemonMsg::SshStatus { phase: p } => {
                if !self.flush_batch() {
                    return false;
                }
                if let Ok(mut guard) = self.signals.phase.lock() {
                    *guard = Some(p);
                }
                self.proxy.send_event(AlacEvent::Wakeup);
                true
            }
            DaemonMsg::Agent(a) => {
                if !self.flush_batch() {
                    return false;
                }
                if let Ok(mut guard) = self.signals.agent.lock() {
                    *guard = a;
                }
                true
            }
            DaemonMsg::AgentStatus(state) => {
                if !self.flush_batch() {
                    return false;
                }
                if let Ok(mut guard) = self.signals.agent_session.lock() {
                    guard.state = state;
                    // The first such frame on an attached link is the pane's
                    // stored status being replayed, not a turn changing state
                    // now.
                    guard.replayed = std::mem::take(&mut self.signals.awaiting_replay);
                }
                self.proxy.send_event(AlacEvent::Wakeup);
                true
            }
            DaemonMsg::Lease(by) => {
                if let Ok(mut guard) = self.signals.lease.lock() {
                    *guard = by;
                }
                self.proxy.send_event(AlacEvent::Wakeup);
                true
            }
            DaemonMsg::Exited { .. } => {
                if !self.flush_batch() {
                    return false;
                }
                self.signals.child_exited.store(true, Ordering::SeqCst);
                // Routine — the child ended — so no log line.
                self.teardown(None);
                false
            }
            _ => true,
        }
    }

    fn handle_size(&mut self, ws: WinSize) -> bool {
        if !self.flush_batch() {
            return false;
        }
        let mode = if self.signals.local_conpty.load(Ordering::Relaxed) {
            crate::prompt::prompt_reflow::ResizeMode::Conpty
        } else {
            // A live echo lands just before the shell's SIGWINCH redraw; a replayed
            // Size heads old bytes that nothing is going to repaint.
            let shell_redraws = !self.replaying_state
                && self
                    .signals
                    .shell
                    .lock()
                    .is_ok_and(|s| s.active && s.at_prompt);
            crate::prompt::prompt_reflow::ResizeMode::Standard { shell_redraws }
        };
        {
            let mut term = self.term.lock();
            if self.quit.load(Ordering::SeqCst) {
                return false;
            }
            crate::prompt::prompt_reflow::resize(
                &mut term,
                TermSize::new(ws.cols as usize, ws.rows as usize),
                mode,
            );
            self.signals.mirror.publish(&term);
        }
        self.proxy.send_event(AlacEvent::Wakeup);
        true
    }

    fn handle_snapshot(&mut self, bytes: Vec<u8>) -> bool {
        if !self.flush_batch() {
            return false;
        }
        self.cursor_scan.reset();
        self.parked_cursor.reset();
        self.proxy.replaying.store(true, Ordering::Relaxed);
        // The replay carries the command marks too, so a pane reattached after
        // `nvim` exited in it comes back with the prompt's cursor.
        let mut cuts: Vec<(usize, ReaderCut)> = Vec::new();
        let scan = self.osc_stream.scan(&bytes, false, &mut cuts);
        // Each prompt mark in it also clears the kitty keyboard stacks, at the
        // mark itself. Live, that happens on the `Prompt` report that follows
        // it; a replay carries one report for the whole ring, so flags a dead
        // program left before an earlier prompt would otherwise come back to
        // whatever runs in the pane now (#1155). The daemon's `TerminalModes`
        // clears them at the same marks, so the prelude ahead of the ring
        // agrees.
        let command_cursor = &mut self.command_cursor;
        let prompt_break = &mut self.prompt_break;
        let fed = feed_grid_mirrored(
            &self.term,
            Some(&self.signals.mirror),
            Some(&self.proxy),
            &mut self.processor,
            &mut self.keyboard_guard,
            &bytes,
            cuts,
            &self.quit,
            |term, guard, cut| match cut {
                ReaderCut::Command(mark) => {
                    if mark == CommandMark::Finished {
                        term_adapter::reset_keyboard_modes(term, guard);
                    }
                    command_cursor.apply(term, mark)
                }
                ReaderCut::Prompt(mark) => {
                    term_adapter::reset_keyboard_modes(term, guard);
                    prompt_break.apply(term, mark)
                }
                ReaderCut::Parked(_) => {}
            },
        );
        if fed.is_none() {
            return false;
        }
        if self.processor.sync_timeout().sync_timeout().is_some() {
            let mut term = self.term.lock();
            if self.quit.load(Ordering::SeqCst) {
                return false;
            }
            term_adapter::stop_sync_with_listener(
                &mut self.processor,
                &mut *term,
                &mut self.keyboard_guard,
                Some(&self.proxy),
            );
            self.signals.mirror.publish(&term);
        }
        self.proxy.replaying.store(false, Ordering::Relaxed);
        // A replayed `?996n` was answered long ago, but the theme may have
        // flipped while the pane was detached, so say what it is now.
        crate::render::color_scheme::fold(
            &mut self.modes,
            &bytes,
            &self.signals.color_scheme_updates,
        );
        self.modes.take_queries();
        self.report_scheme = true;
        // The replay carries the pane's recent marks, so reading it is what
        // lets a reattached window know whether the title it just adopted
        // belongs to anything still running.
        if scan.retire_title {
            self.proxy.send_event(AlacEvent::ResetTitle);
        }
        self.proxy.send_event(AlacEvent::Wakeup);
        true
    }

    fn handle_output(&mut self, bytes: Vec<u8>) {
        // Live output only ever follows the whole replay (the daemon sends the
        // stored status last, and the stream keeps that order), so the first
        // frame here ends the window in which a status can still be a replayed
        // one. Without this, a pane that had no agent session to replay would
        // keep the window open until some agent it ran *later* reported for the
        // first time, and that report would be discounted.
        self.signals.awaiting_replay = false;
        self.replaying_state = false;
        crate::render::color_scheme::fold(
            &mut self.modes,
            &bytes,
            &self.signals.color_scheme_updates,
        );
        if std::mem::take(&mut self.report_scheme) && self.modes.is_on(COLOR_SCHEME_UPDATES) {
            self.proxy
                .send_event(crate::render::color_scheme::query_reply());
        }
        for query in self.modes.take_queries() {
            if query == Query::ColorScheme {
                self.proxy
                    .send_event(crate::render::color_scheme::query_reply());
            }
        }
        self.out_batch.extend_from_slice(&bytes);
        self.trace.frames += 1;
    }

    fn handle_image(&mut self, frame: Vec<u8>) -> bool {
        if !self.flush_batch() {
            return false;
        }
        if let Some(img) = anterm_core::kitty_graphics::Image::decode_frame_owned(frame) {
            // Capture the anchor *now*, at the cursor cell the transmission
            // arrived on; the decode is deferred to the worker thread but must
            // land at this position, not wherever the cursor has scrolled to by
            // the time inflate finishes.
            let (anchor_row, anchor_col) = {
                let term = self.term.lock();
                if self.quit.load(Ordering::SeqCst) {
                    return false;
                }
                let grid = term.grid();
                let row = grid.history_size() as i64 - grid.display_offset() as i64
                    + i64::from(grid.cursor.point.line.0);
                (row, grid.cursor.point.column.0)
            };
            // Hand off without blocking the reader: the worker inflates, swaps,
            // places, and wakes the view. Stale frames coalesce away there.
            self.image_decoder
                .submit(crate::render::images::PendingFrame {
                    img,
                    anchor_row,
                    anchor_col,
                });
        }
        true
    }

    fn handle_clipboard_write(&mut self, frame: Vec<u8>) -> bool {
        if !self.flush_batch() {
            return false;
        }
        if let Some(write) = anterm_core::clipboard::ClipboardWrite::decode_frame(frame) {
            if self
                .signals
                .clipboard_write_busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if let Ok(mut pending) = self.signals.clipboard_writes.lock() {
                    pending.push_back(write);
                    self.proxy.send_event(AlacEvent::Wakeup);
                } else {
                    self.signals
                        .clipboard_write_busy
                        .store(false, Ordering::Release);
                }
            } else {
                let reply =
                    anterm_core::clipboard::response(write.id.as_deref(), "EBUSY");
                self.proxy.send_event(AlacEvent::PtyWrite(
                    String::from_utf8(reply).expect("OSC 5522 replies are ASCII"),
                ));
            }
        } else {
            self.signals
                .clipboard_write_busy
                .store(false, Ordering::Release);
        }
        true
    }

    fn handle_prompt(&mut self, active: bool, at_prompt: bool, last_exit: Option<i32>) -> bool {
        if !self.flush_batch() {
            return false;
        }
        // The replay says a command owns the pane right now. If the ring it
        // replayed had already rolled past that command's `C`, the title just
        // adopted from it is the command's and its `D` has to retire it (#889).
        if self.replaying_state && active && !at_prompt {
            self.osc_stream.joined_mid_command();
        }
        if let Ok(mut guard) = self.signals.shell.lock() {
            *guard = ShellState {
                active,
                at_prompt,
                last_exit,
                seq: guard.seq + 1,
                cycle: guard.cycle + u64::from(at_prompt && !guard.at_prompt),
            };
        }
        if active && at_prompt {
            let mut term = self.term.lock();
            if self.quit.load(Ordering::SeqCst) {
                return false;
            }
            let resets = stale_mode_resets(*term.mode());
            if !resets.is_empty() {
                term_adapter::advance(&mut self.processor, &mut *term, &mut self.keyboard_guard, &resets);
            }
            // Nothing in front of the shell could have negotiated kitty
            // keyboard flags, so they all go — both screens' stacks, not just
            // the flags in effect. `CSI = 0 u` cannot do that: a program killed
            // on the alternate screen left its push on that screen's stack, and
            // the next program to enter it got the flags back and its Escape as
            // `CSI 27 u` — which a ConPTY reader sees as no key at all (#1155).
            term_adapter::reset_keyboard_modes(&mut term, &mut self.keyboard_guard);
            self.signals.mirror.publish(&term);
            drop(term);
            if !resets.is_empty() {
                self.proxy.send_event(AlacEvent::Wakeup);
            }
        }
        true
    }
}

impl RemoteTerminal {
    pub(super) fn spawn_reader(
        term: Arc<FairMutex<Term<EventProxy>>>,
        proxy: EventProxy,
        read_half: Stream,
        buffered: Vec<u8>,
        quit: Arc<AtomicBool>,
        signals: ReaderSignals,
    ) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("anterm-remote-reader".to_string())
            .spawn(move || {
                ReaderLoop::new(term, proxy, read_half, buffered, quit, signals).run();
            })
            .expect("spawn remote reader thread")
    }
}
