//! Reflows a pane's grid without stranding the prompt the shell is about to
//! redraw (#654).
//!
//! A shell sitting at its prompt answers a `SIGWINCH` by redrawing the prompt
//! relative to where it left the cursor: zsh and bash move up as many rows as
//! the prompt's last line and the command being edited took at the width they
//! last drew at, return to column 0, and clear everything below. A column
//! change reflows the grid underneath that arithmetic. A prompt whose right
//! side sits near the edge — an `RPROMPT` clock, a right-aligned segment —
//! wraps onto a second row when the pane narrows by two columns or more, the
//! grid keeps the cursor on the row holding the wrapped tail, and the redraw
//! starts there: the head of the old prompt stays behind, still marked as
//! wrapping into the new one. Every such step of a window drag leaves another,
//! and a later widening joins them into one long line of prompt fragments
//! across the pane — the shell and the grid agreed on the width the whole time.
//!
//! So a reflow at a prompt hands the prompt back first, the way kitty does for
//! a shell with integration: the cursor's line, from its first row down, is
//! the shell's to repaint and is cleared, and after the reflow the cursor is
//! put back as many rows below that line's start as it was before — the count
//! the shell is about to move up by. A second reflow that lands before the
//! shell has redrawn (a drag outrunning the prompt) finds those rows blank and
//! unwrapped, so it leaves them where they are and the count still holds.
//!
//! Only the cursor's own line is handed back. A multi-line prompt's upper
//! lines end in a newline, not a wrap, so they reflow like any other output;
//! they are also the rows the shell redraws without being told where they are.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions as _, Grid, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{MIN_COLUMNS, Term, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, Handler as _};

use crate::backend::size::TermSize;

/// How [`resize`] reconciles the terminal grid with the PTY behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    /// Standard Unix/PTY resize: `alacritty_terminal` pulls scrollback into
    /// newly added rows when growing taller, and hands back a wrapped shell
    /// prompt before reflowing when `shell_redraws` is true.
    Standard { shell_redraws: bool },
    /// Windows ConPTY resize: conhost anchors its viewport at row 0 and never
    /// pulls emulator scrollback back onto the screen when growing taller or
    /// unwrapping wrapped rows. Keep scrollback in history and leave newly
    /// freed viewport rows blank at the bottom so ConPTY's absolute cursor
    /// coordinates stay aligned with the grid.
    Conpty,
}

/// Resizes `term` to `size` according to `mode`.
pub fn resize<T: EventListener>(term: &mut Term<T>, size: TermSize, mode: ResizeMode) {
    match mode {
        ResizeMode::Conpty => resize_conpty(term, size),
        ResizeMode::Standard { shell_redraws } => resize_standard(term, size, shell_redraws),
    }
}

fn resize_standard<T: EventListener>(term: &mut Term<T>, size: TermSize, shell_redraws: bool) {
    // Rows alone reflow nothing, and the alternate screen is a full-screen
    // program's, which repaints the whole of it.
    let reflows = size.cols.max(MIN_COLUMNS) != term.columns();
    if !shell_redraws || !reflows || term.mode().contains(TermMode::ALT_SCREEN) {
        term.resize(size);
        return;
    }

    let cursor = term.grid().cursor.point;
    // Stops at the top of the screen: the shell's cursor-up stops there too,
    // so rows already in scrollback are not part of what it will move over.
    let start = wrapped_line_start(term, cursor.line);
    let rows_up = (cursor.line.0 - start.0) as usize;

    term.grid_mut().cursor.point = Point::new(start, Column(0));
    term.grid_mut().cursor.input_needs_wrap = false;
    term.clear_screen(ClearMode::Below);
    term.resize(size);

    // Linefeeds rather than a jump: a line that sat low on the screen has to
    // scroll the output above it up, as the shell's own layout would have.
    for _ in 0..rows_up {
        term.linefeed();
    }
    let last = Column(term.columns() - 1);
    term.grid_mut().cursor.point.column = cursor.column.min(last);
}

/// Resizes a Windows ConPTY primary grid without pulling scrollback lines down
/// into the viewport.
///
/// ConPTY owns an internal `W×H` screen buffer anchored at top row 0 and emits
/// absolute row jumps (`ESC[row;colH]`) relative to that buffer. When the pane
/// grows taller or widens and unwraps wrapped viewport lines, ConPTY leaves
/// row 0 pinned at the top and pads blank rows at the bottom — it cannot see
/// (and never pulls back) scrollback lines the terminal emulator already
/// scrolled off screen (including output archived by `SCROLL_RESTORED_AWAY`
/// after a restart). `alacritty_terminal::Grid::grow_lines` and
/// `Grid::grow_columns` do the opposite, pulling scrollback lines onto rows
/// `0..pulled` and shifting the live prompt down so ConPTY's next absolute
/// jump overwrites old output mid-screen.
fn resize_conpty<T: EventListener>(term: &mut Term<T>, size: TermSize) {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        term.resize(size);
        return;
    }

    let old_cols = term.columns();
    let old_rows = term.screen_lines();
    let target_cols = size.columns();
    let target_rows = size.screen_lines();

    if old_cols == target_cols && old_rows == target_rows {
        return;
    }

    // Apply width changes at the old height first so column reflow and height
    // growth stay decoupled.
    if target_cols != old_cols {
        if target_cols > old_cols {
            widen_columns_anchored(term, target_cols);
        } else {
            term.resize(TermSize::new(target_cols, old_rows));
        }
    }

    // Apply height changes at `target_cols`. Shrinking never pulls from
    // scrollback; growing must keep scrollback in history and add blank rows
    // at the bottom.
    if target_rows != old_rows {
        if target_rows > old_rows {
            grow_lines_anchored(term, target_rows);
        } else {
            term.resize(TermSize::new(target_cols, target_rows));
        }
    }
}

fn grow_lines_anchored<T: EventListener>(term: &mut Term<T>, target_rows: usize) {
    let cols = term.columns();
    let old_rows = term.screen_lines();
    let old_history = term.grid().history_size();
    let pulled = old_history.min(target_rows - old_rows);
    if pulled == 0 {
        term.resize(TermSize::new(cols, target_rows));
        return;
    }

    let old_display_offset = term.grid().display_offset();
    let old_vi_cursor = term.vi_mode_cursor.point;
    let old_selection = term.selection.take();

    term.resize(TermSize::new(cols, target_rows));

    let region = Line(0)..Line(target_rows as i32);
    {
        let grid = term.grid_mut();
        let template = std::mem::take(&mut grid.cursor.template);
        grid.scroll_up(&region, pulled);
        grid.cursor.template = template;
        grid.cursor.point.line = Line((grid.cursor.point.line.0 - pulled as i32).max(0));
        grid.saved_cursor.point.line =
            Line((grid.saved_cursor.point.line.0 - pulled as i32).max(0));
        grid.scroll_display(Scroll::Bottom);
        if old_display_offset > 0 {
            let delta = i32::try_from(old_display_offset).unwrap_or(i32::MAX);
            grid.scroll_display(Scroll::Delta(delta));
        }
    }

    let viewport_top = Line(-(term.grid().display_offset() as i32));
    let viewport_bottom = viewport_top + term.bottommost_line();
    term.vi_mode_cursor.point.line = old_vi_cursor.line.clamp(viewport_top, viewport_bottom);
    term.vi_mode_cursor.point.column = old_vi_cursor.column.min(term.last_column());
    term.selection = old_selection;
}

fn widen_columns_anchored<T: EventListener>(term: &mut Term<T>, target_cols: usize) {
    let old_cols = term.columns();
    let rows = term.screen_lines();
    let old_history = term.grid().history_size();

    // Without scrollback, or without wrapped lines inside the viewport,
    // `Grid::grow_columns` cannot pull any scrollback row onto the viewport.
    // If a single logical line already wraps across the scrollback/viewport
    // seam (`Line(-1)` -> `Line(0)`), let Alacritty unwrap it naturally.
    if old_history == 0
        || term.grid()[Line(-1)][Column(old_cols - 1)]
            .flags
            .contains(Flags::WRAPLINE)
    {
        term.resize(TermSize::new(target_cols, rows));
        return;
    }

    let any_viewport_wrap = (0..rows).any(|line| {
        term.grid()[Line(line as i32)][Column(old_cols - 1)]
            .flags
            .contains(Flags::WRAPLINE)
    });
    if !any_viewport_wrap {
        term.resize(TermSize::new(target_cols, rows));
        return;
    }

    // Measure the exact number of rows `alacritty_terminal`'s own
    // `grow_columns` will pull from scrollback into the viewport by running it
    // on a viewport-sized probe backed by `rows` unwrapped sentinel history
    // lines.
    let mut probe: Grid<Cell> = Grid::new(rows, old_cols, rows);
    probe.scroll_up(&(Line(0)..Line(rows as i32)), rows);
    for line in 0..rows {
        let src = &term.grid()[Line(line as i32)];
        let dst = &mut probe[Line(line as i32)];
        for col in 0..old_cols {
            dst[Column(col)] = src[Column(col)].clone();
        }
    }
    probe.cursor = term.grid().cursor.clone();
    probe.resize(true, rows, target_cols);
    let viewport_pulled = rows.saturating_sub(probe.history_size());
    if viewport_pulled == 0 {
        term.resize(TermSize::new(target_cols, rows));
        return;
    }

    // Every hard-broken logical line in scrollback takes at least one physical
    // row after widening. Only when scrollback has fewer logical lines than
    // `viewport_pulled` do we need to measure the exact post-widening
    // scrollback height so we never scroll back more rows than existed.
    let mut logical_history = 0usize;
    for h in 1..=old_history {
        if !term.grid()[Line(-(h as i32))][Column(old_cols - 1)]
            .flags
            .contains(Flags::WRAPLINE)
        {
            logical_history += 1;
            if logical_history >= viewport_pulled {
                break;
            }
        }
    }
    let pulled = if logical_history >= viewport_pulled {
        viewport_pulled
    } else {
        let mut probe_h: Grid<Cell> = Grid::new(1, old_cols, old_history);
        for h in (1..=old_history).rev() {
            let src = &term.grid()[Line(-(h as i32))];
            let dst = &mut probe_h[Line(0)];
            for col in 0..old_cols {
                dst[Column(col)] = src[Column(col)].clone();
            }
            probe_h.scroll_up(&(Line(0)..Line(1)), 1);
        }
        probe_h.resize(true, 1, target_cols);
        probe_h.history_size().min(viewport_pulled)
    };

    term.resize(TermSize::new(target_cols, rows));

    if pulled > 0 {
        let region = Line(0)..Line(rows as i32);
        {
            let grid = term.grid_mut();
            let template = std::mem::take(&mut grid.cursor.template);
            grid.scroll_up(&region, pulled);
            grid.cursor.template = template;
            grid.cursor.point.line = Line((grid.cursor.point.line.0 - pulled as i32).max(0));
            let max_line = Line(rows as i32 - 1);
            grid.saved_cursor.point.line = grid.saved_cursor.point.line.min(max_line);
        }
        let viewport_top = Line(-(term.grid().display_offset() as i32));
        let viewport_bottom = viewport_top + term.bottommost_line();
        term.vi_mode_cursor.point.line = term
            .vi_mode_cursor
            .point
            .line
            .clamp(viewport_top, viewport_bottom);
    }
}

/// The two prompt marks the shell brackets its prompt with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMark {
    /// OSC 133;A — a fresh prompt is about to be drawn.
    Opened,
    /// OSC 133;B — the prompt is drawn and input begins here.
    Ready,
}

impl PromptMark {
    /// Reads an OSC payload (`133;A`, `133;B`, …).
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let mark = payload.strip_prefix(b"133;")?;
        let (&kind, rest) = mark.split_first()?;
        if !(rest.is_empty() || rest.first() == Some(&b';')) {
            return None;
        }
        match kind {
            b'A' => Some(Self::Opened),
            b'B' => Some(Self::Ready),
            _ => None,
        }
    }
}

/// Puts the line break back between output that did not end in a newline and
/// the prompt after it.
///
/// zsh's `PROMPT_SP` moves such a prompt onto a line of its own by writing
/// the `%` end-of-line mark and then padding it with spaces until the cursor
/// *wraps*. To the grid that is one wrapped line: `printf xyz` and the prompt
/// under it read as a single line of text, so the reflow above took the
/// output for the prompt's own first row and erased it on the next resize,
/// and widening the pane joined the two onto one row.
///
/// The marks say where that happened. `A` comes from precmd, before
/// `PROMPT_SP`: a cursor off column 0 there is output left dangling. By `B`
/// the prompt is drawn, and if the row that output ended on is now padding
/// all the way to its last column and wraps into the rows below, that wrap is
/// the padding's, not the text's — so it becomes a hard break. A prompt that
/// simply carried on from the output (bash has no `PROMPT_SP`) leaves its own
/// text on that row rather than blanks, and is left alone.
#[derive(Default)]
pub struct PromptBreak {
    /// Where `A` found a dangling line: how many rows into its wrapped line
    /// the cursor was, the column, and the character it had just written —
    /// the last to recognize the row by again at `B`, which the prompt may
    /// have scrolled.
    dangling: Option<(usize, Column, char)>,
}

impl PromptBreak {
    pub fn apply<T: EventListener>(&mut self, term: &mut Term<T>, mark: PromptMark) {
        match mark {
            PromptMark::Opened => {
                let cursor = term.grid().cursor.point;
                // A full row leaves the cursor parked on its last cell: the
                // text ends there, one column short of where it stands.
                let end = match term.grid().cursor.input_needs_wrap {
                    true => cursor.column + 1,
                    false => cursor.column,
                };
                self.dangling = (end.0 > 0).then(|| {
                    let into = (cursor.line.0 - wrapped_line_start(term, cursor.line).0) as usize;
                    let last = term.grid()[cursor.line][end - 1].c;
                    (into, end, last)
                });
            }
            PromptMark::Ready => {
                let Some((into, end, last)) = self.dangling.take() else {
                    return;
                };
                let cursor = term.grid().cursor.point.line;
                let row = wrapped_line_start(term, cursor) + into;
                if row >= cursor {
                    return;
                }
                let cols = term.columns();
                let grid = term.grid();
                let padded = end.0 < cols
                    && grid[row][end - 1].c == last
                    && (end.0 + 1..cols).all(|col| grid[row][Column(col)].c == ' ')
                    && grid[row][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
                if padded {
                    term.grid_mut()[row][Column(cols - 1)]
                        .flags
                        .remove(Flags::WRAPLINE);
                }
            }
        }
    }
}

/// The first row of the wrapped line `line` is part of, stopping at the top of
/// the screen as the shell's own cursor-up does.
fn wrapped_line_start<T>(term: &Term<T>, line: Line) -> Line {
    let last = Column(term.columns() - 1);
    let mut start = line;
    while start > Line(0)
        && term.grid()[Line(start.0 - 1)][last]
            .flags
            .contains(Flags::WRAPLINE)
    {
        start -= 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    const PRIOR: &[u8] = b"echo hi\r\nhi\r\n";
    const CLOCK: &str = "16:46:33";

    fn pane(cols: usize) -> (Term<VoidListener>, Processor) {
        let mut term = Term::new(Config::default(), &TermSize::new(cols, 12), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, PRIOR);
        (term, parser)
    }

    /// What zsh 5.9 writes on `SIGWINCH` for `PROMPT='[~] '` with an
    /// `RPROMPT` clock, as captured off a real pty: up the rows it last drew
    /// above the cursor, back to column 0, clear below, and lay the prompt out
    /// for `cols` — the clock right-aligned one column short of the edge, or
    /// dropped when a long command line leaves it no room.
    fn zsh_redraw(cols: usize, rows_up: usize, buffer: &str) -> Vec<u8> {
        let mut out = "\x1b[A".repeat(rows_up);
        out.push_str("\r\r\x1b[0m\x1b[J[~] ");
        if buffer.is_empty() {
            out.push_str(&format!(
                "\x1b[K\x1b[{}C{CLOCK}\x1b[{}D",
                cols - 13,
                cols - 5
            ));
        } else {
            out.push_str(buffer);
            out.push_str("\x1b[K");
        }
        out.into_bytes()
    }

    /// Every non-blank row, scrollback included, trailing blanks trimmed.
    fn text(term: &Term<VoidListener>) -> Vec<String> {
        let top = -(term.grid().history_size() as i32);
        (top..term.screen_lines() as i32)
            .map(|line| {
                let row = &term.grid()[Line(line)];
                (0..term.columns())
                    .map(|col| row[Column(col)].c)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .filter(|row| !row.is_empty())
            .collect()
    }

    fn prompt_row(cols: usize) -> String {
        format!("[~]{}{CLOCK}", " ".repeat(cols - 12))
    }

    fn standard(shell_redraws: bool) -> ResizeMode {
        ResizeMode::Standard { shell_redraws }
    }

    fn viewport_row(term: &Term<VoidListener>, line: i32) -> String {
        let row = &term.grid()[Line(line)];
        (0..term.columns())
            .map(|col| row[Column(col)].c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// The reported pattern: a clock prompt redrawn after each step of a drag
    /// that narrows the pane by more than one column at a time, then widens it
    /// back. Measured against the unguarded resize, so the test also proves it
    /// reproduces the fragments rather than passing on a scenario that never
    /// produced any.
    #[test]
    fn a_prompt_narrowed_under_its_right_side_leaves_no_fragments() {
        let drag = |guarded: bool| {
            let (mut term, mut parser) = pane(98);
            parser.advance(&mut term, &zsh_redraw(98, 0, ""));
            for cols in [95, 92, 89, 86, 89, 92, 95, 98] {
                if guarded {
                    resize(&mut term, TermSize::new(cols, 12), standard(true));
                } else {
                    term.resize(TermSize::new(cols, 12));
                }
                parser.advance(&mut term, &zsh_redraw(cols, 0, ""));
            }
            text(&term)
        };

        let unguarded = drag(false);
        assert!(
            unguarded.iter().filter(|row| row.contains("[~]")).count() > 1,
            "the scenario no longer strands a prompt without the guard: {unguarded:#?}"
        );
        assert_eq!(drag(true), ["echo hi", "hi", &prompt_row(98)]);
    }

    /// A command line long enough to wrap: zsh moves up by the rows it took at
    /// the old width, which the reflow changes (two rows at 98 columns, three
    /// at 60). Landing one row off either way strands a row of the old line or
    /// clears a row of the output above it.
    #[test]
    fn a_wrapped_command_line_is_redrawn_in_place() {
        let buffer = "x".repeat(150);
        let (mut term, mut parser) = pane(98);
        parser.advance(&mut term, &zsh_redraw(98, 0, &buffer));

        resize(&mut term, TermSize::new(60, 12), standard(true));
        parser.advance(&mut term, &zsh_redraw(60, 1, &buffer));
        assert_eq!(
            text(&term),
            [
                "echo hi".to_string(),
                "hi".to_string(),
                format!("[~] {}", "x".repeat(56)),
                "x".repeat(60),
                "x".repeat(34),
            ]
        );

        resize(&mut term, TermSize::new(98, 12), standard(true));
        parser.advance(&mut term, &zsh_redraw(98, 2, &buffer));
        assert_eq!(
            text(&term),
            [
                "echo hi".to_string(),
                "hi".to_string(),
                format!("[~] {}", "x".repeat(94)),
                "x".repeat(56),
            ]
        );
    }

    /// A drag can outrun the shell: a second reflow lands before the redraw
    /// for the first, on a line the first one already cleared. The shell still
    /// moves up by the rows of the layout it drew last, which the cleared rows
    /// above the cursor still are.
    #[test]
    fn back_to_back_reflows_before_the_redraw_strand_nothing() {
        let (mut term, mut parser) = pane(98);
        parser.advance(&mut term, &zsh_redraw(98, 0, ""));
        resize(&mut term, TermSize::new(92, 12), standard(true));
        resize(&mut term, TermSize::new(86, 12), standard(true));
        parser.advance(&mut term, &zsh_redraw(86, 0, ""));
        assert_eq!(text(&term), ["echo hi", "hi", &prompt_row(86)]);

        let buffer = "x".repeat(150);
        let (mut term, mut parser) = pane(98);
        parser.advance(&mut term, &zsh_redraw(98, 0, &buffer));
        resize(&mut term, TermSize::new(60, 12), standard(true));
        resize(&mut term, TermSize::new(80, 12), standard(true));
        parser.advance(&mut term, &zsh_redraw(80, 1, &buffer));
        assert_eq!(
            text(&term),
            [
                "echo hi".to_string(),
                "hi".to_string(),
                format!("[~] {}", "x".repeat(76)),
                "x".repeat(74),
            ]
        );
    }

    /// Low on the screen, putting the cursor back below the cleared line means
    /// scrolling: the shell's layout had those rows, so the output above moves
    /// up rather than the redraw climbing into it.
    #[test]
    fn a_line_at_the_bottom_scrolls_the_output_above_it() {
        let buffer = "x".repeat(150);
        let mut term = Term::new(Config::default(), &TermSize::new(98, 4), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"one\r\ntwo\r\n");
        parser.advance(&mut term, &zsh_redraw(98, 0, &buffer));
        assert_eq!(term.grid().cursor.point.line, Line(3));

        resize(&mut term, TermSize::new(60, 4), standard(true));
        parser.advance(&mut term, &zsh_redraw(60, 1, &buffer));
        assert_eq!(
            text(&term),
            [
                "one".to_string(),
                "two".to_string(),
                format!("[~] {}", "x".repeat(56)),
                "x".repeat(60),
                "x".repeat(34),
            ]
        );
    }

    /// Away from a prompt nothing is going to repaint the line, so it reflows
    /// like any output — as does a prompt whose width did not change.
    #[test]
    fn only_a_column_change_at_a_prompt_is_handed_back() {
        let (mut term, mut parser) = pane(98);
        parser.advance(&mut term, "y".repeat(150).as_bytes());
        resize(&mut term, TermSize::new(60, 12), standard(false));
        resize(&mut term, TermSize::new(98, 12), standard(false));
        assert_eq!(
            text(&term),
            [
                "echo hi".to_string(),
                "hi".to_string(),
                "y".repeat(98),
                "y".repeat(52)
            ]
        );

        let (mut term, mut parser) = pane(98);
        parser.advance(&mut term, &zsh_redraw(98, 0, ""));
        resize(&mut term, TermSize::new(98, 8), standard(true));
        assert_eq!(text(&term), ["echo hi", "hi", &prompt_row(98)]);
    }

    /// The alternate screen belongs to a full-screen program, which repaints
    /// all of it; a stale prompt flag must not blank it.
    #[test]
    fn the_alternate_screen_is_left_alone() {
        let (mut term, mut parser) = pane(60);
        parser.advance(&mut term, b"\x1b[?1049h\x1b[Hstatus bar");
        resize(&mut term, TermSize::new(98, 12), standard(true));
        assert_eq!(text(&term), ["status bar"]);
    }

    /// What zsh 5.9 writes between precmd's `A` and the prompt when the last
    /// output did not end in a newline, as captured off a real pty: the
    /// standout `%` end-of-line mark, `cols - 1` spaces to force a wrap, then
    /// back to column 0 and clear below.
    fn prompt_sp(cols: usize) -> Vec<u8> {
        let mut out = b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m".to_vec();
        out.extend(std::iter::repeat_n(b' ', cols - 1));
        out.extend_from_slice(b"\r \r\r\x1b[0m\x1b[27m\x1b[24m\x1b[J");
        out
    }

    /// `printf xyz` at a zsh prompt, through a stream the reader has cut at
    /// the prompt marks, then drawn out at `cols`.
    fn dangling_output(cols: usize, guarded: bool) -> (Term<VoidListener>, Processor) {
        let (mut term, mut parser) = pane(cols);
        let mut marks = PromptBreak::default();
        parser.advance(&mut term, b"xyz");
        if guarded {
            marks.apply(&mut term, PromptMark::Opened);
        }
        parser.advance(&mut term, &prompt_sp(cols));
        parser.advance(&mut term, &zsh_redraw(cols, 0, ""));
        if guarded {
            marks.apply(&mut term, PromptMark::Ready);
        }
        (term, parser)
    }

    /// `PROMPT_SP` wraps the dangling output's row into the prompt's. Taken
    /// for the prompt's own first row, that output was cleared by the next
    /// resize at the prompt — `printf xyz` simply vanished when a panel
    /// opened. Measured against the stream without the marks' fix, so the
    /// test also proves the scenario loses the output there.
    #[test]
    fn output_without_a_newline_survives_a_resize_at_the_prompt() {
        let narrow = |guarded: bool| {
            let (mut term, mut parser) = dangling_output(98, guarded);
            resize(&mut term, TermSize::new(60, 12), standard(true));
            parser.advance(&mut term, &zsh_redraw(60, 0, ""));
            text(&term)
        };
        assert!(
            !narrow(false).iter().any(|row| row.starts_with("xyz")),
            "the scenario no longer loses the output without the fix"
        );
        assert_eq!(narrow(true), ["echo hi", "hi", "xyz%", &prompt_row(60)]);
    }

    /// Widening joins wrapped rows back together, and the padding's wrap
    /// joined the output and the prompt onto one row.
    #[test]
    fn widening_keeps_dangling_output_off_the_prompt_row() {
        let (mut term, mut parser) = dangling_output(60, true);
        resize(&mut term, TermSize::new(98, 12), standard(true));
        parser.advance(&mut term, &zsh_redraw(98, 0, ""));
        assert_eq!(text(&term), ["echo hi", "hi", "xyz%", &prompt_row(98)]);
    }

    /// A prompt that carries straight on from the output — bash has no
    /// `PROMPT_SP` — wraps on its own text, not on padding, and that wrap is
    /// the shell's to count.
    #[test]
    fn a_prompt_continuing_the_output_row_keeps_its_wrap() {
        let (mut term, mut parser) = pane(20);
        let mut marks = PromptBreak::default();
        parser.advance(&mut term, b"xyz");
        marks.apply(&mut term, PromptMark::Opened);
        parser.advance(&mut term, b"user@host:~/some/long/path$ ");
        marks.apply(&mut term, PromptMark::Ready);
        let row: Line = term.grid().cursor.point.line - 1;
        assert!(term.grid()[row][Column(19)].flags.contains(Flags::WRAPLINE));
    }

    #[test]
    fn prompt_marks_parse() {
        assert_eq!(PromptMark::parse(b"133;A"), Some(PromptMark::Opened));
        assert_eq!(PromptMark::parse(b"133;B"), Some(PromptMark::Ready));
        assert_eq!(PromptMark::parse(b"133;A;k=s"), Some(PromptMark::Opened));
        assert_eq!(PromptMark::parse(b"133;C"), None);
        assert_eq!(PromptMark::parse(b"133;AB"), None);
    }

    /// Windows ConPTY restore/resize reproduction: restored output is scrolled
    /// into history (`ESC[2J ESC[H`), the fresh shell draws its prompt at rows
    /// 0..1, and then the pane grows taller (with or without a width change).
    /// Standard resize pulls old scrollback rows back into the viewport so
    /// ConPTY's next absolute jump (`ESC[1;1H` / `ESC[2;1H`) overwrites old
    /// output mid-screen; ConPTY resize must keep scrollback in history and
    /// keep the live prompt anchored at row 0.
    #[test]
    fn conpty_height_growth_keeps_scrollback_out_of_viewport() {
        let run = |mode: ResizeMode, target: TermSize| {
            let mut term = Term::new(Config::default(), &TermSize::new(80, 12), VoidListener);
            let mut parser: Processor = Processor::new();
            for i in 0..10 {
                parser.advance(&mut term, format!("old output line {i}\r\n").as_bytes());
            }
            // Same clear-and-home preamble `SCROLL_RESTORED_AWAY` emits on Windows.
            parser.advance(&mut term, b"\x1b[2J\x1b[H");
            let history_before = term.grid().history_size();
            assert!(history_before >= 10);

            // Fresh shell draws a two-line prompt starting at ConPTY row 1 (Line 0).
            parser.advance(&mut term, b"C:\\data\\userdata\\tmp\r\n");
            assert_eq!(term.grid().cursor.point.line, Line(1));

            resize(&mut term, target, mode);

            // ConPTY edits the live command on row 2 (Line 1) using absolute CUP.
            parser.advance(&mut term, b"\x1b[2;1Hbkit open");
            (term, history_before)
        };

        // Without the ConPTY fix, standard resize pulls history onto row 0 so
        // ConPTY's `ESC[2;1H` lands right on top of old restored output.
        let (standard_term, _) = run(standard(false), TermSize::new(80, 20));
        assert!(
            viewport_row(&standard_term, 0).starts_with("old output"),
            "standard resize must still pull scrollback into viewport"
        );
        assert!(
            viewport_row(&standard_term, 1).contains("bkit open"),
            "standard resize reproduces ConPTY overwriting old output on row 1"
        );

        // With `ResizeMode::Conpty` (height-only growth AND width+height growth):
        for target in [TermSize::new(80, 20), TermSize::new(120, 20)] {
            let (term, history_before) = run(ResizeMode::Conpty, target);
            assert_eq!(term.grid().history_size(), history_before);
            assert_eq!(viewport_row(&term, 0), "C:\\data\\userdata\\tmp");
            assert_eq!(viewport_row(&term, 1), "bkit open");
            assert_eq!(term.grid().cursor.point.line, Line(1));
            for row in 2..target.rows as i32 {
                assert!(
                    viewport_row(&term, row).is_empty(),
                    "row {row} should stay blank at {target:?}, got {:?}",
                    viewport_row(&term, row)
                );
            }
            assert_eq!(viewport_row(&term, -1), "old output line 9");
        }
    }

    /// ConPTY height growth while the user is scrolled up, has an active
    /// selection, and the terminal has a non-default background template must
    /// keep scroll position, selection, saved cursor, and blank cell styling
    /// intact.
    #[test]
    fn conpty_height_growth_preserves_scroll_offset_selection_and_default_blanks() {
        use alacritty_terminal::index::Side;
        use alacritty_terminal::selection::{Selection, SelectionType};
        use alacritty_terminal::vte::ansi::{Color, NamedColor};

        let mut term = Term::new(Config::default(), &TermSize::new(60, 8), VoidListener);
        let mut parser: Processor = Processor::new();
        for i in 0..12 {
            parser.advance(&mut term, format!("history {i}\r\n").as_bytes());
        }
        parser.advance(&mut term, b"\x1b[2J\x1b[H");
        // Set a blue background and save cursor on row 1.
        parser.advance(&mut term, b"prompt\r\n\x1b[44m\x1b7input");
        let saved_before = term.grid().saved_cursor.point;
        let history_before = term.grid().history_size();

        term.scroll_display(Scroll::Delta(4));
        assert_eq!(term.grid().display_offset(), 4);

        let sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        term.selection = Some(sel.clone());

        resize(&mut term, TermSize::new(60, 14), ResizeMode::Conpty);

        assert_eq!(term.grid().history_size(), history_before);
        assert_eq!(term.grid().display_offset(), 4);
        assert_eq!(term.grid().saved_cursor.point, saved_before);
        assert_eq!(term.selection, Some(sel));
        assert_eq!(viewport_row(&term, 0), "prompt");
        assert_eq!(viewport_row(&term, 1), "input");
        // Newly added bottom rows must use default background, not SGR 44 blue.
        assert_eq!(
            term.grid()[Line(13)][Column(0)].bg,
            Color::Named(NamedColor::Background)
        );
        // Active cursor template keeps SGR 44 blue for subsequent writes.
        assert_eq!(
            term.grid().cursor.template.bg,
            Color::Named(NamedColor::Blue)
        );
    }

    /// Widening a ConPTY pane that has a wrapped line in its viewport must not
    /// pull scrollback rows onto row 0 — both when scrollback is large and
    /// when scrollback has fewer lines than the unwrapped viewport rows.
    #[test]
    fn conpty_widening_wrapped_viewport_keeps_scrollback_in_history() {
        for history_lines in [20usize, 1usize] {
            let mut term = Term::new(Config::default(), &TermSize::new(40, 8), VoidListener);
            let mut parser: Processor = Processor::new();
            for i in 0..history_lines {
                parser.advance(&mut term, format!("hist-{i}\r\n").as_bytes());
            }
            parser.advance(&mut term, b"\x1b[2J\x1b[H");
            let history_before = term.grid().history_size();

            // A 95-char line wraps across 3 rows at 40 cols and collapses to 1
            // row at 120 cols (freeing 2 viewport rows).
            let long_cmd = "a".repeat(95);
            parser.advance(&mut term, format!("prompt\r\n{long_cmd}").as_bytes());
            assert_eq!(term.grid().cursor.point.line, Line(3));

            resize(&mut term, TermSize::new(120, 8), ResizeMode::Conpty);

            assert_eq!(
                term.grid().history_size(),
                history_before,
                "history_lines={history_lines}"
            );
            assert_eq!(
                viewport_row(&term, 0),
                "prompt",
                "history_lines={history_lines}"
            );
            assert_eq!(
                viewport_row(&term, 1),
                long_cmd,
                "history_lines={history_lines}"
            );
            assert_eq!(
                term.grid().cursor.point.line,
                Line(1),
                "history_lines={history_lines}"
            );
            assert!(viewport_row(&term, 2).is_empty());
        }
    }

    /// ConPTY height shrink and alternate-screen resizes keep standard
    /// behavior without clobbering top-anchored content.
    #[test]
    fn conpty_shrink_and_alt_screen_behave_cleanly() {
        let mut term = Term::new(Config::default(), &TermSize::new(60, 12), VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"line0\r\nline1\r\nline2");
        resize(&mut term, TermSize::new(60, 6), ResizeMode::Conpty);
        assert_eq!(viewport_row(&term, 0), "line0");
        assert_eq!(viewport_row(&term, 1), "line1");
        assert_eq!(viewport_row(&term, 2), "line2");
        assert_eq!(term.grid().cursor.point.line, Line(2));

        parser.advance(&mut term, b"\x1b[?1049h\x1b[Halt-top");
        resize(&mut term, TermSize::new(80, 16), ResizeMode::Conpty);
        assert_eq!(viewport_row(&term, 0), "alt-top");
    }
}
