use super::*;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use gpui::{ClipboardEntry, KeyDownEvent, Modifiers, ScrollDelta, ScrollEvent, WeakEntity};

use anterm_ui_kit::actions::{DecreaseFontSize, IncreaseFontSize};
use anterm_ui_kit::config::MouseZoomModifier;
use crate::backend::hold::Verdict;
use crate::backend::typeahead::RawInput;

#[derive(Clone, Copy)]
pub(super) struct DragScroll {
    pub(super) overshoot: f32,
    pub(super) col: usize,
    pub(super) side: Side,
}

/// In-flight wheel animation. `remaining` is what is left to scroll, in lines,
/// relative to wherever the view happens to be — deliberately not an absolute
/// target, so output arriving mid-animation shifts the grid under us without
/// dragging the animation somewhere else.
#[derive(Clone, Copy)]
pub(super) struct ScrollAnim {
    pub(super) remaining: f32,
    pub(super) last: std::time::Instant,
}

/// Fraction of the remaining distance consumed per [`SCROLL_ANIM_FRAME`].
const SCROLL_ANIM_SMOOTH: f32 = 0.4;

/// The frame `SCROLL_ANIM_SMOOTH` is calibrated against: one nominal 60 Hz
/// frame. Decay is scaled by real elapsed time, so the same feel holds at any
/// refresh rate.
pub(super) const SCROLL_ANIM_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Below this much left to travel, land instead of asymptoting toward it. A
/// twentieth of a line is around a pixel — the tail of an exponential decay is
/// invisible long before it ends, and every frame of it costs a full repaint.
const SCROLL_ANIM_MIN: f32 = 0.05;

/// A jump smaller than this reads as continuous already; spreading it would
/// only put lag between the hand and the grid. Inching a wheel one detent at a
/// time lands here, and so does every event a trackpad sends.
const SCROLL_ANIM_MIN_JUMP: f32 = 1.0;

/// How long a trackpad gesture stays "live" after its last event. Long enough
/// to bridge the gaps in a momentum tail, short enough that reaching for the
/// wheel right after a swipe is not mistaken for more of the swipe.
const SCROLL_GESTURE_IDLE: std::time::Duration = std::time::Duration::from_millis(150);

/// How far a trackpad has to travel, in lines, to earn one font-size step while
/// the platform modifier is held. A wheel detent is one step on its own, so
/// this only ever applies to the continuous stream a gesture produces.
const ZOOM_SCROLL_LINES: f32 = 3.0;

pub(super) enum CmdKey {
    Consumed,
    Bubble,
    FallThrough,
}

#[derive(Debug, PartialEq)]
pub(super) enum WheelRoute {
    Report { base: u8 },
    Arrows { seq: &'static [u8] },
    Scrollback,
}

pub(super) fn wheel_route(mode: TermMode, shift: bool, up: bool) -> WheelRoute {
    if !shift && mode.intersects(TermMode::MOUSE_MODE) {
        return WheelRoute::Report {
            base: if up { 64 } else { 65 },
        };
    }
    if !shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let seq: &'static [u8] = match (up, mode.contains(TermMode::APP_CURSOR)) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1b[A",
            (false, true) => b"\x1bOB",
            (false, false) => b"\x1b[B",
        };
        return WheelRoute::Arrows { seq };
    }
    WheelRoute::Scrollback
}

#[derive(Debug, PartialEq)]
pub(super) enum SelectEndCopy {
    None,
    Grid,
    Editor,
}

/// Whether a right-click belongs to anterm's own context menu rather than to the
/// terminal application.
///
/// A TUI that has turned mouse reporting on (vim with `set mouse=a`, lazygit,
/// tmux, …) draws its own right-button menus, so popping the host menu on top
/// of the application's is a double delivery of one click — #251. While
/// reporting is active the unmodified click is the application's alone; Shift
/// is the escape hatch that reaches anterm, matching how Shift already overrides
/// mouse reporting for selection (`register_mouse_handlers`) and for the wheel
/// (`wheel_route`).
///
/// This is the exact complement of the "forward the press to the app" branch in
/// `TerminalElement::register_mouse_handlers`, which is why both call it: one
/// click must never feed both consumers.
pub(crate) fn should_show_context_menu(mouse_mode: bool, shift: bool) -> bool {
    !mouse_mode || shift
}

pub(super) fn select_end_copy(enabled: bool, grid: bool, editor: bool) -> SelectEndCopy {
    match (enabled, grid, editor) {
        (false, ..) => SelectEndCopy::None,
        (true, true, _) => SelectEndCopy::Grid,
        (true, false, true) => SelectEndCopy::Editor,
        (true, false, false) => SelectEndCopy::None,
    }
}

pub(super) fn encode_mouse(
    sgr: bool,
    base: u8,
    mods: &Modifiers,
    col: usize,
    row: usize,
    pressed: bool,
) -> Option<Vec<u8>> {
    let mut mod_bits = 0u8;
    if mods.shift {
        mod_bits += 4;
    }
    if mods.alt {
        mod_bits += 8;
    }
    if mods.control {
        mod_bits += 16;
    }

    if sgr {
        let c = if pressed { 'M' } else { 'm' };
        let msg = format!("\x1b[<{};{};{}{}", base + mod_bits, col + 1, row + 1, c);
        Some(msg.into_bytes())
    } else {
        if col >= 223 || row >= 223 {
            return None;
        }
        let code = if pressed {
            base + mod_bits
        } else {
            3 + mod_bits
        };
        Some(vec![
            0x1b,
            b'[',
            b'M',
            32 + code,
            (32 + 1 + col) as u8,
            (32 + 1 + row) as u8,
        ])
    }
}

pub(super) fn focus_report_bytes(mode: TermMode, focused: bool) -> Option<&'static [u8]> {
    if !mode.contains(TermMode::FOCUS_IN_OUT) {
        return None;
    }
    Some(if focused { b"\x1b[I" } else { b"\x1b[O" })
}

/// Whether this wheel event is a zoom rather than a scroll.
///
/// Exactly one modifier, and it has to be the configured one: anything
/// alongside it is somebody else's gesture — shift in particular is the escape
/// hatch that scrolls the scrollback out from under a mouse-reporting program.
///
/// Off macOS the platform modifier *is* Ctrl, so `Platform` and `Ctrl` describe
/// the same key there; the setting still round-trips, so a config file shared
/// with a Mac keeps meaning what it meant.
pub(super) fn zoom_wheel(modifier: MouseZoomModifier, mods: &Modifiers) -> bool {
    if mods.number_of_modifiers() != 1 {
        return false;
    }
    match modifier {
        MouseZoomModifier::Platform => anterm_ui_kit::platform::is_secondary(mods),
        MouseZoomModifier::Ctrl => mods.control,
        MouseZoomModifier::Alt => mods.alt,
        MouseZoomModifier::None => false,
    }
}

/// How many font-size steps a zoom event is worth, and what is left over for
/// the next one.
///
/// A wheel detent is a discrete click of intent, so it is one step whatever the
/// platform says it covers — macOS calls a single notch five lines, and five
/// points of font per notch would be unusable. A trackpad has no detents and
/// spends a flick over dozens of events, so those accumulate and only pay out
/// once the fingers have travelled [`ZOOM_SCROLL_LINES`].
pub(super) fn zoom_scroll_steps(lines: f32, debt: f32, gesturing: bool) -> (i32, f32) {
    if !gesturing {
        let step = if lines > 0. {
            1
        } else if lines < 0. {
            -1
        } else {
            0
        };
        return (step, 0.);
    }
    let total = debt + lines;
    let steps = (total / ZOOM_SCROLL_LINES).trunc();
    (steps as i32, total - steps * ZOOM_SCROLL_LINES)
}

pub(super) fn smooth_scroll_step(offset: usize, frac: f32, delta: f32, max: usize) -> (i32, f32) {
    let pos = (offset as f32 + frac + delta).clamp(0., max as f32);
    let new_offset = pos.floor();
    (new_offset as i32 - offset as i32, pos - new_offset)
}

/// Advance the in-flight scroll animation once per presented frame.
///
/// Registered from [`TerminalView::queue_scroll_anim`]: gpui runs the callback
/// immediately before the next frame is drawn, and it re-registers itself as
/// long as the animation lives. Stepping at frame time instead of on a
/// free-running timer keeps the decay steps locked to the monitor's vblank
/// cadence — a 16 ms timer drifting against a 16.67 ms vblank makes some
/// presented frames show twice the movement of their neighbours and others
/// none, which reads as stutter. Frame-aligned stepping shows every frame
/// exactly the ground it covered. The weak handle and the epoch guard make the
/// chain die quietly when the pane is closed or the animation is cancelled.
fn schedule_scroll_anim_frame(view: WeakEntity<TerminalView>, epoch: u64, window: &mut Window) {
    window.on_next_frame(move |window, cx| {
        let Some(view) = view.upgrade() else {
            return;
        };
        if view.update(cx, |view, cx| view.scroll_anim_frame(epoch, cx)) {
            schedule_scroll_anim_frame(view.downgrade(), epoch, window);
        }
    });
}

/// How much of `remaining` to consume this frame, and whether this is the last
/// one. Decay is scaled by the real elapsed time so a dropped frame covers the
/// ground it missed instead of stretching the animation out.
pub(super) fn scroll_anim_step(remaining: f32, dt: std::time::Duration) -> (f32, bool) {
    if remaining.abs() <= SCROLL_ANIM_MIN {
        return (remaining, true);
    }
    let frames = (dt.as_secs_f32() / SCROLL_ANIM_FRAME.as_secs_f32()).clamp(0.1, 8.);
    let consumed = 1. - (1. - SCROLL_ANIM_SMOOTH).powf(frames);
    let step = remaining * consumed;
    if (remaining - step).abs() <= SCROLL_ANIM_MIN {
        (remaining, true)
    } else {
        (step, false)
    }
}

pub(super) fn drag_scroll_step(overshoot: f32) -> i32 {
    let lines = overshoot.abs().ceil().clamp(1., 8.) as i32;
    if overshoot < 0. { -lines } else { lines }
}

impl TerminalView {
    pub(super) fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal.exited {
            return;
        }
        if self.prompt.integration_notice.take().is_some() {
            cx.notify();
        }
        // The remote-listing failure pill goes away on the next keystroke
        // too — by then the user has seen it (#585).
        if self.prompt.remote_completion_notice.take().is_some() {
            cx.notify();
        }
        let reshaped = if cfg!(target_os = "macos") {
            crate::input::reshape_option_keystroke(
                &ev.keystroke,
                cx.global::<Config>().macos_option_as_alt,
            )
        } else {
            None
        };
        let ks = reshaped.as_ref().unwrap_or(&ev.keystroke);
        let m = &ks.modifiers;

        if self.composer_state.focused {
            if ks.key == "escape" {
                self.composer_escape(window, cx);
                cx.stop_propagation();
            } else if ks.key == "c" && m.control && !m.platform && !m.alt && !m.shift {
                self.composer_interrupt(window, cx);
                cx.stop_propagation();
            }
            return;
        }

        if self.search_state.active.is_some() && self.search_state.focused {
            if ks.key == "escape" {
                self.close_search(window, cx);
                cx.stop_propagation();
            }
            return;
        }

        if m.platform && !m.control && !m.alt {
            match self.handle_cmd_shortcut(ks, window, cx) {
                CmdKey::Consumed => {
                    cx.stop_propagation();
                    return;
                }
                CmdKey::Bubble => return,
                CmdKey::FallThrough => {}
            }
        }

        // Ctrl+V is not here: off macOS it is the `AlternatePaste` binding,
        // which the keymap withholds on the alternate screen so a full-screen
        // program gets its SYN, and which the user can retire outright. Copy
        // and cut stay, because both answer a selection this view owns and
        // fall through to the PTY when there is none.
        //
        // Ctrl+Shift+C/X are the keymap's alone: rebinding Copy has to retire
        // Ctrl+Shift+C, which it cannot if this path answers it too.
        if cfg!(not(target_os = "macos"))
            && m.control
            && !m.platform
            && !m.alt
            && !m.shift
            && matches!(ks.key.as_str(), "c" | "x")
        {
            match self.handle_cmd_shortcut(ks, window, cx) {
                CmdKey::Consumed => {
                    cx.stop_propagation();
                    return;
                }
                CmdKey::Bubble | CmdKey::FallThrough => {}
            }
        }

        if !self.accepts_input(cx) {
            return;
        }

        #[cfg(target_os = "macos")]
        if !window.has_pending_keystrokes() && crate::input::defer_to_ime(ks, self.key_flags()) {
            return;
        }

        if self.input_active() {
            self.handle_editor_key(ks, cx);
            cx.stop_propagation();
            return;
        }

        // Ctrl-R landing on the PTY is only worth a notice when the user still
        // expects anterm's menu. With the prompt editor off, the shell owning
        // Ctrl-R is exactly what was asked for.
        if m.control
            && !m.platform
            && !m.alt
            && ks.key == "r"
            && self.prompt.prompt_editor
            && cx.global::<Config>().history_search
        {
            self.note_integration_gap(cx);
        }

        if m.control && !m.platform && !m.alt && ks.key == "c" && self.handoff_active() {
            // A Tab/unknown-chord handoff leaves the daemon at_prompt: the shell
            // never saw Enter, so there is no C mark. Ctrl-C makes readline draw
            // a fresh prompt whose A/B report is consequently true -> true and
            // does not advance prompt_cycle. Remember this report boundary so
            // that fresh prompt can still return ownership to the local editor.
            self.prompt.editor_handoff_interrupt_seq = Some(self.terminal.prompt_seq());
        }

        let kitty = self.key_flags();
        if let Some(bytes) = crate::input::keystroke_to_bytes(ks, kitty) {
            let plain = !m.control && !m.alt && !m.platform;
            let boundary = typeahead_boundary(ks.key.as_str(), m);
            let interrupt = boundary.is_some();
            let shell_owns_prompt = self.shell_owns_prompt();
            let held = plain
                && ks.key == "backspace"
                && !shell_owns_prompt
                && self.gap_holdable()
                && match self.prompt.hold.hold_backspace(&bytes) {
                    Verdict::Held(arm) => {
                        if let Some(epoch) = arm {
                            self.arm_hold_timer(epoch, cx);
                        }
                        true
                    }
                    Verdict::Passthrough => false,
                };
            if !held {
                self.release_hold();
                if let Some(boundary) = boundary.filter(|_| !shell_owns_prompt) {
                    // Ctrl-C interrupts and Ctrl-D can close the foreground reader.
                    // Discard the gap before sending either so a prompt transition
                    // cannot turn the pending record into a later Ctrl-U.
                    self.observe_typeahead(boundary);
                }
                self.terminal.write(bytes);
                if !shell_owns_prompt && !interrupt {
                    self.observe_typeahead(RawInput::Key {
                        key: ks.key.as_str(),
                        plain,
                    });
                }
            }
            self.render.cursor_visible = true;
            self.jump_to_prompt();
            cx.notify();
            cx.stop_propagation();
        }
    }

    pub(super) fn send_shortcut_bytes(&mut self, bytes: &[u8], key: &str, cx: &mut Context<Self>) {
        let shell_owns_prompt = self.shell_owns_prompt();
        self.release_hold();
        self.send_to_pty(bytes, cx);
        if !shell_owns_prompt {
            let alt = self.on_alt_screen();
            self.prompt.typeahead
                .observe(RawInput::Key { key, plain: false }, alt);
        }
    }

    pub(super) fn handle_cmd_shortcut(
        &mut self,
        ks: &gpui::Keystroke,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> CmdKey {
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "c" => {
                if self.copy_contextual(m.control, false, cx) {
                    CmdKey::Consumed
                } else {
                    CmdKey::FallThrough
                }
            }
            "x" => {
                if self.cut_contextual(cx) {
                    CmdKey::Consumed
                } else {
                    CmdKey::FallThrough
                }
            }
            // Cmd+V only: macOS leaves `PasteText` unbound and pastes from
            // here, and Cmd carries no control code to lose. The Ctrl+V half
            // of this key lives in the keymap as `AlternatePaste`.
            "v" => {
                self.paste_from_clipboard(cx);
                CmdKey::Consumed
            }
            "a" => {
                self.select_all_contextual(cx);
                CmdKey::Consumed
            }
            "z" => {
                self.undo_edit(m.shift, cx);
                CmdKey::Consumed
            }
            "left" => {
                if self.input_active() {
                    self.editor_move_edge(false, m.shift);
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x01], "a", cx);
                }
                CmdKey::Consumed
            }
            "right" => {
                if self.input_active() {
                    self.editor_move_edge(true, m.shift);
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x05], "e", cx);
                }
                CmdKey::Consumed
            }
            "backspace" => {
                if self.input_active() {
                    if !self.prompt.cmd.delete_selection() {
                        self.prompt.cmd.delete_to_start();
                    }
                    self.reset_history_nav();
                    self.close_completion();
                    self.render.cursor_visible = true;
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x15], "u", cx);
                }
                CmdKey::Consumed
            }
            "delete" => {
                if self.input_active() {
                    if !self.prompt.cmd.delete_selection() {
                        self.prompt.cmd.delete_to_end();
                    }
                    self.reset_history_nav();
                    self.close_completion();
                    self.render.cursor_visible = true;
                    cx.notify();
                } else if cfg!(target_os = "macos") && self.accepts_input(cx) {
                    self.send_shortcut_bytes(&[0x0b], "k", cx);
                }
                CmdKey::Consumed
            }
            _ => CmdKey::Bubble,
        }
    }

    pub(super) fn handle_editor_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        self.render.cursor_visible = true;
        self.jump_to_prompt();
        self.engage_hold_into_editor();
        self.adopt_typeahead();

        let aliased;
        let ks = if m.control && !m.platform && !m.alt && matches!(key, "p" | "n") {
            aliased = gpui::Keystroke {
                modifiers: gpui::Modifiers::default(),
                key: if key == "p" { "up" } else { "down" }.to_string(),
                key_char: None,
            };
            &aliased
        } else {
            ks
        };
        let m = &ks.modifiers;
        let key = ks.key.as_str();

        if key != "up" && key != "down" {
            self.prompt.editor_goal_col = None;
        }
        if !(m.alt && key == ".") {
            self.prompt.last_word_nav = None;
        }

        if self.prompt.reverse_search.is_some() {
            self.handle_reverse_search_key(ks, cx);
            return;
        }

        if m.control && !m.platform && !m.alt && matches!(key, "j" | "m") {
            self.accept_line(cx);
            return;
        }

        if self.prompt.completion.is_some() && !m.control && !m.alt {
            match (m.platform, key) {
                (false, "up") => {
                    self.completion_select(false, cx);
                    return;
                }
                (false, "down") => {
                    self.completion_select(true, cx);
                    return;
                }
                (false, "enter") => {
                    self.accept_line(cx);
                    return;
                }
                (true, "enter") => {
                    self.completion_accept(cx);
                    self.submit_command(cx);
                    return;
                }
                (false, "escape") => {
                    self.close_completion();
                    cx.notify();
                    return;
                }
                (false, "backspace") if self.prompt.cmd.selection().is_none() && !self.prompt.cmd.is_empty() => {
                    self.prompt.cmd.backspace();
                    self.reset_history_nav();
                    self.completion_refilter();
                    self.render.cursor_visible = true;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        self.close_completion();

        if self.prompt.helix_mode {
            if self.prompt.cmd.helix.mode == anterm_editor::helix::HelixMode::Insert {
                if !m.control && !m.alt && !m.platform && key == "escape" {
                    self.prompt.cmd.helix.mode = anterm_editor::helix::HelixMode::Normal;
                    self.prompt.cmd.helix.clear_pending();
                    self.prompt.cmd.sync_to_helix();
                    cx.notify();
                    return;
                }
            } else {
                self.handle_helix_editor_key(ks, cx);
                return;
            }
        }

        // A function key means nothing to this editor and everything to the
        // shell — PSReadLine puts CharacterSearch on F3 and HistorySearch on
        // F8, and both of those act on the line that is currently on the
        // prompt. So it takes the same route an unknown Ctrl chord takes:
        // hand the line over first, then send the key, with every modifier
        // combination going the same way (Alt+F7 is ClearHistory).
        if crate::input::is_function_key(key) && !m.platform {
            if let Some(bytes) = crate::input::keystroke_to_bytes(ks, self.key_flags()) {
                self.handoff_line_to_shell(&bytes, cx);
                return;
            }
        }

        if m.control && !m.platform && !m.alt {
            if cfg!(not(target_os = "macos")) {
                match key {
                    "left" => {
                        self.editor_move_h(false, m.shift, true);
                        cx.notify();
                        return;
                    }
                    "right" => {
                        self.editor_move_h(true, m.shift, true);
                        cx.notify();
                        return;
                    }
                    "backspace" => {
                        if !self.prompt.cmd.delete_selection() {
                            self.prompt.cmd.delete_word_left();
                        }
                        self.reset_history_nav();
                        cx.notify();
                        return;
                    }
                    "delete" => {
                        if !self.prompt.cmd.delete_selection() {
                            self.prompt.cmd.delete_word_right();
                        }
                        self.reset_history_nav();
                        cx.notify();
                        return;
                    }
                    _ => {}
                }
            }
            if cfg!(not(target_os = "macos")) && key == "a" {
                self.prompt.cmd.select_all();
                self.close_completion();
                self.render.cursor_visible = true;
                cx.notify();
                return;
            }
            if key == "r" {
                if !cx.global::<Config>().history_search {
                    self.handoff_line_to_shell(&[0x12], cx);
                } else {
                    self.sync_shared_history(cx);
                    self.start_reverse_search(cx);
                    cx.notify();
                }
                return;
            }
            if self.apply_readline_ctrl(key, cx) {
                cx.notify();
            } else if let Some(bytes) = crate::input::keystroke_to_bytes(ks, self.key_flags()) {
                self.handoff_line_to_shell(&bytes, cx);
            } else {
                cx.notify();
            }
            return;
        }

        if m.alt && !m.platform && !m.control {
            match key {
                "." => {
                    self.insert_last_word(cx);
                    return;
                }
                "b" => {
                    self.editor_move_h(false, m.shift, true);
                    cx.notify();
                    return;
                }
                "f" => {
                    self.editor_move_h(true, m.shift, true);
                    cx.notify();
                    return;
                }
                "d" => {
                    if !self.prompt.cmd.delete_selection() {
                        self.prompt.cmd.delete_word_right();
                    }
                    self.reset_history_nav();
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        match key {
            "enter" => {
                self.submit_command(cx);
                return;
            }
            "backspace" => {
                if self.prompt.cmd.is_empty() {
                    self.terminal.write(vec![0x7f]);
                    self.observe_typeahead(RawInput::Key {
                        key: "backspace",
                        plain: true,
                    });
                    return;
                }
                if m.alt && self.prompt.cmd.selection().is_none() {
                    self.prompt.cmd.delete_word_left();
                } else {
                    self.prompt.cmd.backspace();
                }
                self.reset_history_nav();
            }
            "delete" => {
                if m.alt {
                    self.prompt.cmd.delete_word_right();
                } else {
                    self.prompt.cmd.delete();
                }
                self.reset_history_nav();
            }
            "left" => self.editor_move_h(false, m.shift, m.alt),
            "right" => {
                if !m.shift && self.prompt.cmd.selection().is_none() {
                    if let Some(full) = self.ghost_suggestion(cx) {
                        self.prompt.cmd.set(&full);
                        self.reset_history_nav();
                        cx.notify();
                        return;
                    }
                }
                self.editor_move_h(true, m.shift, m.alt);
            }
            "home" => self.editor_move_edge(false, m.shift),
            "end" => self.editor_move_edge(true, m.shift),
            "up" => {
                if self.editor_move_v(false, m.shift) {
                    cx.notify();
                } else {
                    self.history_prev(cx);
                }
                return;
            }
            "down" => {
                if self.editor_move_v(true, m.shift) {
                    cx.notify();
                } else {
                    self.history_next(cx);
                }
                return;
            }
            "escape" => {
                let bytes = crate::input::keystroke_to_bytes(ks, self.key_flags())
                    .unwrap_or_else(|| vec![0x1b]);
                self.terminal.write(bytes);
                return;
            }
            _ => {
                if !m.control && !m.platform && !m.alt {
                    if let Some(ch) = ks.key_char.as_deref() {
                        if !ch.is_empty() && ch.chars().all(|c| c >= '\u{20}' && c != '\u{7f}') {
                            self.commit_text(ch, cx);
                            return;
                        }
                    }
                }
                if m.alt && !m.control && !m.platform && key.chars().count() == 1 {
                    let bytes = crate::input::keystroke_to_bytes(ks, self.key_flags())
                        .unwrap_or_else(|| {
                            let name = if m.shift {
                                key.to_uppercase()
                            } else {
                                key.to_string()
                            };
                            let mut b = vec![0x1b];
                            b.extend_from_slice(name.as_bytes());
                            b
                        });
                    self.handoff_line_to_shell(&bytes, cx);
                    return;
                }
            }
        }
        cx.notify();
    }

    fn handle_helix_editor_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        use anterm_editor::helix::{HelixOutcome, HostAction, SurfaceKind};

        let m = &ks.modifiers;
        let key = ks.key.as_str();
        if self.prompt.cmd.helix.minor.is_none() && !m.control && !m.alt && !m.platform && !m.shift {
            if (key == "k" || key == "up") && self.prompt.cmd.is_on_first_line() {
                self.history_prev(cx);
                self.prompt.cmd.sync_to_helix();
                return;
            }
            if (key == "j" || key == "down") && self.prompt.cmd.is_on_last_line() {
                self.history_next(cx);
                self.prompt.cmd.sync_to_helix();
                return;
            }
        }

        let text = self.prompt.cmd.text();
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let outcome = self.prompt.cmd.helix.handle_keystroke(
            anterm_ui_kit::helix_bridge::key_input(ks),
            &text,
            SurfaceKind::InlinePrompt,
            clipboard.as_deref(),
        );

        match outcome {
            HelixOutcome::Ignored => {}
            HelixOutcome::Handled => {
                self.prompt.cmd.sync_from_helix();
                cx.notify();
            }
            HelixOutcome::Yank { clipboard } => {
                if let Some(yanked) = clipboard {
                    cx.write_to_clipboard(ClipboardItem::new_string(yanked));
                }
                self.prompt.cmd.sync_from_helix();
                cx.notify();
            }
            HelixOutcome::Edit {
                range,
                new_text,
                new_anchor,
                new_head,
                yank_to_clipboard,
            } => {
                if let Some(yanked) = yank_to_clipboard {
                    cx.write_to_clipboard(ClipboardItem::new_string(yanked));
                }
                self.prompt.cmd
                    .replace_byte_range(range, &new_text, new_anchor, new_head);
                self.reset_history_nav();
                cx.notify();
            }
            HelixOutcome::Host(action) => match action {
                HostAction::Submit => self.submit_command(cx),
                HostAction::Undo => {
                    self.prompt.cmd.undo();
                    self.prompt.cmd.sync_to_helix();
                    self.reset_history_nav();
                    cx.notify();
                }
                HostAction::Redo => {
                    self.prompt.cmd.redo();
                    self.prompt.cmd.sync_to_helix();
                    self.reset_history_nav();
                    cx.notify();
                }
                HostAction::OpenSearchBar | HostAction::GlobalSearch => {
                    self.sync_shared_history(cx);
                    self.start_reverse_search(cx);
                    cx.notify();
                }
                _ => {
                    self.prompt.cmd.sync_from_helix();
                    if self.prompt.cmd.text() != text {
                        self.reset_history_nav();
                    }
                    cx.notify();
                }
            },
        }
    }

    fn apply_readline_ctrl(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "r" => self.start_reverse_search(cx),
            "a" => {
                self.prompt.cmd.clear_selection();
                self.prompt.cmd.move_home();
            }
            "e" => {
                if self.prompt.cmd.selection().is_none()
                    && let Some(full) = self.ghost_suggestion(cx)
                {
                    self.prompt.cmd.set(&full);
                    self.reset_history_nav();
                } else {
                    self.prompt.cmd.clear_selection();
                    self.prompt.cmd.move_end();
                }
            }
            "b" => {
                self.prompt.cmd.clear_selection();
                self.prompt.cmd.move_left();
            }
            "f" => {
                if let Some(full) = self.ghost_suggestion(cx) {
                    self.prompt.cmd.set(&full);
                    self.reset_history_nav();
                } else {
                    self.prompt.cmd.clear_selection();
                    self.prompt.cmd.move_right();
                }
            }
            "w" => {
                if !self.prompt.cmd.delete_selection() {
                    self.prompt.cmd.delete_path_component_left();
                }
                self.reset_history_nav();
            }
            "u" => {
                if !self.prompt.cmd.delete_selection() {
                    self.prompt.cmd.delete_to_start();
                }
                self.reset_history_nav();
            }
            "k" => {
                if !self.prompt.cmd.delete_selection() {
                    self.prompt.cmd.delete_to_end();
                }
                self.reset_history_nav();
            }
            "h" => {
                self.prompt.cmd.backspace();
                self.reset_history_nav();
            }
            "y" => {
                self.prompt.cmd.yank();
                self.reset_history_nav();
            }
            "l" => {
                self.terminal.write(vec![0x0c]);
            }
            "c" => {
                self.prompt.cmd.clear();
                self.reset_history_nav();
                let _ = self.prompt.typeahead.drain();
                let _ = self.prompt.hold.engage();
                self.terminal.write(vec![0x03]);
            }
            "d" => {
                if self.prompt.cmd.is_empty() {
                    self.wipe_pending_typeahead();
                    self.terminal.write(vec![0x04]);
                } else {
                    self.prompt.cmd.delete();
                    self.reset_history_nav();
                }
            }
            _ => return false,
        }
        true
    }

    fn editor_move_h(&mut self, right: bool, shift: bool, word: bool) {
        if shift {
            self.prompt.cmd.begin_selection();
        } else if let Some((s, e)) = self.prompt.cmd.selection() {
            self.prompt.cmd.set_cursor(if right { e } else { s });
            self.prompt.cmd.clear_selection();
            return;
        }
        match (right, word) {
            (false, false) => self.prompt.cmd.move_left(),
            (false, true) => self.prompt.cmd.move_word_left(),
            (true, false) => self.prompt.cmd.move_right(),
            (true, true) => self.prompt.cmd.move_word_right(),
        }
    }

    fn editor_move_edge(&mut self, end: bool, shift: bool) {
        if shift {
            self.prompt.cmd.begin_selection();
        } else {
            self.prompt.cmd.clear_selection();
        }
        if end {
            self.prompt.cmd.move_end();
        } else {
            self.prompt.cmd.move_home();
        }
    }

    fn editor_move_v(&mut self, down: bool, shift: bool) -> bool {
        let Some((_, scol)) = self.cursor_cell() else {
            return false;
        };
        let cols = self.terminal.term.lock().columns().max(1);
        let chars: Vec<char> = self.prompt.cmd.text().chars().collect();
        let len = chars.len();
        let start = input_start(scol, cols);
        let (positions, _r, _c) = input_char_positions(&chars, start, cols);
        let end_caret = if len == 0 {
            start
        } else {
            let (r, c, w) = positions[len - 1];
            if chars[len - 1] == '\n' {
                (r + 1, 0)
            } else {
                (r, c + w)
            }
        };
        let (cur_row, cur_col) = if self.prompt.cmd.cursor() < len {
            let (r, c, _) = positions[self.prompt.cmd.cursor()];
            (r, c)
        } else {
            end_caret
        };
        let mut max_row = positions
            .iter()
            .map(|&(r, _, _)| r)
            .max()
            .unwrap_or(start.0);
        if chars.last() == Some(&'\n') {
            max_row += 1;
        }
        if (down && cur_row >= max_row) || (!down && cur_row <= start.0) {
            self.prompt.editor_goal_col = None;
            return false;
        }
        let target = if down { cur_row + 1 } else { cur_row - 1 };
        let goal = *self.prompt.editor_goal_col.get_or_insert(cur_col);
        let mut best: Option<(usize, usize)> = None;
        for (i, &(r, c, _)) in positions.iter().enumerate() {
            if r == target {
                let dist = c.abs_diff(goal);
                if best.is_none_or(|(_, bd)| dist < bd) {
                    best = Some((i, dist));
                }
            }
        }
        if end_caret.0 == target {
            let dist = end_caret.1.abs_diff(goal);
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((len, dist));
            }
        }
        let Some((idx, _)) = best else {
            return false;
        };
        if shift {
            self.prompt.cmd.begin_selection();
        } else {
            self.prompt.cmd.clear_selection();
        }
        self.prompt.cmd.set_cursor(idx);
        true
    }

    pub(super) fn has_selection(&self) -> bool {
        self.terminal.has_selection_nonblocking()
    }

    /// Whether *this frame* draws a selection. The grid half comes from
    /// [`Self::sync_frame_facts`] rather than the terminal, which is what keeps
    /// the draw off the lock; a selection that appears while the reader holds
    /// it is drawn one frame later.
    pub(super) fn any_selection(&self) -> bool {
        self.render.frame_has_selection || (self.input_active() && self.prompt.cmd.selected_text().is_some())
    }

    /// The keymap context this pane declares each frame.
    ///
    /// `alt_screen` is how a binding steps aside for a full-screen program:
    /// `AlternatePaste` carries `Terminal && !alt_screen`, so Ctrl+V pastes at
    /// a prompt, reaches vim as SYN, and can still be handed the whole screen
    /// by rebinding `PasteText` onto it (#677).
    pub(crate) fn key_context(&self) -> gpui::KeyContext {
        let mut context = gpui::KeyContext::new_with_defaults(anterm_ui_kit::platform::current_os());
        context.add("Terminal");
        // The frame's own answer, not the terminal's. gpui matches keystrokes
        // against the context the last painted frame published, so this was
        // already a frame-old reading of the mode even when it locked; the
        // chord that must not be a frame late (`AlternatePaste`) asks the
        // terminal again in `alternate_paste`, which is what that comment
        // below is about.
        if self.render.frame_alt_screen {
            context.add("alt_screen");
        }
        if self.prompt.reverse_search.is_some() {
            context.add("reverse_search");
        }
        context
    }

    /// `AlternatePaste`, with the grid asked again before it pastes.
    ///
    /// The `!alt_screen` half of the binding's context comes from the frame
    /// that was last *painted*, and gpui matches keystrokes against that frame
    /// — so a program that took the alternate screen after the last paint is
    /// still "at a prompt" as far as the keymap is concerned. One frame is
    /// enough: the keystroke that launches a full-screen program and the
    /// Ctrl+V after it can land either side of a paint. Pasting there is not a
    /// mistake the user can take back — vim in normal mode runs the clipboard
    /// as commands — so the last word belongs to the terminal mode, not to the
    /// frame. Propagating hands the chord on to `on_key_down`, which encodes it
    /// as the SYN the program is waiting for.
    pub(super) fn alternate_paste(&mut self, cx: &mut Context<Self>) {
        if self.on_alt_screen() {
            cx.propagate();
            return;
        }
        self.paste_from_clipboard(cx);
    }

    pub(crate) fn key_flags(&self) -> crate::input::KeyFlags {
        crate::input::KeyFlags::from_mode_with_local_conpty(
            self.terminal.term.lock().mode(),
            self.terminal.is_local_conpty(),
        )
    }

    pub(super) fn tab_bytes(&self, shift: bool) -> Vec<u8> {
        crate::input::tab_bytes(shift, self.key_flags())
    }

    pub(super) fn jump_to_prompt(&mut self) {
        self.cancel_scroll_anim();
        let mut term = self.terminal.term.lock();
        term.selection = None;
        term.scroll_display(Scroll::Bottom);
        self.terminal.publish_term_state(&term);
        drop(term);
        self.pointer.scroll_frac = 0.;
    }

    pub(crate) fn send_to_pty(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.terminal.exited || !self.accepts_input(cx) {
            return;
        }
        // When a mobile device explicitly holds a SizeLease on this pane and
        // the user starts typing directly at the desktop keyboard, reclaim the
        // lease automatically so control returns effortlessly to the PC.
        if !bytes.is_empty() && self.terminal.leased_by().is_some() {
            self.terminal.take_back();
        }
        self.terminal.write(bytes.to_vec());
        self.render.cursor_visible = true;
        self.jump_to_prompt();
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let mut term = self.terminal.term.lock();
        let grid = term.grid();
        let start = Point::new(grid.topmost_line(), Column(0));
        let end = Point::new(grid.bottommost_line(), grid.last_column());
        let mut sel = Selection::new(SelectionType::Simple, start, Side::Left);
        sel.update(end, Side::Right);
        term.selection = Some(sel);
        self.terminal.publish_term_state(&term);
        drop(term);
        cx.notify();
    }

    pub fn select_all_contextual(&mut self, cx: &mut Context<Self>) {
        if self.input_active() {
            self.prompt.cmd.select_all();
            cx.notify();
        } else {
            self.select_all(cx);
        }
    }

    pub fn paste(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.accepts_input(cx) {
            return;
        }
        // The agent's input is under the box, so a paste meant for it goes
        // into the box — out of sight under it, it would be sent unseen.
        if self.composer_takes_typing(&text, cx) {
            return;
        }
        // Same reason as `commit_text`: what is pasted lands on the prompt, so
        // the prompt is what has to be on screen. Neither branch below moved
        // the viewport, and a paste is a bigger change than a keystroke to
        // make out of sight. This also clears the selection, which the tail of
        // this function used to do on its own.
        self.jump_to_prompt();
        if self.input_active() {
            self.engage_hold_into_editor();
            self.adopt_typeahead();
            let trimmed = text.strip_suffix('\n').unwrap_or(&text);
            self.prompt.cmd.insert_pasted(trimmed);
            self.reset_history_nav();
            self.prompt.editor_goal_col = None;
            self.close_completion();
            self.render.cursor_visible = true;
            cx.notify();
            return;
        }
        let bracketed = self
            .terminal
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        self.write_gap_text(&text, paste_bytes(&text, bracketed), true, cx);
        cx.notify();
    }

    /// Record where this frame anchors the IME, and say whether the platform
    /// has to be told. macOS asks for the rectangle itself whenever it needs
    /// one; Linux only takes it when it is pushed, and gpui pushes it on focus
    /// and around a preedit, never because the caret moved. A TUI moves the
    /// caret on its own — every redraw, every keystroke it echoes — so without
    /// a push the candidate window stayed wherever the last composition or
    /// the focus-in left it, or at the window's corner (#1131).
    pub(crate) fn ime_anchor_moved(&mut self, anchor: Option<gpui::Bounds<Pixels>>) -> bool {
        if self.render.ime_anchor == anchor {
            return false;
        }
        self.render.ime_anchor = anchor;
        anchor.is_some()
    }

    pub fn mouse_mode(&self) -> bool {
        self.pointer.report_mouse
            && self
                .terminal
                .mode_nonblocking()
                .intersects(TermMode::MOUSE_MODE)
    }

    fn write_mouse(&self, base: u8, mods: &Modifiers, col: usize, row: usize, pressed: bool) {
        let sgr = self
            .terminal
            .mode_nonblocking()
            .contains(TermMode::SGR_MOUSE);
        if let Some(msg) = encode_mouse(sgr, base, mods, col, row, pressed) {
            self.terminal.write(msg);
        }
    }

    pub fn mouse_press(&mut self, button: MouseButton, col: usize, row: usize, mods: &Modifiers) {
        let base = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        self.pointer.last_mouse_cell = Some((col, row));
        self.pointer.mouse_grab = Some(button);
        self.write_mouse(base, mods, col, row, true);
    }

    /// Only the release of a press this pane reported: a button let go
    /// anywhere else in the window is not this program's to hear.
    pub fn mouse_release(&mut self, button: MouseButton, col: usize, row: usize, mods: &Modifiers) {
        if self.pointer.mouse_grab != Some(button) {
            return;
        }
        self.pointer.mouse_grab = None;
        let base = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        self.write_mouse(base, mods, col, row, false);
    }

    /// Only a drag that began with a press this pane reported. Once it has,
    /// the drag follows the pointer out of the grid, pinned to its edge.
    pub fn mouse_drag(&mut self, button: MouseButton, col: usize, row: usize, mods: &Modifiers) {
        if self.pointer.mouse_grab != Some(button) || self.pointer.last_mouse_cell == Some((col, row)) {
            return;
        }
        let wants = self.pointer.report_mouse
            && self
                .terminal
                .mode_nonblocking()
                .intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
        if !wants {
            return;
        }
        self.pointer.last_mouse_cell = Some((col, row));
        let base = match button {
            MouseButton::Left => 32,
            MouseButton::Middle => 33,
            MouseButton::Right => 34,
            _ => return,
        };
        self.write_mouse(base, mods, col, row, true);
    }

    /// No button is down, so no press is still waiting for its release —
    /// even one whose release went to another window and never came back.
    pub(crate) fn drop_mouse_grab(&mut self) {
        self.pointer.mouse_grab = None;
    }

    pub fn mouse_motion(&mut self, col: usize, row: usize, mods: &Modifiers) {
        if self.pointer.last_mouse_cell == Some((col, row)) {
            return;
        }
        if !self.pointer.report_mouse
            || !self
                .terminal
                .mode_nonblocking()
                .contains(TermMode::MOUSE_MOTION)
        {
            return;
        }
        self.pointer.last_mouse_cell = Some((col, row));
        self.write_mouse(35, mods, col, row, true);
    }

    /// `at` is the cell under the pointer when the wheel turned. A wheel
    /// report has to say where it happened — tmux scrolls the pane under the
    /// pointer, or its window list — and the last click is no stand-in for
    /// that: a program asking for button events only (1002) is never told the
    /// pointer moved away from it (#1131).
    pub fn scroll(
        &mut self,
        lines: i32,
        at: Option<(usize, usize)>,
        mods: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        if lines == 0 {
            return;
        }
        self.cancel_scroll_anim();
        let mut mode = self.terminal.mode_nonblocking();
        if !self.pointer.report_mouse {
            mode.remove(TermMode::MOUSE_MODE);
        }
        match wheel_route(mode, mods.shift, lines > 0) {
            WheelRoute::Report { base } => {
                let (col, row) = at.or(self.pointer.last_mouse_cell).unwrap_or((0, 0));
                for _ in 0..lines.unsigned_abs() {
                    self.write_mouse(base, mods, col, row, true);
                }
            }
            WheelRoute::Arrows { seq } => {
                let mut out = Vec::with_capacity(seq.len() * lines.unsigned_abs() as usize);
                for _ in 0..lines.unsigned_abs() {
                    out.extend_from_slice(seq);
                }
                self.terminal.write(out);
            }
            WheelRoute::Scrollback => {
                self.pointer.scroll_frac = 0.;
                let mut term = self.terminal.term.lock();
                term.scroll_display(Scroll::Delta(lines));
                self.terminal.publish_term_state(&term);
                drop(term);
                if self.composer_is_open() {
                    cx.notify();
                } else {
                    self.notify_canvas(cx);
                }
            }
        }
    }

    /// `raw` copies the rows as the grid holds them. Otherwise, in a detected
    /// CLI coding agent pane with `copy_join_wrapped` on, the rows the agent's
    /// TUI wrapped are joined; other programs' output is never joined, since
    /// `ls` columns, separators and an editor's own wraps look similar.
    pub fn copy_selection(&mut self, raw: bool, cx: &mut Context<Self>) {
        let cfg = cx.global::<Config>();
        let join = !raw && cfg.copy_join_wrapped && self.terminal.foreground_agent().is_some();
        let text = {
            let term = self.terminal.term.lock();
            if join {
                copy_unwrap::joined(&term)
            } else {
                term.selection_to_string()
            }
        };
        if let Some(mut text) = text {
            if cfg.clipboard_trim_trailing_spaces {
                text = trim_trailing_spaces(&text);
            }
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    pub fn copy_contextual(
        &mut self,
        clear_on_copy: bool,
        raw: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.input_active() {
            if let Some(text) = self.prompt.cmd.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                if clear_on_copy {
                    self.prompt.cmd.clear_selection();
                    cx.notify();
                }
                return true;
            }
        }
        if self.has_selection() {
            self.copy_selection(raw, cx);
            if clear_on_copy {
                self.terminal.term.lock().selection = None;
                cx.notify();
            }
            return true;
        }
        false
    }

    pub fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let direction = if forward {
            Direction::Right
        } else {
            Direction::Left
        };
        self.step_match(direction, cx);
    }

    pub fn undo_edit(&mut self, redo: bool, cx: &mut Context<Self>) {
        if !self.input_active() {
            return;
        }
        if redo {
            self.prompt.cmd.redo();
        } else {
            self.prompt.cmd.undo();
        }
        self.reset_history_nav();
        self.close_completion();
        cx.notify();
    }

    pub fn cut_contextual(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.input_active() {
            return false;
        }
        if let Some(text) = self.prompt.cmd.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.prompt.cmd.delete_selection();
            self.reset_history_nav();
            self.close_completion();
            self.render.cursor_visible = true;
            cx.notify();
        }
        true
    }

    pub fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let paths = clipboard_paths(&item);
        if !paths.is_empty() {
            self.paste_local_paths(paths, cx);
            return;
        }
        if let Some(text) = item.text() {
            self.paste(text, cx);
            return;
        }
        if self.input_active() {
            return;
        }
        if let Some(img) = item.entries().iter().find_map(|e| match e {
            ClipboardEntry::Image(img) => Some(img),
            _ => None,
        }) {
            self.paste_clipboard_image(img, cx);
        }
    }

    fn editor_char_index(&self, col: usize, row: usize, clamp: bool) -> Option<usize> {
        if !self.input_active() {
            return None;
        }
        let (srow, scol) = self.cursor_cell()?;
        if row < srow {
            return clamp.then_some(0);
        }
        let cols = self.terminal.term.lock().columns().max(1);
        let chars: Vec<char> = self.prompt.cmd.text().chars().collect();
        wrapped_click_index(
            &chars,
            input_start(scol, cols),
            cols,
            col,
            row - srow,
            clamp,
        )
    }

    /// Whether a click on `row`/`col` lands on the prompt the shell drew,
    /// ahead of where the input bar's text begins.
    fn click_on_prompt(&self, col: usize, row: usize) -> bool {
        let Some((srow, scol)) = self.cursor_cell() else {
            return false;
        };
        if row < srow {
            return false;
        }
        let cols = self.terminal.term.lock().columns().max(1);
        before_input(input_start(scol, cols), col, row - srow)
    }

    pub fn editor_click(
        &mut self,
        col: usize,
        row: usize,
        clicks: usize,
        shift: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // A click on the prompt itself — the branch in `git:(main)`, the
        // directory before it — is on the prompt's text, not the editor's.
        // It goes to the grid's select like a click on any other output, and
        // leaves the caret where it was: the first half of a double click
        // must not have already thrown it to the start of the line.
        if self.click_on_prompt(col, row) {
            return false;
        }
        let Some(idx) = self.editor_char_index(col, row, false) else {
            return false;
        };
        match clicks {
            1 if shift => {
                self.prompt.cmd.extend_to(idx);
                self.prompt.editor_selecting = true;
                self.prompt.editor_drag_word = None;
            }
            1 => {
                self.prompt.cmd.set_cursor(idx);
                self.prompt.cmd.clear_selection();
                self.prompt.editor_selecting = true;
                self.prompt.editor_drag_word = None;
            }
            2 => {
                let cfg = cx.global::<Config>();
                let (seps, smart) = (cfg.word_separators.clone(), cfg.smart_select);
                self.prompt.cmd.select_word_at(idx, &seps, smart);
                self.prompt.editor_selecting = true;
                self.prompt.editor_drag_word = self.prompt.cmd.selection();
            }
            _ => {
                self.prompt.cmd.select_all();
                self.prompt.editor_selecting = false;
                self.prompt.editor_drag_word = None;
            }
        }
        self.prompt.editor_select_gesture = true;
        self.prompt.editor_goal_col = None;
        self.close_completion();
        if self.prompt.helix_mode {
            self.prompt.cmd.sync_to_helix();
        }
        self.render.cursor_visible = true;
        cx.notify();
        true
    }

    pub fn editor_drag(&mut self, col: usize, row: usize, cx: &mut Context<Self>) -> bool {
        if !self.prompt.editor_selecting {
            return false;
        }
        let Some(idx) = self.editor_char_index(col, row, true) else {
            return false;
        };
        if let Some((s, e)) = self.prompt.editor_drag_word {
            let cfg = cx.global::<Config>();
            let (seps, smart) = (cfg.word_separators.clone(), cfg.smart_select);
            self.prompt.cmd.extend_word_to(s, e, idx, &seps, smart);
        } else {
            self.prompt.cmd.extend_to(idx);
        }
        if self.prompt.helix_mode {
            self.prompt.cmd.sync_to_helix();
        }
        self.render.cursor_visible = true;
        cx.notify();
        true
    }

    pub fn input_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.commit_text(text, cx);
    }

    pub fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.terminal.exited || text.is_empty() || !self.accepts_input(cx) {
            return;
        }
        if self.composer_takes_typing(text, cx) {
            return;
        }
        // Typing goes to the prompt, so the view has to be looking at it.
        // Only the last branch below used to do this, which is the branch
        // taken when anterm is *not* driving the line — so scrolling up and
        // typing did the one thing it must never do at a shell prompt:
        // accepted the characters somewhere the user could not see them.
        // `handle_editor_key` has always jumped, so Left and Backspace came
        // back to the prompt and the letters between them did not.
        self.jump_to_prompt();
        if self.prompt.reverse_search.is_some() {
            self.push_reverse_search_query(text, cx);
            self.render.cursor_visible = true;
            cx.notify();
            return;
        }
        if self.input_active() {
            self.sync_shared_history(cx);
            self.engage_hold_into_editor();
            self.adopt_typeahead();
            self.prompt.cmd.insert_str(text);
            self.reset_history_nav();
            self.prompt.editor_goal_col = None;
            self.prompt.last_word_nav = None;
            self.completion_refilter();
            self.render.cursor_visible = true;
            cx.notify();
            return;
        }
        self.write_gap_text(text, text.as_bytes().to_vec(), false, cx);
        self.render.cursor_visible = true;
        cx.notify();
    }

    pub fn set_marked_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.session.marked_text = text;
        cx.notify();
    }

    pub fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        if !self.session.marked_text.is_empty() {
            self.session.marked_text.clear();
            cx.notify();
        }
    }

    pub fn on_select_start(
        &mut self,
        col: usize,
        row: usize,
        left: bool,
        clicks: usize,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let smart = cx.global::<Config>().smart_select;
        let mut term = self.terminal.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let point = Point::new(Line(row as i32 - display_offset), Column(col));
        let side = if left { Side::Left } else { Side::Right };
        if shift && clicks == 1 && term.selection.is_some() {
            if let Some(sel) = term.selection.as_mut() {
                sel.update(point, side);
            }
            drop(term);
            self.pointer.selecting = true;
            cx.notify();
            return;
        }
        let ty = match clicks {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        let mut selection = Selection::new(ty, point, side);
        if clicks == 2
            && smart
            && let Some(r) = crate::links::smart_select::grid_smart_range(&term, point)
        {
            let ty = if r.exact {
                SelectionType::Simple
            } else {
                SelectionType::Semantic
            };
            selection = Selection::new(ty, r.start, Side::Left);
            selection.update(r.end, Side::Right);
        }
        term.selection = Some(selection);
        self.terminal.publish_term_state(&term);
        drop(term);
        self.pointer.selecting = true;
        cx.notify();
    }

    pub fn on_select_update(&mut self, col: usize, row: usize, left: bool, cx: &mut Context<Self>) {
        if !self.pointer.selecting {
            return;
        }
        let mut term = self.terminal.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let point = Point::new(Line(row as i32 - display_offset), Column(col));
        let side = if left { Side::Left } else { Side::Right };
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, side);
        }
        drop(term);
        cx.notify();
    }

    pub fn select_autoscroll(
        &mut self,
        overshoot: f32,
        col: usize,
        left: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.pointer.selecting || overshoot == 0. {
            self.pointer.drag_scroll = None;
            return;
        }
        let side = if left { Side::Left } else { Side::Right };
        let was_idle = self.pointer.drag_scroll.is_none();
        self.pointer.drag_scroll = Some(DragScroll {
            overshoot,
            col,
            side,
        });
        if !was_idle {
            return;
        }
        self.pointer.drag_scroll_epoch += 1;
        let epoch = self.pointer.drag_scroll_epoch;
        self.drag_scroll_tick(epoch, cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                if !matches!(
                    this.update(cx, |view, cx| view.drag_scroll_tick(epoch, cx)),
                    Ok(true)
                ) {
                    break;
                }
            }
        })
        .detach();
    }

    fn drag_scroll_tick(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        if epoch != self.pointer.drag_scroll_epoch {
            return false;
        }
        if !self.pointer.selecting {
            self.pointer.drag_scroll = None;
        }
        let Some(ds) = self.pointer.drag_scroll else {
            return false;
        };
        self.cancel_scroll_anim();
        let mut term = self.terminal.term.lock();
        let before = term.grid().display_offset();
        term.scroll_display(Scroll::Delta(drag_scroll_step(ds.overshoot)));
        let offset = term.grid().display_offset();
        let row = if ds.overshoot > 0. {
            0
        } else {
            term.screen_lines().saturating_sub(1)
        };
        let point = Point::new(Line(row as i32 - offset as i32), Column(ds.col));
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, ds.side);
        }
        drop(term);
        if offset != before {
            self.pointer.scroll_frac = 0.;
            cx.notify();
        }
        true
    }

    pub fn on_select_end(&mut self, cx: &mut Context<Self>) {
        let copy = select_end_copy(
            cx.global::<Config>().copy_on_select,
            self.pointer.selecting,
            self.prompt.editor_select_gesture,
        );
        self.pointer.selecting = false;
        self.prompt.editor_selecting = false;
        self.prompt.editor_select_gesture = false;
        self.prompt.editor_drag_word = None;
        self.pointer.drag_scroll = None;
        match copy {
            SelectEndCopy::None => {}
            SelectEndCopy::Grid => self.copy_selection(false, cx),
            SelectEndCopy::Editor => {
                if let Some(text) = self.prompt.cmd.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
        }
    }

    pub(super) fn on_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let gesturing = self.track_scroll_gesture(ev.touch_phase);
        // One modifier turns the wheel into a zoom, the way it does in a
        // browser. Which one is the user's to say, because the default is the
        // platform modifier and on macOS that is a key half the world is
        // already holding for something else (#668).
        let wants_zoom = zoom_wheel(cx.global::<Config>().mouse_zoom_modifier, &ev.modifiers);
        // A trackpad gesture answers "scroll or zoom?" once, on its first
        // event, and keeps that answer until the stream dies — momentum tail
        // included. The tail is why: those events are the system's, not the
        // hand's, yet each one is stamped with whatever modifiers happen to be
        // down as it is delivered. Reaching for ⌘ during a flick's coast —
        // ⌘-Tab, ⌘-C, anything — would otherwise turn hundreds of coasting
        // lines into zoom steps and leave the font at its minimum, from a
        // gesture that was never a zoom (#912).
        // The answer is latched per gesture, not per stream: fingers going
        // back down ask it again. Carrying it over would be the same bug
        // wearing the other coat — one ⌘-zoom, and every later flick zooms
        // with nothing held at all, because `Started` keeps the gesture live
        // and would find the old answer still sitting there.
        if !gesturing || matches!(ev.touch_phase, gpui::TouchPhase::Started) {
            self.pointer.gesture_zoom = None;
            // Leftover travel belongs to the gesture that earned it; a new one
            // must not start already part-way to a step.
            self.pointer.zoom_debt = 0.;
        }
        let zoom = if gesturing {
            *self.pointer.gesture_zoom.get_or_insert(wants_zoom)
        } else {
            wants_zoom
        };
        if zoom {
            self.zoom_scroll(ev, gesturing, window, cx);
            return;
        }
        let mult = cx.global::<Config>().mouse_scroll_multiplier;
        let raw = match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y.as_f32() / self.render.line_height.as_f32(),
        };
        let delta = raw * mult;

        let quantized = !ev.modifiers.shift && {
            let mode = *self.terminal.term.lock().mode();
            mode.intersects(TermMode::MOUSE_MODE)
                || mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
        };
        if quantized {
            // Whole lines are what the application gets told about, so this path
            // cannot be spread over frames.
            self.cancel_scroll_anim();
            let total = self.pointer.scroll_debt + delta;
            let lines = total.trunc() as i32;
            self.pointer.scroll_debt = total - lines as f32;
            if lines != 0 {
                let at = self
                    .render
                    .cell_geom
                    .map(|geom| geom.pos_to_cell(ev.position))
                    .map(|(col, row, _)| (col, row));
                self.scroll(lines, at, &ev.modifiers, cx);
            }
            return;
        }

        if self.should_animate_scroll(delta, gesturing, cx) {
            self.queue_scroll_anim(delta, window, cx);
        } else {
            self.cancel_scroll_anim();
            self.smooth_scroll(delta, cx);
        }
    }

    /// Resize the terminal font by whole steps under the platform modifier.
    ///
    /// The event never reaches the buffer, and never reaches the program
    /// running in it either: zooming is chrome, and showing a pane to someone
    /// standing behind you has to work the same whether or not what is running
    /// asked for the wheel. Steps go out as the same actions the keyboard and
    /// the View menu send, so the min/max clamp and the saved setting live in
    /// one place — [`AntermApp::change_font_size`](crate::ui::workspace::app::AntermApp).
    fn zoom_scroll(
        &mut self,
        ev: &ScrollEvent,
        gesturing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Whatever the scrollback still had in flight is dropped: it was
        // travelling in lines of a font that is about to change size.
        self.cancel_scroll_anim();
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y.as_f32() / self.render.line_height.as_f32(),
        };
        let (steps, debt) = zoom_scroll_steps(lines, self.pointer.zoom_debt, gesturing);
        self.pointer.zoom_debt = debt;
        for _ in 0..steps.unsigned_abs() {
            if steps > 0 {
                window.dispatch_action(Box::new(IncreaseFontSize), cx);
            } else {
                window.dispatch_action(Box::new(DecreaseFontSize), cx);
            }
        }
    }

    /// Track whether the pointing device is mid-gesture, which is what tells a
    /// trackpad apart from a wheel.
    ///
    /// Not the delta *type*: macOS reports a wheel mouse as pixels too — one
    /// notch arrives as a single ~100px event — so `Pixels` says nothing about
    /// the device. Phase does: only devices that can gesture ever report
    /// `Started`/`Ended`, and a wheel is `Moved` forever, on every platform.
    ///
    /// The gesture is held open on a timer rather than closed on `Ended`,
    /// because lifting the fingers is not the end of the stream — the momentum
    /// tail keeps delivering `Moved` events, larger than the gesture itself,
    /// and animating those would put a second layer of smoothing on scrolling
    /// the system is already smoothing.
    fn track_scroll_gesture(&mut self, phase: gpui::TouchPhase) -> bool {
        let now = std::time::Instant::now();
        let live = matches!(phase, gpui::TouchPhase::Started)
            || self.pointer.gesture_until.is_some_and(|until| now < until);
        self.pointer.gesture_until = live.then(|| now + SCROLL_GESTURE_IDLE);
        live
    }

    fn should_animate_scroll(&self, delta: f32, gesturing: bool, cx: &App) -> bool {
        if gesturing || !cx.global::<Config>().smooth_scroll {
            return false;
        }
        // Anything already in flight keeps accumulating, or a slow notch
        // arriving mid-animation would fight the frames still to come.
        self.pointer.scroll_anim.is_some() || delta.abs() >= SCROLL_ANIM_MIN_JUMP
    }

    /// Add `delta` to the in-flight animation, starting the frame loop if it is
    /// idle. Successive notches accumulate rather than restart, so spinning the
    /// wheel fast still lands exactly where the notches asked for.
    fn queue_scroll_anim(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(anim) = self.pointer.scroll_anim.as_mut() {
            anim.remaining += delta;
            return;
        }
        self.pointer.scroll_anim = Some(ScrollAnim {
            remaining: delta,
            last: std::time::Instant::now(),
        });
        self.pointer.scroll_anim_epoch += 1;
        let epoch = self.pointer.scroll_anim_epoch;
        schedule_scroll_anim_frame(cx.weak_entity(), epoch, window);
        // Ask for a frame right away: the callback chain's first step then
        // fires on the next display refresh even on a backend that only draws
        // when the window is dirty, and the callback keeps it dirty from then
        // on. Without this, a wheel notch queued on an idle, unfocused pane
        // could sit dormant instead of spreading over frames.
        cx.notify();
    }

    /// Drop whatever is left to travel. Every other way the display offset moves
    /// — jumping to a prompt, dragging a selection past the edge, the keyboard
    /// and mouse-reporting paths — has to come through here first, or the
    /// animation would keep walking away from wherever it put us.
    pub(super) fn cancel_scroll_anim(&mut self) {
        if self.pointer.scroll_anim.take().is_some() {
            self.pointer.scroll_anim_epoch += 1;
        }
    }

    /// Advance the in-flight animation by the ground it covered since the
    /// previous frame. Runs at frame time — right before the next frame is
    /// drawn — so every presented frame shows exactly one decay step and no
    /// step is ever skipped or doubled against the monitor's cadence. Returns
    /// whether the animation is still going.
    pub(super) fn scroll_anim_frame(&mut self, epoch: u64, cx: &mut Context<Self>) -> bool {
        if epoch != self.pointer.scroll_anim_epoch {
            return false;
        }
        let Some(anim) = self.pointer.scroll_anim.as_mut() else {
            return false;
        };
        let now = std::time::Instant::now();
        let dt = now.duration_since(anim.last);
        anim.last = now;
        let (step, last) = scroll_anim_step(anim.remaining, dt);
        anim.remaining -= step;
        if last {
            self.pointer.scroll_anim = None;
        }
        // Hitting the top or the bottom of the scrollback consumes nothing; the
        // remaining distance has nowhere to go, so stop instead of decaying it
        // against the clamp for another 100ms.
        let moved = self.smooth_scroll(step, cx);
        if !moved {
            self.cancel_scroll_anim();
            return false;
        }
        !last
    }

    /// Apply `delta` lines right now. Returns whether anything actually moved.
    pub(super) fn smooth_scroll(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        let mut term = self.terminal.term.lock();
        let offset = term.grid().display_offset();
        let max = term.grid().history_size();
        let (jump, frac) = smooth_scroll_step(offset, self.pointer.scroll_frac, delta, max);
        if jump != 0 {
            term.scroll_display(Scroll::Delta(jump));
        }
        drop(term);
        if jump != 0 || frac != self.pointer.scroll_frac {
            self.pointer.scroll_frac = frac;
            cx.notify();
            return true;
        }
        false
    }
}
