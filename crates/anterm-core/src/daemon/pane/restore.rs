use std::io::Write;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::term_modes::Query;
use crate::daemon::protocol::{DaemonMsg, ReplayMode, WinSize};

use super::PaneState;
use super::fanout::gated_len;

/// The screen a restored pane opens with.
///
/// `segments` is what some earlier pane — the one this one replaces after a
/// daemon died without handing off — last had on it. `banner` is the sentence
/// that says so, supplied by the client because the daemon has no locale of its
/// own. Both are decoration over a shell that is unambiguously new: nothing
/// here revives a process.
pub struct Restore {
    pub segments: Vec<crate::daemon::scrollback::Segment>,
    pub banner: Option<String>,
    /// The pane's last OSC title, carried beside the bytes rather than in them:
    /// the OSC that set it was usually emitted screens ago and trimmed out of
    /// the capped snapshot, so replaying the segments alone brings the screen
    /// back under the default name.
    pub title: Option<String>,
}

/// The OSC that puts a restored pane's title back.
///
/// BEL-terminated rather than ST so the sequence contains no ESC of its own,
/// and the title is stripped of control bytes — a BEL or ESC inside it would
/// end the sequence early and leak the rest into the terminal as input.
pub fn retitle(title: &str) -> Vec<u8> {
    let mut out = b"\x1b]0;".to_vec();
    out.extend(
        title
            .chars()
            .filter(|c| !c.is_control())
            .collect::<String>()
            .as_bytes(),
    );
    out.push(0x07);
    out
}

/// The bytes that separate restored output from the new shell's own.
///
/// The resets are not cosmetic. A snapshot is trimmed at the front, so it can
/// begin in the middle of anything: an SGR run whose reset was cut, a hidden
/// cursor, a disabled autowrap, an alternate screen whose `?1049h` survived but
/// whose `?1049l` never came because the daemon died while `vim` was open.
/// Replaying that leaves the emulator in a state the incoming shell did not ask
/// for and cannot see. Leaving the alternate screen also does the useful thing
/// in the common case: the primary buffer still holds the pre-`vim` scrollback
/// from earlier in the same snapshot.
///
/// The same argument reaches further than the four resets it started with, and
/// [`INPUT_MODE_RESETS`] is the rest of it: a snapshot that ends inside a
/// full-screen program replays that program's `?1002h` and nothing to undo it,
/// and the emulator then reports every pointer move into a shell that never
/// asked (#850).
///
/// On Windows it ends by scrolling the restored screen out of the viewport
/// ([`SCROLL_RESTORED_AWAY`]), which is a correctness requirement rather than a
/// matter of taste — see that constant.
pub fn restore_preamble(banner: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    // Reset alternate screen, visible cursor, autowrap, SGR, scroll margins
    // (DECSTBM) and origin mode (DECOM) so a trimmed snapshot from a TUI app
    // cannot confine subsequent cursor positioning (including `\x1b[H`).
    out.extend_from_slice(b"\x1b[?1049l\x1b[?25h\x1b[?7h\x1b[0m\x1b[r\x1b[?6l");
    out.extend_from_slice(INPUT_MODE_RESETS);
    if let Some(banner) = banner.map(str::trim).filter(|b| !b.is_empty()) {
        out.extend_from_slice(b"\r\n\x1b[2m\xe2\x94\x80\xe2\x94\x80 ");
        // A newline inside the banner would be a client writing multiple lines
        // through a one-line hole; the rest of the sequence assumes one line.
        out.extend_from_slice(banner.replace(['\r', '\n'], " ").as_bytes());
        out.extend_from_slice(b" \xe2\x94\x80\xe2\x94\x80\x1b[0m\r\n");
    }
    if cfg!(windows) {
        out.extend_from_slice(SCROLL_RESTORED_AWAY);
    }
    out
}

/// Turn off every mode that makes a terminal *send* bytes on its own, or encode
/// keys in a way the incoming shell cannot read.
///
/// A restored pane is a snapshot of a dead process replayed at a brand-new
/// shell. The process that switched these on was killed by a hangup with no
/// grace period, so it never emitted its own `l` counterparts, and the snapshot
/// was photographed before it could have anyway. What is left is a byte stream
/// whose last word on mouse reporting is "on", replayed verbatim into an
/// emulator that obeys it: the pointer crossing the pane types SGR reports into
/// the shell's line, and the line grows for as long as the pointer is there
/// (#850).
///
/// Unconditional on purpose. The state to land in is not "whatever the dead
/// program had" but a constant — the new shell asked for none of these, and it
/// is not running yet, so there is nothing here to preserve. Switching off a
/// mode that is already off is a no-op in every emulator, which makes the blunt
/// version the one that cannot fail: a fold over the snapshot's bytes that
/// misreads one sequence leaves the mode on and the bug exactly as it is today.
///
/// - `9`/`1000`/`1002`/`1003` are the mouse reporting level, `1005`/`1006`/
///   `1015`/`1016` its encodings. Our own emulator knows `1000`, `1002`, `1003`,
///   `1005` and `1006` and ignores the rest; `9`, `1015` and `1016` are here for
///   the terminals downstream of a CLI consumer replaying the same ring, which
///   do know them.
/// - `1004` is focus reporting — the other mode that makes the terminal write
///   into the pty unprompted, on every window activation. `2031` is the same
///   for a theme change.
/// - `2004` is bracketed paste: a paste into a shell that does not know the
///   protocol arrives with `ESC[200~` typed around it.
/// - `1` is DECCKM, which sends the arrow keys as `ESC O A` instead of `ESC[A`.
/// - `CSI = 0 ; 1 u` sets the kitty keyboard flags in effect to none. It
///   leaves their stacks alone — no sequence clears those — so a pop could
///   still bring a dead program's flags back; the new shell's first prompt
///   mark is what clears the stacks (`Term::reset_keyboard_modes` in the
///   client, `TerminalModes` here).
///
/// Deliberately absent: `1007` (alternate scroll), which this emulator has *on*
/// by default, so clearing it would walk away from the default rather than back
/// to it; and `2026` (synchronised update), which the client's processor closes
/// out itself the moment a replayed frame ends inside one.
pub const INPUT_MODE_RESETS: &[u8] = b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\
\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l\x1b[?1004l\x1b[?2031l\x1b[?2004l\x1b[?1l\x1b[=0;1u";

/// Push the restored screen into the client's scrollback and put the cursor
/// back at the top-left, so the incoming shell starts on a blank viewport.
///
/// A pty on unix hands the terminal a stream; a ConPTY hands it a *rendering of
/// a screen buffer it owns*. That buffer starts blank with its cursor at the
/// top-left, and conhost addresses it absolutely: PSReadLine redrawing the line
/// being typed emits `ESC[6;20H`, meaning row 6 of conhost's buffer, and every
/// frame conhost paints is positioned the same way. Those row numbers are only
/// correct if the client's viewport is conhost's buffer, row for row.
///
/// Restored output breaks exactly that. It is output conhost never produced and
/// knows nothing about, so leaving it on screen puts the shell's first prompt
/// some rows below where conhost believes it is, and the first keystroke
/// repaints the input line *over the restored text* — the prompt stops
/// responding and the old screen fills with fragments of what is being typed.
/// Nothing the client can do fixes that after the fact: the offset is not a
/// constant (the screen scrolls) and it would have to be unpicked from every
/// absolute address in the stream.
///
/// So the restored screen goes where it can be kept without claiming a row:
/// `ESC[2J` on the primary screen scrolls the viewport into history rather than
/// erasing it, so it is a scroll away, and `ESC[H` leaves the cursor where a
/// fresh ConPTY expects to find it.
///
/// Not done on unix, where the shell positions itself relatively and the
/// restored screen can simply stay where the user can see it.
pub const SCROLL_RESTORED_AWAY: &[u8] = b"\x1b[2J\x1b[H";

/// Everything the pane remembers about a chunk of output: the bytes
/// themselves, and the modes they switched on. One function so the two can
/// never drift apart — the modes are only worth anything if they were folded
/// from exactly the bytes the ring was given.
pub(super) fn record_output(st: &mut PaneState, bytes: &[u8]) {
    st.ring.append(bytes);
    st.modes.feed(bytes);
}

/// A pane with no client attached has nobody to answer its queries, and a
/// replay drops replies, so one asked then is never answered. Programs wait on
/// these: Claude Code ends its OSC 11 theme check with DA1 and hangs on it,
/// skipping every later check, so the theme flips anterm reports never reach it.
///
/// Returns the queries the daemon owes an answer to. With a client attached
/// the client answers, so none are owed. Read under the state lock; the
/// answers are written by [`answer_queries`] after it is released, because a
/// pty whose program is not reading its input blocks the write, and nothing
/// else that needs this pane's state should wait on that.
pub(super) fn detached_queries(st: &mut PaneState) -> Vec<Query> {
    let asked = st.modes.take_queries();
    if st.subscriber.is_some() {
        Vec::new()
    } else {
        asked
    }
}

/// What a client's emulator would have answered `query` with, `dark` being
/// the theme the window last drew.
pub(super) fn detached_reply(query: Query, dark: bool) -> &'static [u8] {
    match query {
        // alacritty_terminal's `identify_terminal`: a VT102.
        Query::DeviceAttributes => b"\x1b[?6c",
        Query::ColorScheme if dark => b"\x1b[?997;1n",
        Query::ColorScheme => b"\x1b[?997;2n",
    }
}

pub(super) fn answer_queries(writer: &Mutex<Box<dyn Write + Send>>, owed: &[Query]) {
    if owed.is_empty() {
        return;
    }
    let dark = owed.contains(&Query::ColorScheme) && crate::machine::appearance().dark;
    if let Ok(mut writer) = writer.lock() {
        for &query in owed {
            let _ = writer.write_all(detached_reply(query, dark));
        }
        let _ = writer.flush();
    }
}

/// Everything a client needs to rebuild the pane's screen and status, in the
/// order it has to be applied.
///
/// `foreground_command` is the answer to "does a program other than the shell
/// own the terminal right now?", asked of the pty rather than of the pane's
/// stored state. It gates the prompt report, and that gate is not cosmetic —
/// see [`replayed_at_prompt`].
pub(super) fn replay_state(st: &PaneState, subscriber: &Sender<DaemonMsg>, foreground_command: bool) -> usize {
    // What the replay queues is charged to the stream's gate like live output,
    // by the same measure its writer credits back (`gated_len`).
    let mut queued = 0;
    let mut send = |msg: DaemonMsg| {
        let len = gated_len(&msg);
        if subscriber.send(msg).is_ok() {
            queued += len;
        }
    };
    // Ahead of the ring, not after it: a client that is put into the alternate
    // screen first paints the replayed frames into the buffer they belong to.
    //
    // Only the modes the ring cannot switch on itself, though. Re-entering an
    // alternate screen the ring still carries is not a harmless duplicate —
    // the emulator makes `?1049h` a no-op when the mode is already on, so the
    // ring's own copy stops clearing the alternate screen, and everything the
    // ring holds from *before* that sequence (the shell scrollback the user
    // had behind the program) is painted into the alternate buffer, which has
    // no history to keep it and is left behind when the program exits. What
    // the ring carries always wins on its own terms.
    if let Some(modes) = st
        .modes
        .replay_prelude(&st.ring.head_modes, &st.ring.modes())
    {
        send(DaemonMsg::Snapshot(modes));
    }
    st.ring.replay(&mut send);
    send_replayed_metadata(st, foreground_command, &mut send);
    queued
}

/// Like [`replay_state`], but starts with a [`DaemonMsg::ReplayCursor`] frame
/// and emits only the bytes appended after `since_offset` when still in the
/// ring, falling back to a full replay (`ReplayMode::Full`) otherwise.
pub(super) fn replay_state_since(
    st: &PaneState,
    subscriber: &Sender<DaemonMsg>,
    foreground_command: bool,
    since_offset: Option<u64>,
) -> usize {
    let mut queued = 0;
    let mut send = |msg: DaemonMsg| {
        let len = gated_len(&msg);
        if subscriber.send(msg).is_ok() {
            queued += len;
        }
    };
    let cursor = st.ring.appended;
    if let Some(since) = since_offset
        && st.ring.can_replay_since(since)
    {
        send(DaemonMsg::ReplayCursor {
            offset: cursor,
            mode: ReplayMode::Delta,
        });
        st.ring.replay_since(since, &mut send);
        send_replayed_metadata(st, foreground_command, &mut send);
        return queued;
    }

    send(DaemonMsg::ReplayCursor {
        offset: cursor,
        mode: ReplayMode::Full,
    });
    if let Some(modes) = st
        .modes
        .replay_prelude(&st.ring.head_modes, &st.ring.modes())
    {
        send(DaemonMsg::Snapshot(modes));
    }
    st.ring.replay(&mut send);
    send_replayed_metadata(st, foreground_command, &mut send);
    queued
}

fn send_replayed_metadata(
    st: &PaneState,
    foreground_command: bool,
    send: &mut impl FnMut(DaemonMsg),
) {
    if st.shell.active {
        send(DaemonMsg::Prompt {
            active: st.shell.active,
            at_prompt: replayed_at_prompt(st, foreground_command),
            last_exit: st.shell.last_exit_code,
        });
    }
    if st.remote.is_some() {
        send(DaemonMsg::RemoteContext(st.remote.clone()));
    }
    // After the remote context, never before it. A client drops its cwd on
    // every `RemoteContext`, because live that frame means the pane just hopped
    // and the old directory belongs to the other side. Replayed, it is only
    // the standing context, and `st.cwd` is already the far shell's own report
    // (`apply_remote_context` clears it on every hop). Sent first, it would be
    // wiped, and a native SSH pane reopened by a client would have no remote
    // directory for ⌘T or a split to start the new pane in until its next
    // prompt.
    if let Some(cwd) = &st.cwd {
        send(DaemonMsg::Cwd(cwd.clone()));
    }
    if let Some(phase) = &st.ssh_phase {
        send(DaemonMsg::SshStatus {
            phase: phase.clone(),
        });
    }
    if st.agent.is_some() {
        send(DaemonMsg::Agent(st.agent));
    }
    if st.agent_session.is_some() {
        send(DaemonMsg::AgentStatus(st.agent_session.clone()));
    }
    if !st.alive {
        send(DaemonMsg::Exited { code: st.exit_code });
    }
}

/// Whether a replay may tell the client the pane is sitting at a shell prompt.
///
/// `st.shell.at_prompt` is only ever as fresh as the last OSC 133 mark the pane
/// produced, and a mark can stay the last word for hours: a shell that printed
/// its prompt (`133;B`) and then handed the terminal to a full-screen program
/// which emits no `133;C` of its own leaves the flag set for as long as that
/// program runs. The live path already declines to believe such a mark — the
/// reader drops `at_prompt` from any prompt mark that arrives while a
/// foreground command owns the pty — but the replay re-asserted the stored
/// value with no check at all.
///
/// That gap is what #711 is. A client that hears "at a prompt" scrubs the TUI
/// modes it finds in its grid, the alternate screen included, on the reasoning
/// that no full-screen program can own a pane whose shell is prompting. On a
/// replay the alternate screen it finds is the one the ring *just rebuilt*, so
/// the scrub swaps it away and leaves the primary screen underneath: the shell
/// banner and the command line the pane was born with. The program is still
/// there, still painting differential updates, now into a grid that no longer
/// holds what those updates are differences from — which is why the pane came
/// back as a few fragments on an empty screen, and why only a resize (a real
/// `SIGWINCH`, a real repaint) put it back.
///
/// Asking the pty closes the gap without costing the scrub its purpose: when
/// the shell really is prompting, no command is in the foreground, the report
/// goes out unchanged, and a genuinely stranded alt screen (an `ssh` that died
/// mid-`vim`) still heals on reattach.
pub(super) fn replayed_at_prompt(st: &PaneState, foreground_command: bool) -> bool {
    st.shell.at_prompt && !foreground_command
}

/// How long the pty stays at the transient size of a [`redraw_nudge`]. Two
/// size changes in one breath reach a program as one `SIGWINCH` (the kernel
/// does not queue them), and a program that reads the size only once it gets
/// around to the signal finds it unchanged and skips the repaint. Long enough
/// for it to see the first size, short enough that nobody looks at it.
pub(super) const REDRAW_NUDGE_SETTLE: Duration = Duration::from_millis(50);

/// A pane reattached again this soon after a nudge is not nudged again: the
/// repaint the last one asked for is already in the ring, or on its way.
pub(super) const REDRAW_NUDGE_COOLDOWN: Duration = Duration::from_secs(2);

/// Whether a client that was just handed the replay needs the program on the
/// pane to repaint, and if so the size to put the pty at for a moment to make
/// it (#1122). Records the nudge, so a burst of reattaches makes one.
///
/// A full-screen program paints its whole screen once, after `?1049h`, and
/// from then on only what changed. The ring keeps the last 8 MiB, so a program
/// that has run long enough has had that first paint, and its `?1049h`, pushed
/// out of the front. The replay prelude still puts the client into the
/// alternate screen, but what follows is a stream of differences from a screen
/// the client never saw, painted onto an empty one. The client has nothing to
/// rebuild it from, and neither has the daemon, which keeps no screen; only
/// the program can draw it again, and the one request every full-screen
/// program answers with a full repaint is a change of size. Asking for the
/// size it already has does nothing — the kernel sends no `SIGWINCH` for it —
/// so the pty steps one row away and back.
///
/// Only when all of it holds: the pane is in the alternate screen and the ring
/// no longer carries the switch into it (which is to say the ring has dropped
/// its front, and with it the first paint), a program owns the pane, and the
/// pane is alive. Anything else — a shell, a program whose start is still in
/// the ring — replays whole, and is left alone.
pub(super) fn redraw_nudge(
    st: &mut PaneState,
    foreground_program: bool,
    now: std::time::Instant,
) -> Option<WinSize> {
    use crate::term_modes::ALT_SCREEN;
    if !st.alive || !foreground_program || !st.modes.is_on(ALT_SCREEN) {
        return None;
    }
    if st
        .last_redraw_nudge
        .is_some_and(|at| now.saturating_duration_since(at) < REDRAW_NUDGE_COOLDOWN)
    {
        return None;
    }
    // Last, for it folds the whole ring.
    if st.ring.modes().is_on(ALT_SCREEN) {
        return None;
    }
    st.last_redraw_nudge = Some(now);
    let size = st.ring.size();
    let rows = if size.rows > 1 {
        size.rows - 1
    } else {
        size.rows + 1
    };
    Some(WinSize { rows, ..size })
}

/// The two steps of a nudge: `transient` now, and after `settle` whatever
/// `settled` says the pane's size is by then (`None`: leave it, the pane is
/// gone). The second step runs on its own thread so the attach that asked for
/// it is not held up.
pub(super) fn run_redraw_nudge(
    transient: WinSize,
    settle: Duration,
    resize: Box<dyn Fn(WinSize) + Send>,
    settled: impl FnOnce() -> Option<WinSize> + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    resize(transient);
    std::thread::Builder::new()
        .name("anterm-redraw-nudge".into())
        .spawn(move || {
            std::thread::sleep(settle);
            if let Some(size) = settled() {
                resize(size);
            }
        })
}
