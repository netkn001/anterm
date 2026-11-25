use super::*;

use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{Icon, IconName, h_flex};

use crate::prompt::completion::{self, CandidateKind};
use crate::render::highlight::{self, TokenKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InputCaretPaint {
    Outline,
    Bar,
    Block,
    Underline,
}

pub(super) fn input_caret_paint(
    focused: bool,
    blink_on: bool,
    style: anterm_ui_kit::config::CursorStyle,
) -> Option<InputCaretPaint> {
    use anterm_ui_kit::config::CursorStyle;

    if !focused {
        return Some(InputCaretPaint::Outline);
    }
    blink_on.then_some(match style {
        CursorStyle::Bar => InputCaretPaint::Bar,
        CursorStyle::Block => InputCaretPaint::Block,
        CursorStyle::Underline => InputCaretPaint::Underline,
    })
}

/// The header the link section of the context menu wears: the file's own
/// name, so a menu opened over a long path says which one it is about without
/// making the menu as wide as the path.
pub(super) fn link_menu_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(super) fn menu_row_with_hint(
    label: &'static str,
    key: Option<&'static str>,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyElement {
    move |_window, _cx| {
        let hint = key.map(|k| {
            Kbd::new(gpui::Keystroke::parse_with_os(k, anterm_ui_kit::platform::current_os()).expect("valid static keystroke"))
                .p_0()
                .flex_nowrap()
                .border_0()
                .bg(gpui::transparent_white())
        });
        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .justify_between()
            .child(label)
            .children(hint)
            .into_any_element()
    }
}

#[cfg(target_os = "macos")]
pub(super) fn mac_only(key: &'static str) -> Option<&'static str> {
    Some(key)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn mac_only(_key: &'static str) -> Option<&'static str> {
    None
}

/// A history entry drawn on a one-line row, with its line breaks folded to a
/// visible stand-in.
///
/// gpui breaks text on `\n` whatever `white_space` says, so an entry carrying
/// one paints its tail over whatever sits below it. Entries do carry them: a
/// command composed in the inline editor keeps its newlines in the in-session
/// list, even though `history::append` refuses to write them to disk. Folding
/// is one char for one char so a fuzzy matcher's positions still address the
/// same characters.
fn one_line_char(c: char) -> char {
    match c {
        '\n' => '↵',
        c if c.is_control() => ' ',
        c => c,
    }
}

fn one_line(text: &str) -> String {
    text.chars().map(one_line_char).collect()
}

/// Split `line` into consecutive runs of matched / unmatched characters, each
/// one folded onto a single line by `one_line_char`. `positions` is ascending.
pub(super) fn highlight_runs(line: &str, positions: &[usize]) -> Vec<(String, bool)> {
    let mut runs: Vec<(String, bool)> = Vec::new();
    let mut pos = positions.iter().copied().peekable();
    for (ci, ch) in line.chars().enumerate() {
        let hit = pos.next_if_eq(&ci).is_some();
        let ch = one_line_char(ch);
        match runs.last_mut() {
            Some((run, run_hit)) if *run_hit == hit => run.push(ch),
            _ => runs.push((ch.to_string(), hit)),
        }
    }
    runs
}

/// Columns this character occupies in the input bar.
///
/// The bar lays out text the terminal is about to receive, so it has to agree
/// with the grid on where every character lands. The grid gets its widths from
/// `unicode-width` by way of `alacritty_terminal`, so the bar reads the same
/// table: a hand-written range list drifts from it the moment Unicode assigns
/// another block, and it silently counted `🀄`, `⌚` and every combining mark
/// as one plain column (#701).
///
/// Control characters have no width of their own. The bar still gives them a
/// column — one was typed, and a cell that occupies nothing is a cell nobody
/// can put the caret on.
pub(super) fn display_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(1)
}

/// One drawn cell of the input bar.
#[derive(Debug, PartialEq)]
pub(super) struct InputCell {
    /// Character indices the cell covers, as `start..end`.
    pub(super) start: usize,
    pub(super) end: usize,
    /// What to draw in it — a base character and whatever rides along with it.
    pub(super) text: String,
    /// Columns it occupies. Zero for a newline, which ends the row rather than
    /// taking space on it.
    pub(super) width: usize,
}

/// Splits input-bar text into the cells it draws.
///
/// Combining marks, variation selectors and the joiner inside an emoji
/// sequence take no column of their own, and the shaper has to see them in the
/// same run as their base to compose a single glyph — so they ride in the cell
/// of the character in front of them instead of each getting a box. Handing
/// them their own cell is what left `e` and its accent side by side, and every
/// mark shifted the rest of the row a column left (#701).
///
/// A mark with no base ahead of it — text pasted mid-sequence, an IME's
/// in-flight buffer — keeps a cell and a column, so it stays visible and the
/// caret has somewhere to sit.
pub(super) fn input_cells(chars: &[char]) -> Vec<InputCell> {
    let mut cells: Vec<InputCell> = Vec::with_capacity(chars.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '\n' {
            cells.push(InputCell {
                start: i,
                end: i + 1,
                text: String::new(),
                width: 0,
            });
            continue;
        }
        let w = display_width(ch);
        match cells.last_mut() {
            // `width > 0` keeps a mark off a newline's cell, which ends a row
            // and draws nothing.
            Some(last) if w == 0 && last.width > 0 => {
                last.text.push(ch);
                last.end = i + 1;
                // U+FE0F asks for the emoji glyph, and an emoji presentation
                // sequence is two columns wide even where its base is one — a
                // rule that only exists at string level (UTS #51), which is why
                // scoring character by character misses it. The grid re-scores
                // the sequence the same way (our `alacritty_terminal` fork,
                // #203), so the bar has to as well or `❤️` sits a column
                // narrower here than where it lands. Never narrower than the
                // base: giving a column back would mean pulling the row left.
                if ch == '\u{FE0F}' {
                    let scored = unicode_width::UnicodeWidthStr::width(last.text.as_str());
                    last.width = scored.max(last.width);
                }
            }
            _ => cells.push(InputCell {
                start: i,
                end: i + 1,
                text: ch.to_string(),
                width: w.max(1),
            }),
        }
    }
    cells
}

fn completion_row_icon(
    raw: Option<&str>,
    kind: CandidateKind,
    color: gpui::Hsla,
) -> gpui::AnyElement {
    let slot = |child: gpui::AnyElement| {
        div()
            .w(px(16.))
            .flex()
            .justify_center()
            .items_center()
            .child(child)
            .into_any_element()
    };

    if let Some(raw) = raw {
        if let Some(emoji) = fig_icon_emoji(raw) {
            return slot(
                div()
                    .text_size(px(13.))
                    .child(emoji.to_string())
                    .into_any_element(),
            );
        }
        if let Some(name) = fig_icon_glyph(raw) {
            return slot(
                Icon::new(name)
                    .size(px(15.))
                    .text_color(color)
                    .into_any_element(),
            );
        }
    }

    let name = match kind {
        CandidateKind::Command | CandidateKind::Value => IconName::SquareTerminal,
        CandidateKind::Flag => IconName::Dash,
        CandidateKind::Dir => IconName::Folder,
        CandidateKind::File => IconName::File,
    };
    slot(
        Icon::new(name)
            .size(px(15.))
            .text_color(color)
            .into_any_element(),
    )
}

pub(super) fn fig_icon_emoji(raw: &str) -> Option<&str> {
    if raw.is_empty() {
        None
    } else if !raw.starts_with("fig://") {
        Some(raw)
    } else if raw.starts_with("fig://template") {
        fig_query_param(raw, "badge")
    } else {
        None
    }
}

pub(super) fn fig_icon_glyph(raw: &str) -> Option<IconName> {
    let ty = raw
        .strip_prefix("fig://icon")
        .and_then(|r| fig_query_param(r, "type"))?;
    match ty {
        "folder" => Some(IconName::Folder),
        "file" => Some(IconName::File),
        "git" => Some(IconName::Github),
        "asterisk" => Some(IconName::Asterisk),
        _ => None,
    }
}

fn fig_query_param<'a>(raw: &'a str, key: &str) -> Option<&'a str> {
    raw.split_once('?')?.1.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// Where the completion menu stops growing.
pub(super) const COMPLETION_MENU_MAX_W: f32 = 480.;

/// How many characters of a candidate's description fit beside its name.
///
/// The menu is monospaced and clips whatever runs past its edge, so a long
/// description used to end mid-word — "…or a symli" — with nothing to say it
/// had been cut. The row spends its width on `px_2` either side, the kind
/// icon, the gap after it and the margin before the description; whatever is
/// left, in cells, is the description's.
///
/// `menu_w` is the width the menu actually got, not the width it would like:
/// in a pane narrower than [`COMPLETION_MENU_MAX_W`] the menu is capped to the
/// grid, and a budget measured against the larger number puts the ellipsis
/// past the edge — which is the same mid-word cut, only harder to see.
pub(super) fn description_budget(cell_width: f32, label_cells: usize, menu_w: f32) -> usize {
    const ROW_CHROME: f32 = 8. + 8. + 16. + 6. + 8.;
    if cell_width <= 0. {
        return 0;
    }
    let free = menu_w - ROW_CHROME - label_cells as f32 * cell_width;
    (free / cell_width).floor().max(0.) as usize
}

/// `text` cut to `budget` characters, with the last one spent on an ellipsis.
/// A budget too small to say anything with returns nothing rather than a bare
/// "…", which reads as a description that is there but unreadable.
pub(super) fn elide(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_string();
    }
    if budget < 2 {
        return String::new();
    }
    let mut out: String = text.chars().take(budget - 1).collect();
    out.push('…');
    out
}

pub(super) fn menu_layout(
    total_rows: usize,
    srow: usize,
    count: usize,
    sel: usize,
    max_rows: usize,
) -> (bool, usize, usize) {
    let want = count.min(max_rows);
    let below = total_rows.saturating_sub(srow + 1);
    let above = srow;
    let footers = if count > want { 2 } else { 0 };
    let need = want + footers;
    let (place_above, visible) = if below >= need {
        (false, want)
    } else if above >= need {
        (true, want)
    } else {
        let squeeze = |room: usize| room.saturating_sub(2).max(1);
        if above > below {
            (true, squeeze(above))
        } else {
            (false, squeeze(below))
        }
    };
    let visible = visible.min(count);
    let first = sel
        .saturating_sub(visible.saturating_sub(1))
        .min(count.saturating_sub(visible));
    (place_above, visible, first)
}

/// The fewest columns the input is given beside the prompt, however wide the
/// pane. Below this nearly any command with an argument or two wraps on its
/// first row, and a row that short is harder to read than a row lower down.
const INPUT_MIN_BESIDE_PROMPT: usize = 20;

/// Where the input bar starts, as `(row, column)` with the prompt's own row as
/// row 0: right after the prompt, or — when the prompt has left too little of
/// its row — at column 0 of the row below, with the whole width to itself
/// (#767).
///
/// "Too little" is under a third of the pane, and never under
/// [`INPUT_MIN_BESIDE_PROMPT`]. A third keeps an ordinary prompt on its own
/// line in any pane you would type in (`user@host ~/src/app % ` is about 25
/// columns; an 80-column pane still leaves 55), and only moves the input when
/// a deep path or a busy theme has eaten most of the row — the report behind
/// this had a 119-column prompt in a 143-column pane, 24 columns left. The
/// floor covers narrow panes, where a third is a handful of columns.
///
/// The move also has to gain something: in a pane barely wider than the
/// floor, a short prompt leaves under 20 columns and a new row would hardly
/// give more. So it only happens when the prompt is at least as wide as what
/// it left — the new row at least doubles the room.
pub(super) fn input_start(scol: usize, cols: usize) -> (usize, usize) {
    let left = cols.saturating_sub(scol);
    let floor = INPUT_MIN_BESIDE_PROMPT.max(cols / 3);
    if left < floor && scol >= left {
        (1, 0)
    } else {
        (0, scol)
    }
}

/// Where each character of the input bar lands: `(row, column, width)`, one
/// entry per character, plus the row and column the text ends on. `start` is
/// where the first one goes, from [`input_start`].
///
/// Walks the same cells the bar draws rather than re-deriving widths per
/// character — an emoji presentation sequence is two columns and a stranded
/// combining mark is one, and a second width table would put clicks, wrapping
/// and the caret a column away from the glyph on screen.
///
/// Only the base of a cell carries the width, so a click can never land on a
/// character riding along with it. Those riders are parked at the column the
/// caret takes after the cell, which is where a caret sitting on one belongs.
pub(super) fn input_char_positions(
    chars: &[char],
    start: (usize, usize),
    cols: usize,
) -> (Vec<(usize, usize, usize)>, usize, usize) {
    let mut positions: Vec<(usize, usize, usize)> = Vec::with_capacity(chars.len());
    let (mut r, mut c) = start;
    for cell in input_cells(chars) {
        if chars[cell.start] == '\n' {
            positions.push((r, c, 0));
            r += 1;
            c = 0;
            continue;
        }
        if c + cell.width > cols {
            r += 1;
            c = 0;
        }
        positions.push((r, c, cell.width));
        c += cell.width;
        for _ in cell.start + 1..cell.end {
            positions.push((r, c, 0));
        }
    }
    (positions, r, c)
}

pub(super) fn input_overlay_rows(
    chars: &[char],
    cursor: usize,
    marked: &str,
    start: (usize, usize),
    cols: usize,
) -> (usize, usize) {
    let mut merged: Vec<char> = Vec::with_capacity(chars.len() + marked.len());
    let cursor = cursor.min(chars.len());
    merged.extend_from_slice(&chars[..cursor]);
    merged.extend(marked.chars());
    merged.extend_from_slice(&chars[cursor..]);
    let (positions, r, c) = input_char_positions(&merged, start, cols);
    let end_row = if cursor >= chars.len() && marked.is_empty() && c >= cols {
        r + 1
    } else {
        r
    };
    let caret_vrow = positions.get(cursor).map_or(end_row, |&(pr, _, _)| pr);
    (end_row + 1, caret_vrow)
}

pub(super) fn input_overflow_shift(crow: usize, caret_vrow: usize, visual_rows: usize, rows: usize) -> usize {
    (crow + visual_rows)
        .saturating_sub(rows)
        .min(crow + caret_vrow)
}

/// Whether `col` on input-relative row `target` sits before `start`, the cell
/// the input bar's first character goes in.
pub(super) fn before_input(start: (usize, usize), col: usize, target: usize) -> bool {
    target < start.0 || (target == start.0 && col < start.1)
}

pub(super) fn wrapped_click_index(
    chars: &[char],
    start: (usize, usize),
    cols: usize,
    col: usize,
    target: usize,
    clamp: bool,
) -> Option<usize> {
    let len = chars.len();
    let (positions, r, c) = input_char_positions(chars, start, cols);
    let end_row = if c >= cols { r + 1 } else { r };
    if target > end_row {
        return clamp.then_some(len);
    }
    for (i, &(pr, pc, pw)) in positions.iter().enumerate() {
        if pr == target && col >= pc && col < pc + pw {
            return Some(i);
        }
    }
    if let Some(fi) = positions.iter().position(|&(pr, _, _)| pr == target) {
        if col < positions[fi].1 {
            return Some(fi);
        }
    }
    if let Some(last) = positions.iter().rposition(|&(pr, _, _)| pr == target) {
        if chars[last] == '\n' {
            return Some(last);
        }
    }
    match positions.iter().position(|&(pr, _, _)| pr > target) {
        Some(ni) => Some(ni),
        None => Some(len),
    }
}

impl TerminalView {
    pub(super) fn render_input_bar(&self, focused: bool, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (crow, ccol) = self.cursor_cell()?;
        let cx_left = px(GRID_PAD_X) + self.render.cell_width * (ccol as f32);
        let shift = self.input_scroll_rows();
        let cy_top = px(GRID_PAD_Y) + self.render.line_height * (crow as f32 - shift as f32);

        let theme = cx.theme();
        let cell_bg = crate::element::opaque_cell_bg(theme.background, self.render.dim, cx);

        if let Some(rs) = &self.prompt.reverse_search {
            let label = format!("(reverse-i-search)`{}': ", rs.query());
            let matched = cx
                .try_global::<HistoryStore>()
                .map(|store| {
                    let snap = self.reverse_search_snapshot(rs.filter(), store);
                    one_line(rs.selected_line(&snap.entries).unwrap_or_default())
                })
                .unwrap_or_default();
            return Some(
                div()
                    .absolute()
                    .left(cx_left)
                    .top(cy_top)
                    .right_4()
                    .h(self.render.line_height)
                    .bg(cell_bg)
                    // The row floats over live grid cells; a bare div inserts no
                    // hitbox, so a click aimed at it started a selection in the
                    // text underneath (#541).
                    .occlude()
                    .flex()
                    .items_center()
                    .font_family(self.render.font.family.clone())
                    .text_size(self.render.font_size)
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_color(theme.foreground)
                            .child(matched),
                    ),
            );
        }

        let chars: Vec<char> = self.prompt.cmd.text().chars().collect();
        let len = chars.len();
        let cursor = self.prompt.cmd.cursor();
        let marked = self.session.marked_text.clone();
        let has_marked = !marked.is_empty();
        let selection = self.prompt.cmd.selection();

        let fg = theme.foreground;
        let caret_col = theme.caret;
        let muted = theme.muted_foreground;
        // Composite the selection tint over `cell_bg` so selected cells stay
        // opaque and never bleed existing grid glyphs underneath through.
        let sel_bg: gpui::Hsla = {
            let mut s = gpui::Rgba::from(theme.selection);
            let b = gpui::Rgba::from(cell_bg);
            let a = 0.55;
            s.r = s.r * a + b.r * (1. - a);
            s.g = s.g * a + b.g * (1. - a);
            s.b = s.b * a + b.b * (1. - a);
            s.a = 1.;
            s.into()
        };
        let cell_w = self.render.cell_width;
        let lh = self.render.line_height;
        let caret_h = px((self.render.font_size.as_f32() * 1.2).min(lh.as_f32()));
        let caret_top = px((lh.as_f32() - caret_h.as_f32()) / 2.0);

        let line: String = chars.iter().collect();
        let mut colors: Vec<gpui::Hsla> = Vec::with_capacity(len);
        for span in highlight::highlight(&line) {
            let c = self.kind_color(span.kind, cx);
            for _ in span.text.chars() {
                colors.push(c);
            }
        }

        // This caret only exists at a prompt, so the prompt's own shape wins.
        let cursor_style = if self.prompt.helix_mode {
            if self.prompt.cmd.helix.mode.is_block_cursor() {
                anterm_ui_kit::config::CursorStyle::Block
            } else {
                anterm_ui_kit::config::CursorStyle::Bar
            }
        } else {
            let cfg = cx.global::<Config>();
            cfg.prompt_cursor_style.shape().unwrap_or(cfg.cursor_style)
        };
        let cursor_paint = input_caret_paint(focused, self.render.cursor_visible, cursor_style);
        let cursor_on = cursor_paint.is_some();
        let block_cursor = cursor_paint == Some(InputCaretPaint::Block);
        let can_show_caret =
            selection.is_none() || (self.prompt.helix_mode && self.prompt.cmd.helix.mode.is_block_cursor());
        // A block caret is drawn as reverse video on the cell it covers, not as
        // a translucent tint over it — the way every other terminal draws one,
        // and the only way the caret keeps the contrast the theme gave it.
        let caret_ink = anterm_ui_kit::presets::caret_ink(caret_col, theme.background, fg);
        let caret_bar = move || {
            let base = div().absolute().left_0();
            match cursor_paint.expect("caret is only rendered when it has a paint style") {
                // An inactive terminal keeps a steady hollow block, independent
                // of the configured focused-caret shape and current blink phase.
                InputCaretPaint::Outline => base
                    .top(px(0.))
                    .w_full()
                    .h(lh)
                    .border_1()
                    .border_color(caret_col),
                InputCaretPaint::Block => base.top(px(0.)).w_full().h(lh).bg(caret_col),
                InputCaretPaint::Bar => base.top(caret_top).w(px(1.5)).h(caret_h).bg(caret_col),
                InputCaretPaint::Underline => {
                    let uh = px(2.);
                    base.top(lh - uh).w_full().h(uh).bg(caret_col)
                }
            }
        };
        let cell = |color: gpui::Hsla,
                    text: String,
                    width: usize,
                    selected: bool,
                    caret: bool,
                    underline: bool| {
            let inverted = caret && block_cursor;
            let w = cell_w * (width as f32);
            let mut d = div()
                .relative()
                .flex_none()
                .w(w)
                .h(lh)
                .flex()
                .items_center()
                .text_color(if inverted { caret_ink } else { color });
            if inverted {
                d = d.bg(caret_col);
            } else if selected {
                d = d.bg(sel_bg);
            } else {
                d = d.bg(cell_bg);
            }
            if underline {
                d = d.border_b_1().border_color(fg);
            }
            d = d.child(text);
            if caret && !inverted {
                d = d.child(caret_bar());
            }
            d.into_any_element()
        };

        let blank = move |w: gpui::Pixels| div().flex_none().w(w).h(lh);

        // The prompt's row, then the input's. Starting below the prompt leaves
        // that row empty — the prompt shows through it — and the input begins
        // at column 0 of the next.
        let cols = self.terminal.term.lock().columns().max(1);
        let (start_row, start_col) = input_start(ccol, cols);
        let mut lines: Vec<Vec<gpui::AnyElement>> = (0..start_row).map(|_| Vec::new()).collect();
        lines.push(vec![blank(cell_w * (start_col as f32)).into_any_element()]);

        let is_multiline = chars.contains(&'\n');

        let marked_cells = input_cells(&marked.chars().collect::<Vec<char>>());
        for c in input_cells(&chars) {
            // The caret sits on the base of a cell, so an IME's in-flight text
            // opens wherever the caret is drawn — anywhere inside the cell, not
            // only on its first character.
            if has_marked && (c.start..c.end).contains(&cursor) {
                if let Some(last) = lines.last_mut() {
                    for mc in &marked_cells {
                        last.push(cell(
                            fg,
                            mc.text.clone(),
                            mc.width,
                            false,
                            false,
                            true,
                        ));
                    }
                }
            }
            let i = c.start;
            if chars[i] == '\n' {
                if let Some(last) = lines.last_mut() {
                    if can_show_caret && !has_marked && cursor_on && cursor == i {
                        last.push(
                            blank(cell_w)
                                .relative()
                                .bg(cell_bg)
                                .child(caret_bar())
                                .into_any_element(),
                        );
                    } else if selection.is_some_and(|(s, e)| i >= s && i < e) {
                        last.push(blank(cell_w).bg(sel_bg).into_any_element());
                    }
                }
                lines.push(Vec::new());
                continue;
            }
            // A cell is one glyph, so it tints as a unit: a selection that
            // covers any character in it — the base or a mark riding on it —
            // covers the whole thing.
            let selected = selection.is_some_and(|(s, e)| s < c.end && c.start < e);
            let caret =
                can_show_caret && !has_marked && cursor_on && (c.start..c.end).contains(&cursor);
            if let Some(last) = lines.last_mut() {
                last.push(cell(colors[i], c.text, c.width, selected, caret, false));
            }
        }

        let ghost: Option<String> = if selection.is_none() && !has_marked && !is_multiline {
            self.ghost_suggestion(cx)
                .map(|full| full.chars().skip(len).collect::<String>())
                .filter(|r| !r.is_empty())
        } else {
            None
        };

        if cursor == len
            && let Some(last) = lines.last_mut()
        {
            if has_marked {
                for mc in &marked_cells {
                    last.push(cell(fg, mc.text.clone(), mc.width, false, false, true));
                }
            } else if ghost.is_none() {
                let mut tail = blank(cell_w).relative().bg(cell_bg);
                if can_show_caret && cursor_on {
                    tail = tail.child(caret_bar());
                }
                last.push(tail.into_any_element());
            }
        }

        if let Some(rem) = ghost
            && let Some(last) = lines.last_mut()
        {
            let flat: Vec<char> = rem.chars().map(one_line_char).collect();
            for (gi, gc) in input_cells(&flat).into_iter().enumerate() {
                let caret = gi == 0 && cursor == len && cursor_on;
                last.push(cell(muted, gc.text, gc.width, false, caret, false));
            }
        }

        let rows = lines.into_iter().map(move |cells| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .w_full()
                .min_h(lh)
                .children(cells)
        });

        Some(
            div()
                .absolute()
                .left(px(GRID_PAD_X))
                .top(cy_top)
                .right_4()
                .min_h(lh)
                .flex()
                .flex_col()
                .font_family(self.render.font.family.clone())
                .text_size(self.render.font_size)
                .line_height(lh)
                .text_color(fg)
                .children(rows),
        )
    }

    pub(super) fn render_completion_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let s = self.prompt.completion.as_ref()?;
        let items: Vec<&completion::Candidate> = s.filtered.iter().map(|&i| &s.all[i]).collect();
        if items.is_empty() {
            return None;
        }
        let (srow, scol) = self.cursor_cell()?;

        const MAX_ROWS: usize = 10;
        let (total_rows, total_cols) = {
            let term = self.terminal.term.lock();
            (term.screen_lines(), term.columns())
        };
        // Anchored to where the input starts, which is a row below the prompt
        // when the prompt left it too little room.
        let (start_row, scol) = input_start(scol, total_cols.max(1));
        let srow = (srow + start_row).saturating_sub(self.input_scroll_rows());
        // How wide the menu ends up, decided before the rows so their
        // descriptions can be elided against it. A menu wider than the pane
        // has its right-hand column clipped by the pane, and the clip takes
        // the ellipsis with it — which is exactly the mid-word cut the
        // ellipsis exists to prevent. The history menu below already caps
        // itself to the grid this way.
        let grid_w = self.render.cell_width * (total_cols as f32);
        let menu_w = px(COMPLETION_MENU_MAX_W).min(grid_w);
        let (place_above, visible, first) = menu_layout(
            total_rows,
            srow,
            items.len(),
            s.index.unwrap_or(0),
            MAX_ROWS,
        );
        let hidden_above = first;
        let hidden_below = items.len() - first - visible;

        let theme = cx.theme();
        let lh = self.render.line_height;
        let cell = self.render.cell_width.as_f32();
        let row = |i: usize| {
            let cand = items[i];
            let selected = s.index == Some(i);
            let icon_color = if selected {
                theme.foreground
            } else {
                theme.muted_foreground
            };
            let icon = completion_row_icon(cand.icon.as_deref(), cand.kind, icon_color);
            let label = if cand.is_dir() && !cand.text.ends_with('/') {
                format!("{}/", cand.text)
            } else {
                cand.text.clone()
            };
            let budget = description_budget(cell, label.chars().count(), menu_w.as_f32());
            div()
                .h(lh)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .whitespace_nowrap()
                .when(selected, |d| {
                    d.bg(theme.list_active).text_color(theme.foreground)
                })
                .child(icon)
                .child(div().flex_shrink_0().child(label))
                .when_some(cand.description.clone(), |d, desc| {
                    d.child(
                        div()
                            .ml_2()
                            .text_color(theme.muted_foreground)
                            .child(elide(&desc, budget)),
                    )
                })
                .into_any_element()
        };
        let rows: Vec<gpui::AnyElement> = (first..first + visible).map(row).collect();

        let footer = |n: usize, label: String| {
            (n > 0).then(|| {
                div()
                    .h(lh)
                    .flex()
                    .items_center()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child(label)
                    .into_any_element()
            })
        };
        let footer_lines = (hidden_above > 0) as usize + (hidden_below > 0) as usize;
        let line_count = visible + footer_lines;
        let menu_h = self.render.line_height * (line_count as f32) + px(10.);

        let gap = px(6.);
        // Anchored under the word being completed, then pulled back inside the
        // pane if that would hang it over the right edge.
        let anchor = px(GRID_PAD_X) + self.render.cell_width * (scol as f32);
        let x = anchor
            .min(px(GRID_PAD_X) + grid_w - menu_w)
            .max(px(GRID_PAD_X));
        let y = if place_above {
            px(GRID_PAD_Y) + self.render.line_height * (srow as f32) - menu_h - gap
        } else {
            px(GRID_PAD_Y) + self.render.line_height * ((srow + 1) as f32) + gap
        };

        Some(
            div()
                .absolute()
                .left(x)
                .top(y)
                // The menu has no click handlers of its own, and a handlerless
                // element inserts no hitbox — so without this the press falls
                // through to the grid: the selection there is cleared, or a
                // modified click opens whatever link lies under the row (#541).
                .occlude()
                .flex()
                .flex_col()
                .py_1()
                .min_w(px(120.).min(menu_w))
                .max_w(menu_w)
                .overflow_hidden()
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .rounded(px(6.))
                .font_family(self.render.font.family.clone())
                .text_size(self.render.font_size)
                // Resting rows sit back so the selected row, which paints
                // itself in the full foreground, is the brightest line in
                // the menu — matching what the row icons already do.
                .text_color(theme.muted_foreground)
                .children(footer(hidden_above, format!("↑ {hidden_above} more")))
                .children(rows)
                .children(footer(hidden_below, format!("↓ {hidden_below} more"))),
        )
    }

    pub(super) fn render_reverse_search_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        use crate::prompt::history::HistoryFilter;

        let rs = self.prompt.reverse_search.as_ref()?;
        let filter = rs.filter();
        let matches = rs.matches();
        let (srow, _) = self.cursor_cell()?;

        const MAX_ROWS: usize = 10;
        let (total_rows, total_cols) = {
            let term = self.terminal.term.lock();
            (term.screen_lines(), term.columns())
        };
        let body_count = matches.len().max(1);
        // Account for the 1-row filter header when placing the menu above/below.
        let (place_above, visible, first) = menu_layout(
            total_rows,
            srow,
            body_count,
            rs.selected().min(body_count - 1),
            MAX_ROWS,
        );
        let hidden_above = if matches.is_empty() { 0 } else { first };
        let hidden_below = if matches.is_empty() {
            0
        } else {
            matches.len().saturating_sub(first + visible)
        };

        let theme = cx.theme();
        let popover = theme.popover;
        let border = theme.border;
        let fg = theme.foreground;
        let muted_fg = theme.muted_foreground;
        let muted_bg = theme.muted;
        let list_active = theme.list_active;
        let blue = theme.blue;
        let red = theme.red;
        let lh = self.render.line_height;
        let now = unix_now();

        let scope_label = |mode: HistoryFilter| -> &'static str {
            match mode {
                HistoryFilter::Session => t(L10nKey::HistoryScopeSession),
                HistoryFilter::Directory => t(L10nKey::HistoryScopeDirectory),
                HistoryFilter::Host => t(L10nKey::HistoryScopeHost),
                HistoryFilter::All => t(L10nKey::HistoryScopeAll),
            }
        };

        let pills: Vec<gpui::AnyElement> = HistoryFilter::ALL_MODES
            .into_iter()
            .enumerate()
            .map(|(idx, mode)| {
                let active = mode == filter;
                div()
                    .id(("history-filter-pill", idx))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .when(active, |d| {
                        d.bg(list_active)
                            .text_color(fg)
                            .font_weight(gpui::FontWeight::MEDIUM)
                    })
                    .when(!active, |d| {
                        d.text_color(muted_fg).hover(move |s| s.bg(muted_bg))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                            this.set_reverse_search_filter(mode, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(scope_label(mode))
                    .into_any_element()
            })
            .collect();

        let header = div()
            .h(lh)
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(div().flex().items_center().gap_1().children(pills))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(muted_fg)
                    .whitespace_nowrap()
                    .child(t(L10nKey::HistorySearchHint)),
            );

        let rows: Vec<gpui::AnyElement> = if matches.is_empty() {
            vec![
                div()
                    .h(lh)
                    .flex()
                    .items_center()
                    .px_2()
                    .text_color(muted_fg)
                    .child(t(L10nKey::HistorySearchEmpty))
                    .into_any_element(),
            ]
        } else {
            let store = cx.try_global::<HistoryStore>();
            let snap = store.map(|s| self.reverse_search_snapshot(filter, s));
            (first..first + visible)
                .filter_map(|i| {
                    let m = &matches[i];
                    let snap = snap?;
                    let line = snap.entries.get(m.index)?.as_str();
                    let selected = rs.selected() == i;
                    let base = if selected { fg } else { muted_fg };

                    let spans: Vec<gpui::AnyElement> = highlight_runs(line, &m.positions)
                        .into_iter()
                        .map(|(run, hit)| {
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .text_color(if hit { blue } else { base })
                                .child(run)
                                .into_any_element()
                        })
                        .collect();

                    let meta = snap.meta.get(line);
                    let failed = meta.and_then(|em| em.exit).filter(|&e| e != 0);
                    let duration = meta
                        .and_then(|em| em.duration_ms)
                        .and_then(crate::prompt::history::format_duration);
                    let ago = meta
                        .and_then(|em| em.ts)
                        .map(|ts| crate::prompt::history::format_ago(now, ts));

                    Some(
                        div()
                            .h(lh)
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .whitespace_nowrap()
                            .when(selected, |d| d.bg(list_active))
                            .child(div().flex_1().flex().overflow_hidden().children(spans))
                            .when_some(failed, |d, code| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_color(red)
                                        .child(format!("✗ {code}")),
                                )
                            })
                            .when_some(duration, |d, dur| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_color(muted_fg)
                                        .child(dur),
                                )
                            })
                            .when_some(ago, |d, ago| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .text_color(muted_fg)
                                        .child(ago),
                                )
                            })
                            .into_any_element(),
                    )
                })
                .collect()
        };

        let footer = |n: usize, label: String| {
            (n > 0).then(|| {
                div()
                    .h(lh)
                    .flex()
                    .items_center()
                    .px_2()
                    .text_color(muted_fg)
                    .child(label)
                    .into_any_element()
            })
        };
        let footer_lines = (hidden_above > 0) as usize + (hidden_below > 0) as usize;
        // +1 for the filter header row.
        let line_count = 1 + rows.len().max(1) + footer_lines;
        let menu_h = lh * (line_count as f32) + px(10.);

        let gap = px(6.);
        let grid_w = self.render.cell_width * (total_cols as f32);
        let menu_w = if grid_w < px(720.) { grid_w } else { px(720.) };
        let y = if place_above {
            px(GRID_PAD_Y) + lh * (srow as f32) - menu_h - gap
        } else {
            px(GRID_PAD_Y) + lh * ((srow + 1) as f32) + gap
        };

        Some(
            div()
                .absolute()
                .left(px(GRID_PAD_X))
                .top(y)
                // Same fall-through as the completion menu (#541): a bare div
                // inserts no hitbox, so a click on a history row landed on the
                // grid beneath it.
                .occlude()
                .flex()
                .flex_col()
                .py_1()
                .w(menu_w)
                .overflow_hidden()
                .bg(popover)
                .border_1()
                .border_color(border)
                .rounded(px(6.))
                .font_family(self.render.font.family.clone())
                .text_size(self.render.font_size)
                .text_color(muted_fg)
                .child(header)
                .children(footer(hidden_above, format!("↑ {hidden_above} more")))
                .children(rows)
                .children(footer(hidden_below, format!("↓ {hidden_below} more"))),
        )
    }

    pub(super) fn render_integration_notice(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let text = self.prompt.integration_notice.clone()?;
        Some(Self::notice_pill(text, cx))
    }

    /// The one-row pill that floats over the pane's bottom-right corner.
    fn notice_pill(text: String, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        div()
            .absolute()
            .bottom(px(GRID_PAD_Y))
            .right(px(GRID_PAD_X))
            .max_w(px(560.))
            .px_3()
            .py_1()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .child(text)
    }

    /// What the remote-completion pill should say right now, if anything:
    /// the listing's failure once it has one, "listing…" while it runs, and
    /// nothing at all the rest of the time (#585).
    pub(super) fn remote_completion_notice_text(&self) -> Option<String> {
        if let Some(notice) = &self.prompt.remote_completion_notice {
            return Some(notice.clone());
        }
        self.prompt.remote_completion_inflight
            .then(|| t(L10nKey::CompletionListingRemote).to_string())
    }

    pub(super) fn render_remote_completion_notice(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let text = self.remote_completion_notice_text()?;
        Some(Self::notice_pill(text, cx))
    }

    pub(super) fn render_helix_prompt_badge(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        use anterm_editor::helix::HelixMode;
        if !self.input_active() || !self.prompt.helix_mode || self.prompt.cmd.helix.mode == HelixMode::Insert {
            return None;
        }
        let theme = cx.theme();
        let mode_bg = match self.prompt.cmd.helix.mode {
            HelixMode::Normal => theme.blue,
            HelixMode::Select => theme.magenta,
            HelixMode::Insert => theme.green,
        };
        let pending = self.prompt.cmd.helix.pending_display();
        Some(
            div()
                .absolute()
                .bottom(px(GRID_PAD_Y))
                .right(px(GRID_PAD_X))
                .flex()
                .items_center()
                .gap(px(6.))
                .when_some(pending, |d, seq| {
                    d.child(
                        div()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(theme.popover)
                            .border_1()
                            .border_color(theme.border)
                            .font_family(self.render.font.family.clone())
                            .text_size(px(11.))
                            .text_color(theme.foreground)
                            .child(seq),
                    )
                })
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(mode_bg.opacity(0.22))
                        .font_family(self.render.font.family.clone())
                        .text_size(px(10.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(mode_bg)
                        .child(self.prompt.cmd.helix.mode.badge()),
                ),
        )
    }

    fn kind_color(&self, kind: TokenKind, cx: &App) -> gpui::Hsla {
        let theme = cx.theme();
        match kind {
            TokenKind::Command => theme.green,
            TokenKind::Flag => theme.cyan,
            TokenKind::Path => theme.blue,
            TokenKind::StringLit => theme.yellow,
            TokenKind::Operator => theme.magenta,
            TokenKind::Comment => theme.muted_foreground,
            TokenKind::Arg | TokenKind::Whitespace => theme.foreground,
        }
    }
}
