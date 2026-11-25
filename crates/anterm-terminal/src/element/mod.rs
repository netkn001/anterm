use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column as AlacColumn, Line as AlacLine, Point as AlacPoint};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{TermDamage, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, Bounds, ContentMask, Corners, CursorStyle, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, HitboxId, Hsla, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, SharedString, Style, TextRun, Window, fill,
    point, px, relative, size,
};
use gpui_kit::component::ActiveTheme as _;

use super::view::{TerminalView, should_show_context_menu};
use anterm_ui_kit::config::Config;

mod colors;
mod osc8_underline;
mod paint;
mod row_cache;
#[cfg(test)]
mod tests;

pub(super) use colors::{PaintColors, dim_under, opaque_cell_bg};
#[cfg(test)]
pub(super) use colors::blend_toward;
use colors::{dim_cell, snapshot_cell};
#[cfg(test)]
use paint::GlyphStyle;
use paint::{
    build_cached_row, build_faces, invert_cursor_cell, paint_backgrounds, paint_cell_runs,
    paint_cursor, paint_glyphs, paint_marked, paint_special_underlines,
};
pub(crate) use row_cache::RowCache;
use row_cache::{RowCacheKey, SearchSig, colors_fingerprint};

/// How much of the text's own colour a link's underline keeps before the
/// modifier is down. Low enough to read as a hint rather than as markup, high
/// enough to survive a light theme.
const HELD_BACK_LINK_ALPHA: f32 = 0.45;

#[derive(Clone, Copy, PartialEq, Default, Debug)]
enum UnderlineKind {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone)]
pub(super) struct RenderCell {
    c: char,
    marks: Option<Box<[char]>>,
    fg: Hsla,
    bg: Hsla,
    draw_bg: bool,
    bold: bool,
    italic: bool,
    strikeout: bool,
    underline: UnderlineKind,
    underline_color: Option<Hsla>,
    spacer: bool,
    selected: bool,
    match_hit: bool,
    match_current: bool,
    link_hover: bool,
    osc8_dots: bool,
}

impl Default for RenderCell {
    fn default() -> Self {
        Self {
            c: ' ',
            marks: None,
            fg: Hsla::default(),
            bg: Hsla::default(),
            draw_bg: false,
            bold: false,
            italic: false,
            strikeout: false,
            underline: UnderlineKind::None,
            underline_color: None,
            spacer: false,
            selected: false,
            match_hit: false,
            match_current: false,
            link_hover: false,
            osc8_dots: false,
        }
    }
}

pub struct TerminalElement {
    view: gpui::Entity<TerminalView>,
}

impl TerminalElement {
    pub fn new(view: gpui::Entity<TerminalView>) -> Self {
        Self { view }
    }
}

pub struct TermLayout {
    cell_width: Pixels,
    line_height: Pixels,
    cols: usize,
    rows: usize,
    hitbox: Hitbox,
}

/// What colour a hovered link's underline takes.
///
/// Until the modifier is down the link is only being pointed out, so it is
/// drawn as the same underline held back. A cell carrying an underline of its
/// own keeps the colour it was given: dimming that would be editing the
/// application's output rather than annotating it.
fn link_underline_color(cell: &RenderCell, armed: bool) -> Option<Hsla> {
    if armed || cell.underline != UnderlineKind::None || cell.underline_color.is_some() {
        return cell.underline_color;
    }
    Some(cell.fg.opacity(HELD_BACK_LINK_ALPHA))
}

#[derive(Clone, Copy)]
pub(super) struct GridCursor {
    row: usize,
    col: usize,
    // Where the IME candidate window should anchor: the fake caret drawn by
    // cursor-hiding TUIs when one is identifiable, else `col`.
    ime_col: usize,
    hidden: bool,
    style: anterm_ui_kit::config::CursorStyle,
}

fn cursor_style_from_shape(shape: CursorShape) -> anterm_ui_kit::config::CursorStyle {
    match shape {
        CursorShape::Beam => anterm_ui_kit::config::CursorStyle::Bar,
        CursorShape::Underline => anterm_ui_kit::config::CursorStyle::Underline,
        _ => anterm_ui_kit::config::CursorStyle::Block,
    }
}

#[derive(Clone)]
pub(super) struct GridSnapshot {
    cursor: Option<GridCursor>,
    sliver: Option<Vec<RenderCell>>,
    any_selected: bool,
    /// Scrollback state at snapshot time, so the paint pass can map a kitty
    /// image's absolute anchor row back to a screen row: screen_row =
    /// anchor_row - history_size + display_offset.
    display_offset: i32,
    history_size: usize,
}

impl GridSnapshot {
    /// The terminal caret this frame paints, if any. None while the program
    /// has the cursor hidden: alacritty reports a DECTCEM-reset cursor
    /// (`?25l`) as `CursorShape::Hidden`, and a TUI that hides it is usually
    /// drawing a caret of its own that ours must not cover (#844).
    pub(super) fn painted_cursor(
        &self,
    ) -> Option<(usize, usize, anterm_ui_kit::config::CursorStyle)> {
        self.cursor
            .filter(|c| !c.hidden)
            .map(|c| (c.row, c.col, c.style))
    }
}

impl TerminalElement {
    /// The shape `prompt_cursor_style` puts on the grid cursor while the shell
    /// waits at a prompt with its own line editor — `prompt_editor` off, or a
    /// line handed back to ZLE/readline.
    pub(super) fn prompt_cursor_shape(&self, cx: &App) -> Option<anterm_ui_kit::config::CursorStyle> {
        let shape = cx.global::<Config>().prompt_cursor_style.shape()?;
        let terminal = &self.view.read(cx).terminal;
        terminal.at_prompt().then_some(shape)
    }

    #[cfg(test)]
    pub(super) fn build_grid(
        &self,
        colors: &PaintColors,
        buf: &mut Vec<RenderCell>,
        rows: usize,
        cols: usize,
        want_sliver: bool,
        cx: &App,
        dim: f32,
        under: Rgba,
        must_block: bool,
    ) -> Option<GridSnapshot> {
        let mut cursor: Option<GridCursor> = None;
        let mut sliver: Option<Vec<RenderCell>> = None;
        let mut any_selected = false;
        let display_offset;
        let history_size;
        let prompt_shape = self.prompt_cursor_shape(cx);
        {
            let mut palette = self.view.read(cx).terminal.palette;
            if let Some(active) = cx.try_global::<crate::render::palette::ActivePalette>() {
                palette[..16].copy_from_slice(&active.ansi16);
            }
            let term = self.view.read(cx).terminal.term.clone();
            // Rendering does not queue for the grid lock. Holding it is the
            // pane's own reader, part-way through feeding a batch of output
            // into the emulator — and one UI thread draws every pane in every
            // window, so waiting here wires one pane's write speed to the frame
            // rate of the whole window. That is the same illness as #709, which
            // was this thread parked in `write(2)` for a stalled link; this is
            // the read side of it. A frame that cannot have the lock paints the
            // one before it, and nobody can see a frame of lag.
            //
            // `try_lock_unfair` rather than a lease: a painter that queued
            // would make the reader wait for a frame it is not going to get
            // anyway. Skipping the queue is safe precisely because it never
            // waits.
            let term = match term.try_lock_unfair() {
                Some(term) => term,
                // The two frames that have to have it: the first one, with no
                // previous grid to fall back on, and the one after a resize,
                // where the previous grid is the wrong shape. Both are rare and
                // neither is in the steady state.
                None if must_block => term.lock(),
                None => return None,
            };
            // After the lock, not before: an early return must leave the
            // previous frame's cells intact for the caller to paint again.
            buf.clear();
            buf.resize(rows * cols, RenderCell::default());
            let content = term.renderable_content();
            display_offset = content.display_offset as i32;
            history_size = term.grid().history_size();
            let selection = content.selection;

            let cur = content.cursor;
            let cursor_row = cur.point.line.0 + display_offset;
            let cursor_hidden = matches!(cur.shape, CursorShape::Hidden);
            // TUIs that hide the hardware cursor (Kimi CLI, Ink apps) draw
            // their own caret as a reverse-video cell and park the real
            // cursor wherever the frame's last write ended, which strands
            // the IME candidate window there (#275). A lone short inverse
            // run on the cursor's row is that fake caret; collect runs so
            // the IME anchor can snap to it.
            let mut inverse_runs: Vec<(usize, usize)> = Vec::new();

            for cell in content.display_iter {
                let row = cell.point.line.0 + display_offset;
                let col = cell.point.column.0;
                if row < 0 || row as usize >= rows || col >= cols {
                    continue;
                }
                if cursor_hidden
                    && row == cursor_row
                    && cell.cell.flags.contains(Flags::INVERSE)
                    && !cell
                        .cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    match inverse_runs.last_mut() {
                        Some((start, len)) if *start + *len == col => *len += 1,
                        _ => inverse_runs.push((col, 1)),
                    }
                }
                let rc = snapshot_cell(cell.cell, cell.point, &palette, colors, selection.as_ref());
                any_selected |= rc.selected;
                buf[row as usize * cols + col] = if dim < 1. {
                    dim_cell(rc, dim, under)
                } else {
                    rc
                };
            }

            if want_sliver && (display_offset as usize) < term.grid().history_size() {
                let line = AlacLine(-display_offset - 1);
                let mut row_buf = vec![RenderCell::default(); cols];
                for (col, slot) in row_buf
                    .iter_mut()
                    .enumerate()
                    .take(term.columns().min(cols))
                {
                    let point = AlacPoint::new(line, AlacColumn(col));
                    let mut rc = snapshot_cell(
                        &term.grid()[line][AlacColumn(col)],
                        point,
                        &palette,
                        colors,
                        selection.as_ref(),
                    );
                    if dim < 1. {
                        rc = dim_cell(rc, dim, under);
                    }
                    any_selected |= rc.selected;
                    *slot = rc;
                }
                sliver = Some(row_buf);
            }

            let row = cursor_row;
            let col = cur.point.column.0;
            if row >= 0 && (row as usize) < rows && col < cols {
                // Snap the IME anchor to the fake caret when there is
                // exactly one caret-sized inverse run on the row.
                let ime_col = match inverse_runs.as_slice() {
                    [(start, len)] if cursor_hidden && *len <= 2 => *start,
                    _ => col,
                };
                cursor = Some(GridCursor {
                    row: row as usize,
                    col,
                    ime_col,
                    hidden: cursor_hidden,
                    // Checked under the grid lock: `on_alt_screen` would take
                    // it a second time.
                    style: prompt_shape
                        .filter(|_| !term.mode().contains(TermMode::ALT_SCREEN))
                        .unwrap_or_else(|| cursor_style_from_shape(cur.shape)),
                });
            }
        }

        let _ = self.flag_search_matches(buf, rows, cols, display_offset, cx);
        self.flag_hovered_link(buf, rows, cols, display_offset, cx);
        Some(GridSnapshot {
            cursor,
            sliver,
            any_selected,
            display_offset,
            history_size,
        })
    }

    /// Damage-tracked incremental grid sync.
    ///
    /// Consumes `alacritty_terminal::Term::damage()` under a short
    /// `try_lock_unfair()` (or blocking lock on first/resized frame) and only
    /// rebuilds `RenderCell`s and marks `RowCache` dirty for rows that actually
    /// changed. Calls `term.reset_damage()` only once the dirty rows have been
    /// committed to `buf`.
    #[allow(clippy::too_many_arguments)]
    fn sync_grid_incremental(
        &self,
        colors: &PaintColors,
        buf: &mut Vec<RenderCell>,
        cache: &mut RowCache,
        geom: &CellGeom,
        font_size: Pixels,
        scale: f32,
        base_font: &gpui::Font,
        bold_font: Option<&gpui::Font>,
        italic_font: Option<&gpui::Font>,
        want_sliver: bool,
        cx: &App,
        dim: f32,
        under: Rgba,
        must_block: bool,
    ) -> Option<GridSnapshot> {
        let rows = geom.rows;
        let cols = geom.cols;
        let prompt_shape = self.prompt_cursor_shape(cx);
        let (mut palette, term_arc, search_sig, hovered_link) = {
            let view = self.view.read(cx);
            let search_sig = match view.search_state.active.as_ref() {
                Some(s) => SearchSig {
                    active: true,
                    match_count: s.matches.len(),
                    current_index: s.current_index,
                    scanned_history: s.scanned_history,
                    epoch: view.search_state.scan_epoch,
                },
                None => SearchSig::default(),
            };
            (
                view.terminal.palette,
                view.terminal.term.clone(),
                search_sig,
                view.links.hovered_link.clone(),
            )
        };
        if let Some(active) = cx.try_global::<crate::render::palette::ActivePalette>() {
            palette[..16].copy_from_slice(&active.ansi16);
        }

        let colors_hash = colors_fingerprint(&palette, colors);
        let mut cursor: Option<GridCursor> = None;
        let mut sliver: Option<Vec<RenderCell>> = None;
        let mut dirty_rows = vec![false; rows];
        let display_offset;
        let history_size;

        {
            let mut term = match term_arc.try_lock_unfair() {
                Some(term) => term,
                None if must_block => term_arc.lock(),
                None => return None,
            };
            self.view.read(cx).terminal.sync_term_state(&term);

            display_offset = term.grid().display_offset() as i32;
            history_size = term.grid().history_size();
            let selection = term.selection.as_ref().and_then(|s| s.to_range(&term));
            let cur_style = term.cursor_style();
            let cur_point = term.grid().cursor.point;
            let alt_screen = term.mode().contains(TermMode::ALT_SCREEN);

            let key = RowCacheKey {
                rows,
                cols,
                display_offset,
                cell_width_bits: geom.cell_width.as_f32().to_bits(),
                line_height_bits: geom.line_height.as_f32().to_bits(),
                font_size_bits: font_size.as_f32().to_bits(),
                scale_bits: scale.to_bits(),
                font_family: base_font.family.clone(),
                bold_family: bold_font.map(|f| f.family.clone()),
                italic_family: italic_font.map(|f| f.family.clone()),
                font_features: base_font.features.clone(),
                colors_hash,
                dim_bits: dim.to_bits(),
                under_bits: ((under.r.to_bits() as u64 ^ (under.g.to_bits() as u64).rotate_left(7))
                    as u32)
                    ^ under.b.to_bits().rotate_left(13)
                    ^ under.a.to_bits().rotate_left(21),
            };

            let full_rebuild = cache.key.as_ref() != Some(&key)
                || cache.rows.len() != rows
                || buf.len() != rows * cols
                || cache.last_selection != selection
                || cache.last_search != search_sig;

            if full_rebuild {
                dirty_rows.fill(true);
            } else {
                match term.damage() {
                    TermDamage::Full => dirty_rows.fill(true),
                    TermDamage::Partial(iter) => {
                        for line in iter {
                            if line.line < rows {
                                dirty_rows[line.line] = true;
                            }
                        }
                    }
                }
                if cache.last_hovered_link != hovered_link {
                    let mark_link_rows = |link: &Option<crate::view::HoveredLink>,
                                          dirty: &mut [bool]| {
                        if let Some(link) = link {
                            for &(start, _) in &link.runs {
                                let r = start.line.0 + display_offset;
                                if r >= 0 && (r as usize) < rows {
                                    dirty[r as usize] = true;
                                }
                            }
                        }
                    };
                    mark_link_rows(&cache.last_hovered_link, &mut dirty_rows);
                    mark_link_rows(&hovered_link, &mut dirty_rows);
                }
            }
            // Safe to reset damage now: every damaged row will be copied into
            // `buf` and flagged dirty in `cache` before this lock drops.
            term.reset_damage();

            if buf.len() != rows * cols {
                buf.clear();
                buf.resize(rows * cols, RenderCell::default());
            }
            cache.resize_rows(rows);
            cache.key = Some(key);
            cache.last_selection = selection;
            cache.last_search = search_sig;
            cache.last_hovered_link = hovered_link;

            let grid = term.grid();
            let copy_cols = cols.min(grid.columns());
            let screen_lines = grid.screen_lines();

            for (r, &is_dirty) in dirty_rows.iter().enumerate() {
                if !is_dirty {
                    continue;
                }
                let row_start = r * cols;
                for c in copy_cols..cols {
                    buf[row_start + c] = RenderCell::default();
                }
                if r < screen_lines {
                    let line = AlacLine(r as i32 - display_offset);
                    let grid_row = &grid[line];
                    for c in 0..copy_cols {
                        let point = AlacPoint::new(line, AlacColumn(c));
                        let rc = snapshot_cell(
                            &grid_row[AlacColumn(c)],
                            point,
                            &palette,
                            colors,
                            selection.as_ref(),
                        );
                        buf[row_start + c] = if dim < 1. {
                            dim_cell(rc, dim, under)
                        } else {
                            rc
                        };
                    }
                } else {
                    for c in 0..copy_cols {
                        buf[row_start + c] = RenderCell::default();
                    }
                }
                cache.rows[r].dirty = true;
            }

            // Cursor & fake-caret IME anchor detection on the single cursor row.
            let cursor_row = cur_point.line.0 + display_offset;
            let cursor_hidden = !term.mode().contains(TermMode::SHOW_CURSOR)
                || matches!(cur_style.shape, CursorShape::Hidden);
            let mut inverse_runs: Vec<(usize, usize)> = Vec::new();
            if cursor_hidden && cursor_row >= 0 && (cursor_row as usize) < screen_lines.min(rows) {
                let line = AlacLine(cursor_row - display_offset);
                let grid_row = &grid[line];
                for col in 0..copy_cols {
                    let cell = &grid_row[AlacColumn(col)];
                    if cell.flags.contains(Flags::INVERSE)
                        && !cell
                            .flags
                            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        match inverse_runs.last_mut() {
                            Some((start, len)) if *start + *len == col => *len += 1,
                            _ => inverse_runs.push((col, 1)),
                        }
                    }
                }
            }

            if want_sliver && (display_offset as usize) < history_size {
                let line = AlacLine(-display_offset - 1);
                let mut row_buf = vec![RenderCell::default(); cols];
                for (col, slot) in row_buf.iter_mut().enumerate().take(copy_cols) {
                    let point = AlacPoint::new(line, AlacColumn(col));
                    let mut rc = snapshot_cell(
                        &grid[line][AlacColumn(col)],
                        point,
                        &palette,
                        colors,
                        selection.as_ref(),
                    );
                    if dim < 1. {
                        rc = dim_cell(rc, dim, under);
                    }
                    *slot = rc;
                }
                sliver = Some(row_buf);
            }

            let col = cur_point.column.0;
            if cursor_row >= 0 && (cursor_row as usize) < rows && col < cols {
                let ime_col = match inverse_runs.as_slice() {
                    [(start, len)] if cursor_hidden && *len <= 2 => *start,
                    _ => col,
                };
                cursor = Some(GridCursor {
                    row: cursor_row as usize,
                    col,
                    ime_col,
                    hidden: cursor_hidden,
                    style: prompt_shape
                        .filter(|_| !alt_screen)
                        .unwrap_or_else(|| cursor_style_from_shape(cur_style.shape)),
                });
            }
        }

        // Apply search & hovered-link annotations only to dirty rows (clean
        // rows already carry their matching annotations in `buf` & `cache`).
        let any_dirty = dirty_rows.iter().any(|&d| d);
        if any_dirty {
            self.flag_search_matches_rows(buf, &dirty_rows, rows, cols, display_offset, cx);
            self.flag_hovered_link_rows(buf, &dirty_rows, rows, cols, display_offset, cx);
        }

        let sliver_selected = sliver
            .as_ref()
            .is_some_and(|row| row.iter().any(|c| c.selected));

        Some(GridSnapshot {
            cursor,
            sliver,
            any_selected: sliver_selected,
            display_offset,
            history_size,
        })
    }

    fn flag_hovered_link_rows(
        &self,
        buf: &mut [RenderCell],
        dirty_rows: &[bool],
        rows: usize,
        cols: usize,
        display_offset: i32,
        cx: &App,
    ) {
        let Some(link) = self.view.read(cx).links.hovered_link.as_ref() else {
            return;
        };
        for &(start, end) in &link.runs {
            let grid_row = start.line.0 + display_offset;
            if grid_row < 0 || grid_row as usize >= rows || !dirty_rows[grid_row as usize] {
                continue;
            }
            let row = grid_row as usize * cols;
            for col in start.column.0..=end.column.0.min(cols.saturating_sub(1)) {
                let cell = &mut buf[row + col];
                osc8_underline::unmark(cell);
                cell.link_hover = true;
                cell.underline_color = link_underline_color(cell, link.armed);
            }
        }
    }

    fn flag_search_matches_rows(
        &self,
        buf: &mut [RenderCell],
        dirty_rows: &[bool],
        rows: usize,
        cols: usize,
        display_offset: i32,
        cx: &App,
    ) {
        let Some(search) = self.view.read(cx).search_state.active.as_ref() else {
            return;
        };
        let first = search
            .matches
            .partition_point(|m| m.end().line.0 + display_offset < 0);
        for (i, m) in search.matches.iter().enumerate().skip(first) {
            let is_current = search.current_index == Some(i);
            let start = *m.start();
            let end = *m.end();
            if start.line.0 + display_offset >= rows as i32 {
                break;
            }
            let mut line = start.line.0;
            while line <= end.line.0 {
                let row = line + display_offset;
                if row >= 0 && (row as usize) < rows && dirty_rows[row as usize] {
                    let col_start = if line == start.line.0 {
                        start.column.0
                    } else {
                        0
                    };
                    let col_end = if line == end.line.0 {
                        end.column.0
                    } else {
                        cols.saturating_sub(1)
                    };
                    let mut col = col_start;
                    while col <= col_end && col < cols {
                        let rc = &mut buf[row as usize * cols + col];
                        if is_current {
                            rc.match_current = true;
                        } else {
                            rc.match_hit = true;
                        }
                        col += 1;
                    }
                }
                line += 1;
            }
        }
    }

    #[cfg(test)]
    fn flag_hovered_link(
        &self,
        buf: &mut [RenderCell],
        rows: usize,
        cols: usize,
        display_offset: i32,
        cx: &App,
    ) {
        let Some(link) = self.view.read(cx).links.hovered_link.as_ref() else {
            return;
        };
        for &(start, end) in &link.runs {
            let grid_row = start.line.0 + display_offset;
            if grid_row < 0 || grid_row as usize >= rows {
                continue;
            }
            let row = grid_row as usize * cols;
            for col in start.column.0..=end.column.0.min(cols.saturating_sub(1)) {
                let cell = &mut buf[row + col];
                osc8_underline::unmark(cell);
                cell.link_hover = true;
                cell.underline_color = link_underline_color(cell, link.armed);
            }
        }
    }

    #[cfg(test)]
    fn flag_search_matches(
        &self,
        buf: &mut [RenderCell],
        rows: usize,
        cols: usize,
        display_offset: i32,
        cx: &App,
    ) -> (bool, bool) {
        let Some(search) = self.view.read(cx).search_state.active.as_ref() else {
            return (false, false);
        };
        let (mut any_hit, mut any_current) = (false, false);
        let first = search
            .matches
            .partition_point(|m| m.end().line.0 + display_offset < 0);
        for (i, m) in search.matches.iter().enumerate().skip(first) {
            let is_current = search.current_index == Some(i);
            let start = *m.start();
            let end = *m.end();
            if start.line.0 + display_offset >= rows as i32 {
                break;
            }
            if is_current {
                any_current = true;
            } else {
                any_hit = true;
            }
            let mut line = start.line.0;
            while line <= end.line.0 {
                let row = line + display_offset;
                if row >= 0 && (row as usize) < rows {
                    let col_start = if line == start.line.0 {
                        start.column.0
                    } else {
                        0
                    };
                    let col_end = if line == end.line.0 {
                        end.column.0
                    } else {
                        cols.saturating_sub(1)
                    };
                    let mut col = col_start;
                    while col <= col_end && col < cols {
                        let rc = &mut buf[row as usize * cols + col];
                        if is_current {
                            rc.match_current = true;
                        } else {
                            rc.match_hit = true;
                        }
                        col += 1;
                    }
                }
                line += 1;
            }
        }
        (any_hit, any_current)
    }

    fn register_mouse_handlers(
        &self,
        geom: CellGeom,
        bounds: Bounds<Pixels>,
        hitbox: HitboxId,
        window: &mut Window,
    ) {
        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
            if !phase.bubble() || !hitbox.is_hovered(window) {
                return;
            }
            let (col, row, left) = geom.pos_to_cell(ev.position);
            let raw_row = geom.pos_to_row_raw(ev.position);
            let mods = ev.modifiers;
            let button = ev.button;
            let clicks = ev.click_count;
            view.update(cx, |v, cx| {
                if button == MouseButton::Right {
                    // Before anything else: gpui-component builds the popup
                    // from a deferred callback, and by then the pointer is
                    // only a memory. Reading the grid now is what lets the
                    // menu name the file that was actually under the click.
                    match should_show_context_menu(v.mouse_mode(), mods.shift) {
                        true => v.record_menu_link(col, row, cx),
                        false => v.forget_menu_link(),
                    }
                }
                let link_modifier = anterm_ui_kit::platform::is_secondary(mods) || v.link_modifier_down();
                if link_modifier
                    && button == MouseButton::Left
                    && v.open_link_at(col, row, window, cx)
                {
                    return;
                }
                // The mirror image of the context-menu gate in
                // `TerminalView::render`: a click the application gets is never
                // also a click anterm acts on, and vice versa.
                if !should_show_context_menu(v.mouse_mode(), mods.shift) {
                    v.mouse_press(button, col, row, &mods);
                    return;
                }
                if button == MouseButton::Left
                    && v.editor_click(col, raw_row, clicks, mods.shift, cx)
                {
                    return;
                }
                if button == MouseButton::Left {
                    v.on_select_start(col, row, left, clicks, mods.shift, cx);
                }
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, _phase, window, cx| {
            let (col, row, left) = geom.pos_to_cell(ev.position);
            let raw_row = geom.pos_to_row_raw(ev.position);
            let mods = ev.modifiers;
            let Some(button) = ev.pressed_button else {
                let inside = hitbox.is_hovered(window);
                if inside && cx.global::<Config>().focus_follows_mouse {
                    let handle = view.read(cx).focus_handle.clone();
                    if !handle.is_focused(window) {
                        window.focus(&handle, cx);
                    }
                }
                view.update(cx, |v, cx| {
                    v.drop_mouse_grab();
                    if inside {
                        if !mods.shift {
                            v.mouse_motion(col, row, &mods);
                        }
                        let armed = anterm_ui_kit::platform::is_secondary(mods) || v.link_modifier_down();
                        v.hover_link_at(col, row, armed, cx);
                    } else {
                        v.clear_hovered_link(cx);
                    }
                });
                return;
            };
            view.update(cx, |v, cx| {
                if v.mouse_mode() && !mods.shift {
                    v.mouse_drag(button, col, row, &mods);
                    return;
                }
                if button == MouseButton::Left && v.editor_drag(col, raw_row, cx) {
                    return;
                }
                if button == MouseButton::Left {
                    v.on_select_update(col, row, left, cx);
                    let overshoot = drag_overshoot(ev.position.y, bounds, geom.line_height);
                    v.select_autoscroll(overshoot, col, left, cx);
                }
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _window, cx| {
            if !phase.bubble() {
                return;
            }
            let (col, row, _left) = geom.pos_to_cell(ev.position);
            let mods = ev.modifiers;
            let button = ev.button;
            view.update(cx, |v, cx| {
                if v.mouse_mode() && !mods.shift {
                    v.mouse_release(button, col, row, &mods);
                    return;
                }
                v.on_select_end(cx);
            });
        });
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TermLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        style.flex_grow = 1.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (font_size, base_font, line_height_mul, cached_width, reserved) = {
            let view = self.view.read(cx);
            let cached = view
                .render
                .cell_width_cache
                .as_ref()
                .filter(|(fam, sz, _)| *fam == view.render.font.family && *sz == view.render.font_size)
                .map(|(_, _, w)| *w);
            (
                view.render.font_size,
                view.render.font.clone(),
                view.render.line_height_mul,
                cached,
                view.composer_reserved_rows(),
            )
        };

        let cell_width = match cached_width {
            Some(w) => w,
            None => {
                let sample = window.text_system().shape_line(
                    SharedString::new_static("M"),
                    font_size,
                    &[TextRun {
                        len: 1,
                        font: base_font.clone(),
                        color: Hsla::default(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                sample.width.max(px(1.))
            }
        };
        let line_height = px((font_size.as_f32() * line_height_mul).round()).max(px(1.));

        let cols = (bounds.size.width.as_f32() / cell_width.as_f32())
            .floor()
            .max(1.0) as usize;
        let fits = (bounds.size.height.as_f32() / line_height.as_f32())
            .floor()
            .max(1.0) as usize;
        // The composer covering an input shorter than itself takes rows off
        // the bottom (see `composer_reserved_rows`); they stay part of the
        // pane's leftover space as far as the layer laid over it goes.
        let rows = fits.saturating_sub(reserved).max(1);

        let scale = window.scale_factor();
        self.view.update(cx, |view, cx| {
            if cached_width.is_none() {
                view.render.cell_width_cache = Some((base_font.family.clone(), font_size, cell_width));
            }
            let changed = view.set_grid_size(cols, rows, cell_width, line_height, scale, cx);
            // Whatever lays itself over the bottom rows (the composer)
            // measures up from the bottom, past this.
            let slack = bounds.size.height - line_height * fits as f32;
            if changed || view.render.grid_slack != slack {
                view.render.grid_slack = slack;
                cx.notify();
            }
        });

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        TermLayout {
            cell_width,
            line_height,
            cols,
            rows,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let fps_start = crate::backend::fps::enabled().then(std::time::Instant::now);

        let (
            frac,
            input_shift,
            font_size,
            base_font,
            bold_font,
            italic_font,
            focused,
            cursor_visible,
            bell_flash,
            editor_active,
            dim_raw,
            focus_handle,
            marked,
            image_store,
        ) = {
            let view = self.view.read(cx);
            (
                view.pointer.scroll_frac.clamp(0., 1.),
                view.input_scroll_rows(),
                view.render.font_size,
                view.render.font.clone(),
                view.render.font_bold.clone(),
                view.render.font_italic.clone(),
                view.focus_handle.is_focused(window),
                view.render.cursor_visible,
                view.render.bell_flash,
                view.input_active(),
                view.render.dim.clamp(0., 1.),
                view.focus_handle.clone(),
                view.session.marked_text.clone(),
                view.terminal.images(),
            )
        };
        let geom = CellGeom {
            origin: point(
                bounds.origin.x,
                bounds.origin.y + prepaint.line_height * (frac - input_shift as f32),
            ),
            cell_width: prepaint.cell_width,
            line_height: prepaint.line_height,
            cols: prepaint.cols,
            rows: prepaint.rows,
        };
        let colors = PaintColors::resolve(cx.theme(), cx);
        let (colors, dim, under) = if dim_raw < 1. {
            let under = dim_under(cx);
            (colors.dimmed(dim_raw, under), dim_raw, under)
        } else {
            (colors, 1., Rgba::default())
        };
        let scale = window.scale_factor();

        // Borrow this pane's persistent grid buffer and incremental row cache
        // in one entity update.
        let (mut buf, mut row_cache, previous) = self.view.update(cx, |view, _| {
            (
                std::mem::take(&mut view.render.grid_buf),
                std::mem::take(&mut view.render.row_cache),
                view.render.grid_snap.clone(),
            )
        });
        let must_block = previous.is_none() || buf.len() != geom.rows * geom.cols;
        let built = self.sync_grid_incremental(
            &colors,
            &mut buf,
            &mut row_cache,
            &geom,
            font_size,
            scale,
            &base_font,
            bold_font.as_ref(),
            italic_font.as_ref(),
            frac > 0.,
            cx,
            dim,
            under,
            must_block,
        );
        if built.is_none() {
            // `try_lock_unfair` lost to the reader thread; schedule one retry
            // on the next frame so a missed lock can never stick on stale pixels.
            let view = self.view.clone();
            window.on_next_frame(move |_, cx| {
                view.update(cx, |v, cx| v.notify_canvas(cx));
            });
        }
        let next_snap = built.clone();
        let Some(snap) = built.or(previous) else {
            self.view.update(cx, |view, _| {
                view.render.grid_buf = buf;
                view.render.row_cache = row_cache;
            });
            return;
        };
        let cursor = snap.cursor;
        let sliver = snap.sliver.as_ref();

        let cursor_cell = cursor.map(|c| (c.row, c.ime_col));
        let render_cursor = snap.painted_cursor();

        // Track focused block cursor without mutating `buf` in place: moving or
        // blinking the cursor dirties only the 1–2 affected cached rows.
        let active_block_cursor = if !editor_active && focused && cursor_visible {
            match render_cursor {
                Some((row, col, anterm_ui_kit::config::CursorStyle::Block)) => Some((row, col)),
                _ => None,
            }
        } else {
            None
        };
        if row_cache.last_inverted_cursor != active_block_cursor {
            if let Some((old_row, _)) = row_cache.last_inverted_cursor {
                row_cache.mark_row_dirty(old_row);
            }
            if let Some((new_row, _)) = active_block_cursor {
                row_cache.mark_row_dirty(new_row);
            }
            row_cache.last_inverted_cursor = active_block_cursor;
        }

        // Rebuild only dirty rows in `row_cache`.
        let any_row_dirty = row_cache.rows.iter().any(|r| r.dirty);
        if any_row_dirty {
            let faces = build_faces(&base_font, bold_font.as_ref(), italic_font.as_ref());
            let mut primary_ids: [Option<gpui::FontId>; 4] = [None; 4];
            let run_buf = &mut [TextRun {
                len: 0,
                font: base_font.clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }];
            let mut cursor_row_scratch: Vec<RenderCell> = Vec::new();
            for row in 0..geom.rows.min(row_cache.rows.len()) {
                if !row_cache.rows[row].dirty {
                    #[cfg(test)]
                    {
                        row_cache.stats_reused_rows += 1;
                    }
                    continue;
                }
                #[cfg(test)]
                {
                    row_cache.stats_rebuilt_rows += 1;
                }
                let row_base = row * geom.cols;
                let row_slice = match active_block_cursor {
                    Some((cr, cc)) if cr == row => {
                        cursor_row_scratch.clear();
                        cursor_row_scratch.extend_from_slice(&buf[row_base..row_base + geom.cols]);
                        invert_cursor_cell(&mut cursor_row_scratch, geom.cols, 0, cc, &colors);
                        cursor_row_scratch.as_slice()
                    }
                    _ => &buf[row_base..row_base + geom.cols],
                };
                build_cached_row(
                    window,
                    cx,
                    row_slice,
                    geom.cell_width,
                    geom.line_height,
                    font_size,
                    &faces,
                    &mut primary_ids,
                    run_buf,
                    &mut row_cache.rows[row],
                );
            }
        } else {
            #[cfg(test)]
            {
                row_cache.stats_reused_rows += geom.rows.min(row_cache.rows.len());
            }
        }

        // With the input bar up, the IME composes into the bar, which starts a
        // row below the prompt when the prompt left it too little room.
        let ime_cell = cursor.map(|c| {
            self.view
                .read(cx)
                .input_ime_cell(c.row, c.col, geom.cols)
                .unwrap_or((c.row, c.ime_col))
        });
        let cursor_bounds = ime_cell.map(|(row, col)| geom.cell_rect(row, col, 1));
        let ime_anchor = cursor_bounds.filter(|_| focused);
        window.handle_input(
            &focus_handle,
            super::input::TerminalInputHandler::new(self.view.clone(), cursor_bounds),
            cx,
        );

        let retired_images = image_store.take_retired();
        let images = image_store.snapshot();
        let safe_scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let paint_rows = geom.rows.min(row_cache.rows.len());
            for row in 0..paint_rows {
                row_cache.rows[row].paint_backgrounds(window, &geom, row);
            }
            for row in 0..paint_rows {
                if row_cache.rows[row].has_selection {
                    row_cache.rows[row].paint_selection(window, &geom, row, colors.selection_bg);
                }
            }
            for row in 0..paint_rows {
                let cr = &row_cache.rows[row];
                if cr.has_match || cr.has_current_match {
                    cr.paint_matches(window, &geom, row, colors.match_bg, colors.current_match_bg);
                }
            }
            for row in 0..paint_rows {
                row_cache.rows[row].paint_glyphs(window, cx, &geom, row, scale);
            }
            for row in 0..paint_rows {
                if !row_cache.rows[row].special_underlines.is_empty() {
                    row_cache.rows[row].paint_special_underlines(window, &geom, row, scale);
                }
            }
            for img in &images {
                let round_w = (geom.cell_width.as_f32() * safe_scale).round().max(1.);
                let round_h = (geom.line_height.as_f32() * safe_scale).round().max(1.);
                let span_cols = if img.cols > 0 {
                    img.cols as f32
                } else {
                    (img.width_px as f32 / round_w).round().max(1.)
                };
                let span_rows = if img.rows > 0 {
                    img.rows as f32
                } else {
                    (img.height_px as f32 / round_h).round().max(1.)
                };
                let screen_row =
                    img.anchor_row - snap.history_size as i64 + snap.display_offset as i64;
                if screen_row + span_rows as i64 <= 0 || screen_row >= geom.rows as i64 {
                    continue;
                }
                if !image_store.claim_for_paint(img) {
                    continue;
                }
                let top = geom.origin.y + geom.line_height * screen_row as f32;
                let left = geom.origin.x + geom.cell_width * img.anchor_col as f32;
                let bounds = Bounds {
                    origin: point(left, top),
                    size: size(geom.cell_width * span_cols, geom.line_height * span_rows),
                };
                let _ = window.paint_image(
                    bounds,
                    bounds,
                    Corners::default(),
                    img.data.clone(),
                    0,
                    false,
                );
                if dim < 1. {
                    let mut c: Hsla = under.into();
                    c.a = 1. - dim;
                    window.paint_quad(fill(bounds, c));
                }
            }
            for retired in &retired_images {
                let _ = window.drop_image(retired.clone());
            }
            if let Some(row) = sliver {
                let sg = CellGeom {
                    origin: point(geom.origin.x, geom.origin.y - geom.line_height),
                    rows: 1,
                    ..geom
                };
                paint_backgrounds(window, &sg, row);
                if snap.any_selected {
                    paint_cell_runs(window, &sg, row, colors.selection_bg, |c| c.selected);
                }
                paint_glyphs(
                    window,
                    cx,
                    &sg,
                    row,
                    font_size,
                    &base_font,
                    bold_font.as_ref(),
                    italic_font.as_ref(),
                );
                paint_special_underlines(window, &sg, row, scale);
            }
            if !editor_active {
                paint_cursor(
                    window,
                    &geom,
                    render_cursor,
                    focused,
                    cursor_visible,
                    colors.caret,
                );
                paint_marked(
                    window,
                    cx,
                    &geom,
                    cursor_cell,
                    &marked,
                    font_size,
                    &base_font,
                    colors.default_fg,
                    colors.default_bg,
                );
            }

            if bell_flash {
                let mut c = colors.default_fg;
                c.a = 0.12;
                window.paint_quad(fill(bounds, c));
            }
        });

        let ime_moved = self.view.update(cx, |view, _| {
            view.render.grid_buf = buf;
            view.render.row_cache = row_cache;
            if let Some(snap) = next_snap {
                view.render.grid_snap = Some(snap);
            }
            view.render.cell_geom = Some(geom);
            view.set_dim(1.);
            view.ime_anchor_moved(ime_anchor)
        });
        if ime_moved {
            window.invalidate_character_coordinates();
        }

        self.register_mouse_handlers(geom, bounds, prepaint.hitbox.id, window);

        let view = self.view.read(cx);
        if view.links.hovered_link.as_ref().is_some_and(|link| link.armed) {
            window.set_cursor_style(CursorStyle::PointingHand, &prepaint.hitbox);
        } else if !view.mouse_mode() {
            window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        }

        if let Some(start) = fps_start {
            crate::backend::fps::record(start.elapsed());
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CellGeom {
    pub(super) origin: Point<Pixels>,
    pub(super) cell_width: Pixels,
    pub(super) line_height: Pixels,
    pub(super) cols: usize,
    pub(super) rows: usize,
}

impl CellGeom {
    fn cell_rect(&self, row: usize, col: usize, span: usize) -> Bounds<Pixels> {
        let x = self.origin.x + self.cell_width * (col as f32);
        let y = self.origin.y + self.line_height * (row as f32);
        Bounds::new(
            point(x, y),
            size(self.cell_width * (span as f32), self.line_height),
        )
    }

    pub(super) fn pos_to_cell(&self, pos: Point<Pixels>) -> (usize, usize, bool) {
        let lx = (pos.x - self.origin.x).as_f32().max(0.);
        let ly = (pos.y - self.origin.y).as_f32().max(0.);
        let colf = lx / self.cell_width.as_f32();
        let col = (colf.floor() as usize).min(self.cols.saturating_sub(1));
        let row =
            ((ly / self.line_height.as_f32()).floor() as usize).min(self.rows.saturating_sub(1));
        let left = (colf - colf.floor()) <= 0.5;
        (col, row, left)
    }

    fn pos_to_row_raw(&self, pos: Point<Pixels>) -> usize {
        let ly = (pos.y - self.origin.y).as_f32().max(0.);
        (ly / self.line_height.as_f32()).floor() as usize
    }
}

fn drag_overshoot(y: Pixels, bounds: Bounds<Pixels>, line_height: Pixels) -> f32 {
    let lh = line_height.as_f32().max(1.);
    if y < bounds.top() {
        (bounds.top() - y).as_f32() / lh
    } else if y > bounds.bottom() {
        -((y - bounds.bottom()).as_f32() / lh)
    } else {
        0.
    }
}
