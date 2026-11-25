//! Thin [`ansi::Handler`] adapter around official `alacritty_terminal::Term`.
//!
//! Keeps four terminal-correctness invariants without carrying a fork of
//! `alacritty_terminal`:
//! 1. **UTS #51 emoji presentation width (`U+FE0F`)**: widens a single-column
//!    neutral-width emoji base (e.g. `❤` + `U+FE0F` -> `❤️`) into `WIDE_CHAR` +
//!    `WIDE_CHAR_SPACER` when `UnicodeWidthStr` scores the sequence as width 2.
//! 2. **DEC Mode 2027 (Unicode Grapheme Clustering)**: responds to DECRQM
//!    `CSI ? 2027 $ p` and, while enabled (`CSI ? 2027 h`), folds ZWJ emoji
//!    sequences (`👨‍💻`, `👨‍👩‍👧‍👦`), Fitzpatrick skin-tone modifiers (`👍🏽`),
//!    keycap clusters (`1️⃣`), and regional-indicator flag pairs (`🇺🇸`) into
//!    a single 2-cell wide cluster so cursor column accounting stays exact.
//! 3. **Kitty keyboard stack overflow guard**: `alacritty_terminal 0.26.0`
//!    contains an upstream copy-paste bug in `push_keyboard_mode` that trims
//!    `title_stack` instead of `keyboard_mode_stack` at depth 4096 (panicking
//!    when `title_stack` is empty). Tracking per-screen push depth and capping
//!    at 4095 prevents the crash while keeping the active mode accurate.
//! 4. **Dual-screen keyboard mode reset**: empties both primary and alternate
//!    keyboard mode stacks when returning to a shell prompt (`#1155`).

use alacritty_terminal::event::{Event as AlacEvent, EventListener};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{
    self, Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink,
    KeyboardModes, KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys,
    NamedPrivateMode, PrivateMode, Rgb, ScpCharPath, ScpUpdateMode, StandardCharset,
    TabulationClearMode,
};
use unicode_width::UnicodeWidthStr as _;

/// One less than `alacritty_terminal`'s internal `KEYBOARD_MODE_STACK_MAX_DEPTH`
/// (4096), so `Term::push_keyboard_mode` never executes its buggy overflow arm.
const MAX_SAFE_KEYBOARD_DEPTH: usize = 4095;

/// DEC Private Mode 2027: Unicode Grapheme Clustering.
const DEC_MODE_GRAPHEME_CLUSTERING: u16 = 2027;

/// Per-pane parser adapter state: kitty keyboard mode stack depths on both
/// screens (`[0]` primary, `[1]` alternate) and DEC Mode 2027 grapheme
/// clustering state.
#[derive(Default)]
pub(super) struct KeyboardStackGuard {
    depths: [usize; 2],
    grapheme_cluster: bool,
    zwj_pending: bool,
}

impl KeyboardStackGuard {
    #[inline]
    fn slot<T>(term: &Term<T>) -> usize {
        usize::from(term.mode().contains(TermMode::ALT_SCREEN))
    }

    pub(super) fn reset(&mut self) {
        self.depths = [0, 0];
        self.grapheme_cluster = false;
        self.zwj_pending = false;
    }

    pub(super) fn reset_keyboard_only(&mut self) {
        self.depths = [0, 0];
    }

    #[cfg(test)]
    pub(super) fn grapheme_clustering(&self) -> bool {
        self.grapheme_cluster
    }
}

/// Drops kitty keyboard protocol flags and empties both screens' keyboard mode
/// stacks without disturbing primary-screen contents, selection, or saved cursor.
pub(super) fn reset_keyboard_modes<T: EventListener>(
    term: &mut Term<T>,
    guard: &mut KeyboardStackGuard,
) {
    term.pop_keyboard_modes(u16::MAX);
    term.set_keyboard_mode(KeyboardModes::NO_MODE, KeyboardModesApplyBehavior::Replace);

    if !term.mode().contains(TermMode::ALT_SCREEN) {
        let selection = term.selection.take();
        let saved_cursor = term.grid().saved_cursor.clone();
        term.swap_alt();
        term.pop_keyboard_modes(u16::MAX);
        term.set_keyboard_mode(KeyboardModes::NO_MODE, KeyboardModesApplyBehavior::Replace);
        term.swap_alt();
        term.grid_mut().saved_cursor = saved_cursor;
        term.selection = selection;
    }

    guard.reset_keyboard_only();
}

/// Feeds `bytes` through `processor` into `term` via [`TermAdapter`].
///
/// Returns `true` if any visible grid, cursor, mode, title, or bell mutation
/// actually reached `term` (i.e. bytes were not merely buffered inside an open
/// DEC `?2026` synchronized update).
pub(super) fn advance<T: EventListener>(
    processor: &mut ansi::Processor,
    term: &mut Term<T>,
    guard: &mut KeyboardStackGuard,
    bytes: &[u8],
) -> bool {
    advance_with_listener(processor, term, guard, None, bytes)
}

/// Like [`advance`], forwarding DECRQM replies for adapter-owned modes (such
/// as DEC `?2027`) to `listener`.
pub(super) fn advance_with_listener<T: EventListener>(
    processor: &mut ansi::Processor,
    term: &mut Term<T>,
    guard: &mut KeyboardStackGuard,
    listener: Option<&T>,
    bytes: &[u8],
) -> bool {
    let mut adapter = TermAdapter {
        term,
        guard,
        listener,
        mutated: false,
    };
    processor.advance(&mut adapter, bytes);
    adapter.mutated
}

/// Flushes a timed-out synchronized update into `term` via [`TermAdapter`],
/// forwarding DECRQM replies for adapter-owned modes to `listener`.
pub(super) fn stop_sync_with_listener<T: EventListener>(
    processor: &mut ansi::Processor,
    term: &mut Term<T>,
    guard: &mut KeyboardStackGuard,
    listener: Option<&T>,
) -> bool {
    let mut adapter = TermAdapter {
        term,
        guard,
        listener,
        mutated: false,
    };
    processor.stop_sync(&mut adapter);
    adapter.mutated
}

struct TermAdapter<'a, T> {
    term: &'a mut Term<T>,
    guard: &'a mut KeyboardStackGuard,
    listener: Option<&'a T>,
    mutated: bool,
}

fn previous_cell_coord<T: EventListener>(term: &Term<T>) -> Option<(Line, Column)> {
    let cursor = term.grid().cursor.point;
    let input_needs_wrap = term.grid().cursor.input_needs_wrap;
    if !input_needs_wrap && cursor.column.0 == 0 {
        return None;
    }
    let mut column = cursor.column;
    if !input_needs_wrap {
        column.0 -= 1;
    }
    let line = cursor.line;
    if term.grid()[line][column]
        .flags
        .contains(Flags::WIDE_CHAR_SPACER)
    {
        column.0 = column.saturating_sub(1);
    }
    Some((line, column))
}

fn widen_cell_if_narrow<T: EventListener>(term: &mut Term<T>, line: Line, column: Column) {
    let cursor = term.grid().cursor.point;
    let input_needs_wrap = term.grid().cursor.input_needs_wrap;
    if term.grid()[line][column].flags.contains(Flags::WIDE_CHAR) {
        return;
    }
    if input_needs_wrap || cursor.line != line || cursor.column != column + 1 {
        return;
    }

    term.grid_mut()[line][column].flags.insert(Flags::WIDE_CHAR);
    term.grid_mut()
        .cursor
        .template
        .flags
        .insert(Flags::WIDE_CHAR_SPACER);
    term.input(' ');
    term.grid_mut()
        .cursor
        .template
        .flags
        .remove(Flags::WIDE_CHAR_SPACER);
}

fn widen_emoji_presentation<T: EventListener>(term: &mut Term<T>) {
    let Some((line, column)) = previous_cell_coord(term) else {
        return;
    };
    let cell = &term.grid()[line][column];
    if cell.flags.contains(Flags::WIDE_CHAR) {
        return;
    }

    let mut sequence = String::with_capacity(8);
    sequence.push(cell.c);
    sequence.extend(cell.zerowidth().into_iter().flatten());
    if sequence.as_str().width() != 2 {
        return;
    }

    widen_cell_if_narrow(term, line, column);
}

fn append_to_previous_cell<T: EventListener>(term: &mut Term<T>, c: char) -> bool {
    let Some((line, column)) = previous_cell_coord(term) else {
        return false;
    };
    term.grid_mut()[line][column].push_zerowidth(c);
    true
}

fn append_skin_tone_to_previous_cell<T: EventListener>(term: &mut Term<T>, c: char) -> bool {
    let Some((line, column)) = previous_cell_coord(term) else {
        return false;
    };
    if term.grid()[line][column].c.is_ascii() {
        return false;
    }
    term.grid_mut()[line][column].push_zerowidth(c);
    widen_cell_if_narrow(term, line, column);
    true
}

fn append_regional_indicator_to_previous_cell<T: EventListener>(
    term: &mut Term<T>,
    c: char,
) -> bool {
    let Some((line, column)) = previous_cell_coord(term) else {
        return false;
    };
    let cell = &term.grid()[line][column];
    if !matches!(cell.c, '\u{1F1E6}'..='\u{1F1FF}')
        || cell.zerowidth().is_some()
        || cell.flags.contains(Flags::WIDE_CHAR)
    {
        return false;
    }
    term.grid_mut()[line][column].push_zerowidth(c);
    widen_cell_if_narrow(term, line, column);
    true
}

fn widen_keycap_cluster<T: EventListener>(term: &mut Term<T>) {
    let Some((line, column)) = previous_cell_coord(term) else {
        return;
    };
    if matches!(term.grid()[line][column].c, '0'..='9' | '#' | '*') {
        widen_cell_if_narrow(term, line, column);
    }
}

impl<T: EventListener> Handler for TermAdapter<'_, T> {
    fn set_title(&mut self, title: Option<String>) {
        self.mutated = true;
        self.term.set_title(title);
    }

    fn set_cursor_style(&mut self, style: Option<CursorStyle>) {
        self.mutated = true;
        self.term.set_cursor_style(style);
    }

    fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.mutated = true;
        self.term.set_cursor_shape(shape);
    }

    fn input(&mut self, c: char) {
        self.mutated = true;
        if self.guard.grapheme_cluster {
            if self.guard.zwj_pending {
                self.guard.zwj_pending = false;
                if !c.is_ascii()
                    && !c.is_control()
                    && c != '\u{200D}'
                    && append_to_previous_cell(self.term, c)
                {
                    if c == '\u{FE0F}' {
                        widen_emoji_presentation(self.term);
                    }
                    return;
                }
            }
            if c == '\u{200D}' {
                self.term.input(c);
                if previous_cell_coord(self.term).is_some() {
                    self.guard.zwj_pending = true;
                }
                return;
            }
            if matches!(c, '\u{1F3FB}'..='\u{1F3FF}')
                && append_skin_tone_to_previous_cell(self.term, c)
            {
                return;
            }
            if matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
                && append_regional_indicator_to_previous_cell(self.term, c)
            {
                return;
            }
            self.term.input(c);
            if c == '\u{FE0F}' {
                widen_emoji_presentation(self.term);
            } else if c == '\u{20E3}' {
                widen_keycap_cluster(self.term);
            }
            return;
        }
        self.term.input(c);
        if c == '\u{FE0F}' {
            widen_emoji_presentation(self.term);
        }
    }

    fn goto(&mut self, line: i32, col: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.goto(line, col);
    }

    fn goto_line(&mut self, line: i32) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.goto_line(line);
    }

    fn goto_col(&mut self, col: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.goto_col(col);
    }

    fn insert_blank(&mut self, count: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.insert_blank(count);
    }

    fn move_up(&mut self, rows: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_up(rows);
    }

    fn move_down(&mut self, rows: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_down(rows);
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        self.term.identify_terminal(intermediate);
    }

    fn device_status(&mut self, arg: usize) {
        self.term.device_status(arg);
    }

    fn move_forward(&mut self, cols: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_forward(cols);
    }

    fn move_backward(&mut self, cols: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_backward(cols);
    }

    fn move_down_and_cr(&mut self, rows: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_down_and_cr(rows);
    }

    fn move_up_and_cr(&mut self, rows: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_up_and_cr(rows);
    }

    fn put_tab(&mut self, count: u16) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.put_tab(count);
    }

    fn backspace(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.backspace();
    }

    fn carriage_return(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.carriage_return();
    }

    fn linefeed(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.linefeed();
    }

    fn bell(&mut self) {
        self.mutated = true;
        self.term.bell();
    }

    fn substitute(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.substitute();
    }

    fn newline(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.newline();
    }

    fn set_horizontal_tabstop(&mut self) {
        self.mutated = true;
        self.term.set_horizontal_tabstop();
    }

    fn scroll_up(&mut self, rows: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.scroll_up(rows);
    }

    fn scroll_down(&mut self, rows: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.scroll_down(rows);
    }

    fn insert_blank_lines(&mut self, count: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.insert_blank_lines(count);
    }

    fn delete_lines(&mut self, count: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.delete_lines(count);
    }

    fn erase_chars(&mut self, count: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.erase_chars(count);
    }

    fn delete_chars(&mut self, count: usize) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.delete_chars(count);
    }

    fn move_backward_tabs(&mut self, count: u16) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_backward_tabs(count);
    }

    fn move_forward_tabs(&mut self, count: u16) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.move_forward_tabs(count);
    }

    fn save_cursor_position(&mut self) {
        self.term.save_cursor_position();
    }

    fn restore_cursor_position(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.restore_cursor_position();
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.clear_line(mode);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.clear_screen(mode);
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        self.term.clear_tabs(mode);
    }

    fn set_tabs(&mut self, interval: u16) {
        self.term.set_tabs(interval);
    }

    fn reset_state(&mut self) {
        self.mutated = true;
        self.guard.reset();
        self.term.reset_state();
    }

    fn reverse_index(&mut self) {
        self.guard.zwj_pending = false;
        self.mutated = true;
        self.term.reverse_index();
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.term.terminal_attribute(attr);
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mutated = true;
        self.term.set_mode(mode);
    }

    fn unset_mode(&mut self, mode: Mode) {
        self.mutated = true;
        self.term.unset_mode(mode);
    }

    fn report_mode(&mut self, mode: Mode) {
        self.term.report_mode(mode);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        if mode == PrivateMode::Unknown(DEC_MODE_GRAPHEME_CLUSTERING) {
            self.guard.grapheme_cluster = true;
            self.guard.zwj_pending = false;
            return;
        }
        if !matches!(mode, PrivateMode::Named(NamedPrivateMode::SyncUpdate)) {
            self.mutated = true;
        }
        self.term.set_private_mode(mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if mode == PrivateMode::Unknown(DEC_MODE_GRAPHEME_CLUSTERING) {
            self.guard.grapheme_cluster = false;
            self.guard.zwj_pending = false;
            return;
        }
        if !matches!(mode, PrivateMode::Named(NamedPrivateMode::SyncUpdate)) {
            self.mutated = true;
        }
        self.term.unset_private_mode(mode);
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        if mode == PrivateMode::Unknown(DEC_MODE_GRAPHEME_CLUSTERING) {
            if let Some(listener) = self.listener {
                let state = if self.guard.grapheme_cluster { 1 } else { 2 };
                listener.send_event(AlacEvent::PtyWrite(format!("\x1b[?2027;{state}$y")));
            }
            return;
        }
        self.term.report_private_mode(mode);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        self.mutated = true;
        self.term.set_scrolling_region(top, bottom);
    }

    fn set_keypad_application_mode(&mut self) {
        self.mutated = true;
        self.term.set_keypad_application_mode();
    }

    fn unset_keypad_application_mode(&mut self) {
        self.mutated = true;
        self.term.unset_keypad_application_mode();
    }

    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.term.set_active_charset(index);
    }

    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.term.configure_charset(index, charset);
    }

    fn set_color(&mut self, index: usize, color: Rgb) {
        self.mutated = true;
        self.term.set_color(index, color);
    }

    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, terminator: &str) {
        self.term.dynamic_color_sequence(prefix, index, terminator);
    }

    fn reset_color(&mut self, index: usize) {
        self.mutated = true;
        self.term.reset_color(index);
    }

    fn clipboard_store(&mut self, clipboard: u8, base64: &[u8]) {
        self.term.clipboard_store(clipboard, base64);
    }

    fn clipboard_load(&mut self, clipboard: u8, terminator: &str) {
        self.term.clipboard_load(clipboard, terminator);
    }

    fn decaln(&mut self) {
        self.mutated = true;
        self.term.decaln();
    }

    fn push_title(&mut self) {
        self.mutated = true;
        self.term.push_title();
    }

    fn pop_title(&mut self) {
        self.mutated = true;
        self.term.pop_title();
    }

    fn text_area_size_pixels(&mut self) {
        self.term.text_area_size_pixels();
    }

    fn text_area_size_chars(&mut self) {
        self.term.text_area_size_chars();
    }

    fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        self.term.set_hyperlink(hyperlink);
    }

    fn report_keyboard_mode(&mut self) {
        self.term.report_keyboard_mode();
    }

    fn push_keyboard_mode(&mut self, mode: KeyboardModes) {
        self.mutated = true;
        let slot = KeyboardStackGuard::slot(self.term);
        if self.guard.depths[slot] >= MAX_SAFE_KEYBOARD_DEPTH {
            self.term.pop_keyboard_modes(1);
            self.guard.depths[slot] = self.guard.depths[slot].saturating_sub(1);
        }
        self.term.push_keyboard_mode(mode);
        self.guard.depths[slot] += 1;
    }

    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        self.mutated = true;
        let slot = KeyboardStackGuard::slot(self.term);
        self.guard.depths[slot] = self.guard.depths[slot].saturating_sub(to_pop as usize);
        self.term.pop_keyboard_modes(to_pop);
    }

    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        self.mutated = true;
        self.term.set_keyboard_mode(mode, behavior);
    }

    fn set_modify_other_keys(&mut self, mode: ModifyOtherKeys) {
        self.mutated = true;
        self.term.set_modify_other_keys(mode);
    }

    fn report_modify_other_keys(&mut self) {
        self.term.report_modify_other_keys();
    }

    fn set_scp(&mut self, char_path: ScpCharPath, update_mode: ScpUpdateMode) {
        self.mutated = true;
        self.term.set_scp(char_path, update_mode);
    }
}
