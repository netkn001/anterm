use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::vte::ansi::Rgb;
use gpui::{Bounds, ContentMask, Hsla, Pixels, ShapedLine, Window, fill, point, size};

use super::colors::PaintColors;
use super::paint::{PowerlineShape, for_each_special_underline, powerline_path, powerline_solid_edge};
use super::{CellGeom, UnderlineKind};
use crate::view::HoveredLink;

/// Snapshot of all viewport-wide inputs that affect how cells map to styled
/// glyphs. Changing any field triggers a full row rebuild; steady-state frames
/// match this key and only rebuild rows reported by `Term::damage()`.
#[derive(Clone, PartialEq)]
pub(super) struct RowCacheKey {
    pub(super) rows: usize,
    pub(super) cols: usize,
    pub(super) display_offset: i32,
    pub(super) cell_width_bits: u32,
    pub(super) line_height_bits: u32,
    pub(super) font_size_bits: u32,
    pub(super) scale_bits: u32,
    pub(super) font_family: gpui::SharedString,
    pub(super) bold_family: Option<gpui::SharedString>,
    pub(super) italic_family: Option<gpui::SharedString>,
    pub(super) font_features: gpui::FontFeatures,
    pub(super) colors_hash: u64,
    pub(super) dim_bits: u32,
    pub(super) under_bits: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct SearchSig {
    pub(super) active: bool,
    pub(super) match_count: usize,
    pub(super) current_index: Option<usize>,
    pub(super) scanned_history: usize,
    pub(super) epoch: u64,
}

#[derive(Clone)]
pub(super) enum CachedGlyphOp {
    Shaped {
        /// Horizontal offset from row origin (`geom.origin.x`).
        dx: Pixels,
        /// Vertical offset from row top (`y`), nonzero only when an icon is centered.
        dy: Pixels,
        /// Clip horizontal offset from row origin.
        clip_x: Pixels,
        clip_w: Pixels,
        line: ShapedLine,
    },
    Powerline {
        col: usize,
        shape: PowerlineShape,
        fg: Hsla,
    },
    BoxDraw {
        col: usize,
        c: char,
        fg: Hsla,
    },
}

/// Precomputed row-relative paint commands for one screen row.
///
/// All coordinates are stored relative to `(geom.origin.x, row_y)` so smooth
/// scrolling, window drags, and split-divider moves replay cached rows without
/// re-segmenting or re-shaping.
#[derive(Clone, Default)]
pub(super) struct CachedRow {
    pub(super) dirty: bool,
    pub(super) has_selection: bool,
    pub(super) has_match: bool,
    pub(super) has_current_match: bool,
    pub(super) bg_runs: Vec<(usize, usize, Hsla)>,
    pub(super) selection_runs: Vec<(usize, usize)>,
    pub(super) match_runs: Vec<(usize, usize)>,
    pub(super) current_match_runs: Vec<(usize, usize)>,
    pub(super) special_underlines: Vec<(usize, usize, UnderlineKind, Hsla)>,
    pub(super) glyphs: Vec<CachedGlyphOp>,
}

impl CachedRow {
    pub(super) fn clear_ops(&mut self) {
        self.bg_runs.clear();
        self.selection_runs.clear();
        self.match_runs.clear();
        self.current_match_runs.clear();
        self.special_underlines.clear();
        self.glyphs.clear();
        self.has_selection = false;
        self.has_match = false;
        self.has_current_match = false;
    }

    pub(super) fn paint_backgrounds(&self, window: &mut Window, geom: &CellGeom, row: usize) {
        for &(start, span, bg) in &self.bg_runs {
            window.paint_quad(fill(geom.cell_rect(row, start, span), bg));
        }
    }

    pub(super) fn paint_selection(&self, window: &mut Window, geom: &CellGeom, row: usize, color: Hsla) {
        for &(start, span) in &self.selection_runs {
            window.paint_quad(fill(geom.cell_rect(row, start, span), color));
        }
    }

    pub(super) fn paint_matches(
        &self,
        window: &mut Window,
        geom: &CellGeom,
        row: usize,
        match_bg: Hsla,
        current_match_bg: Hsla,
    ) {
        for &(start, span) in &self.match_runs {
            window.paint_quad(fill(geom.cell_rect(row, start, span), match_bg));
        }
        for &(start, span) in &self.current_match_runs {
            window.paint_quad(fill(geom.cell_rect(row, start, span), current_match_bg));
        }
    }

    pub(super) fn paint_glyphs(
        &self,
        window: &mut Window,
        cx: &mut gpui::App,
        geom: &CellGeom,
        row: usize,
        scale: f32,
    ) {
        let y = geom.origin.y + geom.line_height * (row as f32);
        for op in &self.glyphs {
            match op {
                CachedGlyphOp::Shaped {
                    dx,
                    dy,
                    clip_x,
                    clip_w,
                    line,
                } => {
                    let clip = Bounds::new(
                        point(geom.origin.x + *clip_x, y),
                        size(*clip_w, geom.line_height),
                    );
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        _ = line.paint(
                            point(geom.origin.x + *dx, y + *dy),
                            geom.line_height,
                            gpui::TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    });
                }
                CachedGlyphOp::Powerline { col, shape, fg } => {
                    let cell_bounds = geom.cell_rect(row, *col, 1);
                    if let Some(edge) = powerline_solid_edge(cell_bounds, *shape, scale, fg.a) {
                        window.paint_quad(fill(edge, *fg));
                    }
                    let path = powerline_path(cell_bounds, *shape);
                    window.paint_path(path, *fg);
                }
                CachedGlyphOp::BoxDraw { col, c, fg } => {
                    let cell_bounds = geom.cell_rect(row, *col, 1);
                    if let Some(ink) =
                        crate::render::boxdraw::glyph(*c, cell_bounds, scale)
                    {
                        for piece in ink {
                            match piece {
                                crate::render::boxdraw::Ink::Rect(r) => {
                                    window.paint_quad(fill(r, *fg));
                                }
                                crate::render::boxdraw::Ink::Shade(r, alpha) => {
                                    let mut c = *fg;
                                    c.a *= alpha;
                                    window.paint_quad(fill(r, c));
                                }
                                crate::render::boxdraw::Ink::Path(p) => {
                                    window.paint_path(p, *fg);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn paint_special_underlines(
        &self,
        window: &mut Window,
        geom: &CellGeom,
        row: usize,
        scale: f32,
    ) {
        for &(start, span, kind, color) in &self.special_underlines {
            for_each_special_underline(geom.cell_rect(row, start, span), kind, scale, |rect| {
                window.paint_quad(fill(rect, color));
            });
        }
    }
}

/// Per-pane incremental render cache owned by [`crate::view::TerminalView`].
#[derive(Default)]
pub(crate) struct RowCache {
    pub(super) key: Option<RowCacheKey>,
    pub(super) rows: Vec<CachedRow>,
    pub(super) last_selection: Option<SelectionRange>,
    pub(super) last_search: SearchSig,
    pub(super) last_hovered_link: Option<HoveredLink>,
    /// `(row, col)` of the currently inverted block cursor baked into `rows[row]`,
    /// so moving or blinking the cursor invalidates at most 1–2 rows while
    /// keeping `TerminalView::grid_buf` unmutated.
    pub(super) last_inverted_cursor: Option<(usize, usize)>,
    #[cfg(test)]
    pub(crate) stats_rebuilt_rows: usize,
    #[cfg(test)]
    pub(crate) stats_reused_rows: usize,
}

impl RowCache {
    pub(super) fn resize_rows(&mut self, rows: usize) {
        if self.rows.len() != rows {
            self.rows.resize_with(rows, CachedRow::default);
            for row in &mut self.rows {
                row.dirty = true;
            }
        }
    }

    pub(super) fn mark_row_dirty(&mut self, row: usize) {
        if let Some(entry) = self.rows.get_mut(row) {
            entry.dirty = true;
        }
    }
}

/// Computes a fast deterministic fingerprint over the active palette and
/// resolved paint colours so theme/palette edits invalidate [`RowCache`].
pub(super) fn colors_fingerprint(palette: &[Rgb; 256], colors: &PaintColors) -> u64 {
    let mut h = DefaultHasher::new();
    for rgb in palette {
        rgb.r.hash(&mut h);
        rgb.g.hash(&mut h);
        rgb.b.hash(&mut h);
    }
    colors.fg_rgb.r.hash(&mut h);
    colors.fg_rgb.g.hash(&mut h);
    colors.fg_rgb.b.hash(&mut h);
    colors.bg_rgb.r.hash(&mut h);
    colors.bg_rgb.g.hash(&mut h);
    colors.bg_rgb.b.hash(&mut h);
    colors.legible_dim.hash(&mut h);
    colors.caret.h.to_bits().hash(&mut h);
    colors.caret.s.to_bits().hash(&mut h);
    colors.caret.l.to_bits().hash(&mut h);
    colors.caret.a.to_bits().hash(&mut h);
    colors.selection_bg.h.to_bits().hash(&mut h);
    colors.selection_bg.l.to_bits().hash(&mut h);
    colors.match_bg.h.to_bits().hash(&mut h);
    colors.current_match_bg.h.to_bits().hash(&mut h);
    h.finish()
}
