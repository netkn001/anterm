//! Client-side logical-line reflow for zero-PC-interference mobile viewing.
//!
//! Unwraps soft-wrapped rows (marked with `Flags::WRAPLINE` by `alacritty_terminal`)
//! back into logical lines and re-wraps them cleanly to the phone's target column
//! width purely on the mobile client — never issuing `TIOCSWINSZ` to the host PTY.

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar as _;

/// One visual line produced by client-side reflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflowedLine {
    /// Logical line index from the top of the captured buffer.
    pub logical_index: usize,
    /// True if this row is a continuation wrapped purely on the mobile client.
    pub is_continuation: bool,
    /// Plain UTF-8 text of this visual row.
    pub text: String,
}

/// Extract logical lines from `term` (scrollback tail + visible screen) and
/// re-wrap them locally to `target_cols` without mutating the terminal or PTY.
pub fn reflow_terminal<T>(
    term: &Term<T>,
    target_cols: usize,
    max_logical_lines: usize,
) -> Vec<ReflowedLine> {
    let target_cols = target_cols.max(8);
    let cols = term.columns();
    let grid = term.grid();
    let topmost = grid.topmost_line().0;
    let bottommost = grid.bottommost_line().0;

    // Assemble logical lines by joining consecutive rows ending in WRAPLINE.
    let mut logical_lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for y in topmost..=bottommost {
        let line = Line(y);
        let mut row_wrapped = false;
        for col in 0..cols {
            let cell = &grid[Point::new(line, Column(col))];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            if col + 1 == cols && cell.flags.contains(Flags::WRAPLINE) {
                row_wrapped = true;
            }
            current.push(cell.c);
            if let Some(extra) = cell.zerowidth() {
                for &zw in extra {
                    current.push(zw);
                }
            }
        }
        if !row_wrapped {
            let trimmed = current.trim_end_matches(' ').to_string();
            logical_lines.push(trimmed);
            current.clear();
        }
    }
    if !current.is_empty() {
        logical_lines.push(current.trim_end_matches(' ').to_string());
    }

    // Drop trailing empty viewport padding rows while keeping at least one line.
    while logical_lines.len() > 1 && logical_lines.last().is_some_and(String::is_empty) {
        logical_lines.pop();
    }
    if logical_lines.len() > max_logical_lines {
        let keep_from = logical_lines.len() - max_logical_lines;
        logical_lines.drain(..keep_from);
    }

    let mut out = Vec::with_capacity(logical_lines.len());
    for (idx, logical) in logical_lines.iter().enumerate() {
        wrap_logical_line(idx, logical, target_cols, &mut out);
    }
    out
}

fn wrap_logical_line(
    logical_index: usize,
    text: &str,
    target_cols: usize,
    out: &mut Vec<ReflowedLine>,
) {
    if text.is_empty() {
        out.push(ReflowedLine {
            logical_index,
            is_continuation: false,
            text: String::new(),
        });
        return;
    }
    let mut current = String::new();
    let mut width = 0usize;
    let mut continuation = false;

    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > target_cols && !current.is_empty() {
            out.push(ReflowedLine {
                logical_index,
                is_continuation: continuation,
                text: std::mem::take(&mut current),
            });
            width = 0;
            continuation = true;
        }
        current.push(ch);
        width += cw;
    }
    if !current.is_empty() {
        out.push(ReflowedLine {
            logical_index,
            is_continuation: continuation,
            text: current,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn unwraps_desktop_wide_line_and_rewraps_for_narrow_phone() {
        // Desktop terminal is 20 cols wide; write a 26-char logical line so
        // alacritty sets WRAPLINE on row 0, then reflow locally to 10 cols.
        let mut term = Term::new(Config::default(), &TermSize::new(20, 6), VoidListener);
        let mut proc: Processor = Processor::new();
        proc.advance(&mut term, b"abcdefghijklmnopqrstuvwxyz\r\nOK");

        let rows = reflow_terminal(&term, 10, 100);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].text, "abcdefghij");
        assert!(!rows[0].is_continuation);
        assert_eq!(rows[1].text, "klmnopqrst");
        assert!(rows[1].is_continuation);
        assert_eq!(rows[2].text, "uvwxyz");
        assert!(rows[2].is_continuation);
        assert_eq!(rows[3].text, "OK");
        assert!(!rows[3].is_continuation);
        // Original Term dimensions remain completely untouched.
        assert_eq!(term.columns(), 20);
    }

    #[test]
    fn cjk_wide_characters_never_overflow_target_cols() {
        let mut term = Term::new(Config::default(), &TermSize::new(40, 4), VoidListener);
        let mut proc: Processor = Processor::new();
        proc.advance(&mut term, "你好世界测试".as_bytes());
        // 6 CJK chars = 12 columns; at target_cols = 8, first row holds 4 chars (8 cols).
        let rows = reflow_terminal(&term, 8, 100);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].text, "你好世界");
        assert_eq!(rows[1].text, "测试");
    }
}
