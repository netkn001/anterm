use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{
    App, BorderStyle, Bounds, Font, Hsla, Pixels, Point, SharedString, StrikethroughStyle,
    TextAlign, TextRun, Window, fill, outline, point, px, size,
};

use super::colors::{PaintColors, build_font};
use super::row_cache::{CachedGlyphOp, CachedRow};
use super::{CellGeom, RenderCell, UnderlineKind};

pub(super) fn paint_backgrounds(window: &mut Window, geom: &CellGeom, buf: &[RenderCell]) {
    for row in 0..geom.rows {
        let mut col = 0;
        while col < geom.cols {
            let cell = &buf[row * geom.cols + col];
            if !cell.draw_bg {
                col += 1;
                continue;
            }
            let bg = cell.bg;
            let start = col;
            while col < geom.cols {
                let c = &buf[row * geom.cols + col];
                if c.spacer || (c.draw_bg && c.bg == bg) {
                    col += 1;
                } else {
                    break;
                }
            }
            window.paint_quad(fill(geom.cell_rect(row, start, col - start), bg));
        }
    }
}

pub(super) fn paint_cell_runs(
    window: &mut Window,
    geom: &CellGeom,
    buf: &[RenderCell],
    color: Hsla,
    mut covered: impl FnMut(&RenderCell) -> bool,
) {
    for row in 0..geom.rows {
        let mut col = 0;
        while col < geom.cols {
            if !covered(&buf[row * geom.cols + col]) {
                col += 1;
                continue;
            }
            let start = col;
            while col < geom.cols {
                let cell = &buf[row * geom.cols + col];
                if covered(cell) || cell.spacer {
                    col += 1;
                } else {
                    break;
                }
            }
            window.paint_quad(fill(geom.cell_rect(row, start, col - start), color));
        }
    }
}

pub(super) fn for_each_special_underline(
    bounds: Bounds<Pixels>,
    kind: UnderlineKind,
    scale: f32,
    mut draw: impl FnMut(Bounds<Pixels>),
) {
    let scale = scale.max(0.1);
    let snap = |v: f32| (v * scale).round() / scale;
    let device_px = 1. / scale;
    let x0 = snap(bounds.origin.x.as_f32());
    let x1 = snap((bounds.origin.x + bounds.size.width).as_f32());
    let y1 = snap((bounds.origin.y + bounds.size.height).as_f32());
    let line = |y: f32, start: f32, end: f32| {
        Bounds::new(
            point(px(start), px(y)),
            size(px((end - start).max(0.)), px(device_px)),
        )
    };

    match kind {
        UnderlineKind::Double => {
            draw(line(y1 - 4. * device_px, x0, x1));
            draw(line(y1 - 2. * device_px, x0, x1));
        }
        UnderlineKind::Dotted | UnderlineKind::Dashed => {
            // The ink is one device pixel thick, but its rhythm is measured in
            // logical pixels. Otherwise at 2x a dotted underline alternates a
            // single physical pixel on/off and reads as a grey solid line.
            let (on, off) = if kind == UnderlineKind::Dotted {
                (1., 1.)
            } else {
                (3., 2.)
            };
            let mut x = x0;
            while x < x1 {
                draw(line(y1 - 2. * device_px, x, (x + on).min(x1)));
                x += on + off;
            }
        }
        _ => {}
    }
}

pub(super) fn paint_special_underlines(window: &mut Window, geom: &CellGeom, buf: &[RenderCell], scale: f32) {
    for row in 0..geom.rows {
        let row_base = row * geom.cols;
        let mut col = 0;
        while col < geom.cols {
            let cell = &buf[row_base + col];
            if !matches!(
                cell.underline,
                UnderlineKind::Double | UnderlineKind::Dotted | UnderlineKind::Dashed
            ) {
                col += 1;
                continue;
            }
            let kind = cell.underline;
            let color = cell.underline_color.unwrap_or(cell.fg);
            let start = col;
            col += 1;
            while col < geom.cols {
                let next = &buf[row_base + col];
                if next.spacer
                    || (next.underline == kind && next.underline_color.unwrap_or(next.fg) == color)
                {
                    col += 1;
                } else {
                    break;
                }
            }
            for_each_special_underline(
                geom.cell_rect(row, start, col - start),
                kind,
                scale,
                |rect| {
                    window.paint_quad(fill(rect, color));
                },
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct GlyphStyle {
    fg: Hsla,
    bold: bool,
    italic: bool,
    strikeout: bool,
    underline: UnderlineKind,
    underline_color: Option<Hsla>,
    link_hover: bool,
}

impl GlyphStyle {
    pub(super) fn of(cell: &RenderCell) -> Self {
        Self {
            fg: cell.fg,
            bold: cell.bold,
            italic: cell.italic,
            strikeout: cell.strikeout,
            underline: cell.underline,
            underline_color: cell.underline_color,
            link_hover: cell.link_hover,
        }
    }

    fn draws_on_blanks(&self) -> bool {
        self.underline != UnderlineKind::None || self.strikeout || self.link_hover
    }

    pub(super) fn underline_style(&self) -> Option<gpui::UnderlineStyle> {
        (matches!(self.underline, UnderlineKind::Single | UnderlineKind::Curly)
            || (self.link_hover && self.underline == UnderlineKind::None))
            .then(|| {
                let wavy = self.underline == UnderlineKind::Curly;
                gpui::UnderlineStyle {
                    thickness: px(1.),
                    color: Some(self.underline_color.unwrap_or(self.fg)),
                    wavy,
                }
            })
    }

    pub(super) fn strikethrough_style(&self) -> Option<StrikethroughStyle> {
        self.strikeout.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(self.fg),
        })
    }
}

fn is_blank(cell: &RenderCell) -> bool {
    (cell.c == '\0' || cell.c == ' ') && cell.marks.is_none()
}

#[derive(Debug, PartialEq)]
pub(super) enum RowSeg {
    Run {
        start: usize,
        cells: usize,
        text: String,
    },
    /// Batch of consecutive same-style square CJK/Kana/Hangul wide characters
    /// shaped in one `shape_line` call with `force_width = 2 * cell_width`.
    WideRun {
        start: usize,
        cells: usize,
        text: String,
    },
    Wide {
        start: usize,
        cells: usize,
        text: SharedString,
    },
    Solo {
        col: usize,
    },
    Cluster {
        col: usize,
        cells: usize,
        text: String,
        wide_base: bool,
    },
}

/// Whether a wide character is a square CJK ideograph, Kana, or Hangul syllable
/// that safely batches into a single [`RowSeg::WideRun`] shaped with
/// `force_width = 2 * cell_width`.
///
/// Fullwidth punctuation (`（`, `）`, `，`), Emoji, and Private Use Area icons
/// are intentionally excluded so they retain individual `ink_bounds` /
/// `fit_scale` / `icon_fit` handling in [`RowSeg::Wide`].
pub(super) fn is_batchable_cjk_wide(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF       // Hiragana & Katakana
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0x20000..=0x2FFFF // CJK Unified Ideographs Extensions B-F
    )
}

fn push_cell(text: &mut String, cell: &RenderCell) {
    text.push(cell.c);
    text.extend(cell.marks.iter().flat_map(|marks| marks.iter()));
}

fn is_sara_am(c: char) -> bool {
    matches!(c, '\u{0E33}' | '\u{0EB3}')
}

fn sara_am_at(row: &[RenderCell], col: usize) -> Option<&RenderCell> {
    row.get(col)
        .filter(|cell| !cell.spacer && is_sara_am(cell.c))
}

fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

fn regional_indicator_at(row: &[RenderCell], col: usize) -> Option<&RenderCell> {
    row.get(col)
        .filter(|cell| !cell.spacer && is_regional_indicator(cell.c))
}

pub(super) fn segment_row(row: &[RenderCell]) -> Vec<RowSeg> {
    let mut segs = Vec::new();
    let mut col = 0;
    while col < row.len() {
        let cell = &row[col];
        if cell.spacer {
            col += 1;
            continue;
        }
        if is_blank(cell) {
            let style = GlyphStyle::of(cell);
            if style.underline == UnderlineKind::None && !style.strikeout {
                col += 1;
                continue;
            }
            let start = col;
            let mut text = String::new();
            while col < row.len()
                && !row[col].spacer
                && is_blank(&row[col])
                && GlyphStyle::of(&row[col]) == style
            {
                text.push(' ');
                col += 1;
            }
            segs.push(RowSeg::Run {
                start,
                cells: col - start,
                text,
            });
            continue;
        }
        // Ahead of the marks branch: a stray mark on either half must not
        // split the pair, or the other half paints as a lettered box.
        if is_regional_indicator(cell.c)
            && let Some(next) = regional_indicator_at(row, col + 1)
        {
            let mut text = String::with_capacity(8);
            push_cell(&mut text, cell);
            push_cell(&mut text, next);
            segs.push(RowSeg::Cluster {
                col,
                cells: 2,
                text,
                wide_base: false,
            });
            col += 2;
            continue;
        }
        if let Some(marks) = &cell.marks {
            let wide_base = col + 1 < row.len() && row[col + 1].spacer;
            let mut cells = if wide_base { 2 } else { 1 };
            let mut text = String::with_capacity(1 + marks.len());
            push_cell(&mut text, cell);
            if !wide_base
                && !is_sara_am(cell.c)
                && let Some(am) = sara_am_at(row, col + 1)
            {
                push_cell(&mut text, am);
                cells = 2;
            }
            segs.push(RowSeg::Cluster {
                col,
                cells,
                text,
                wide_base,
            });
            col += cells;
            continue;
        }
        if !cell.c.is_ascii_graphic() {
            if col + 1 < row.len() && row[col + 1].spacer {
                if is_batchable_cjk_wide(cell.c) {
                    let style = GlyphStyle::of(cell);
                    let start = col;
                    let mut text = String::new();
                    let mut cells = 0usize;
                    while col + 1 < row.len() {
                        let cur = &row[col];
                        if cur.spacer
                            || cur.marks.is_some()
                            || !row[col + 1].spacer
                            || !is_batchable_cjk_wide(cur.c)
                            || GlyphStyle::of(cur) != style
                        {
                            break;
                        }
                        text.push(cur.c);
                        cells += 2;
                        col += 2;
                    }
                    if cells == 2 {
                        segs.push(RowSeg::Wide {
                            start,
                            cells: 2,
                            text: char_string(cell.c),
                        });
                    } else {
                        segs.push(RowSeg::WideRun { start, cells, text });
                    }
                } else {
                    segs.push(RowSeg::Wide {
                        start: col,
                        cells: 2,
                        text: char_string(cell.c),
                    });
                    col += 2;
                }
            } else if !is_sara_am(cell.c)
                && let Some(am) = sara_am_at(row, col + 1)
            {
                let mut text = String::with_capacity(2);
                push_cell(&mut text, cell);
                push_cell(&mut text, am);
                segs.push(RowSeg::Cluster {
                    col,
                    cells: 2,
                    text,
                    wide_base: false,
                });
                col += 2;
            } else {
                segs.push(RowSeg::Solo { col });
                col += 1;
            }
            continue;
        }

        let style = GlyphStyle::of(cell);
        let start = col;
        let mut text = String::new();
        text.push(cell.c);
        let mut cells = 1;
        col += 1;
        let mut gap = 0;
        while col < row.len() {
            let c = &row[col];
            if is_blank(c) && !c.spacer {
                if style.draws_on_blanks() {
                    break;
                }
                gap += 1;
                col += 1;
                continue;
            }
            if c.spacer
                || c.marks.is_some()
                || !c.c.is_ascii_graphic()
                || GlyphStyle::of(c) != style
            {
                break;
            }
            for _ in 0..gap {
                text.push(' ');
            }
            cells += gap;
            gap = 0;
            text.push(c.c);
            cells += 1;
            col += 1;
        }
        segs.push(RowSeg::Run { start, cells, text });
    }
    segs
}

thread_local! {
                            static CHAR_STRINGS: RefCell<HashMap<char, SharedString>> = RefCell::new(HashMap::new());

                        /// Measured ink bounds and the font size they were measured at.
                        static INK_BOUNDS: RefCell<(Pixels, HashMap<(gpui::FontId, char), Option<Bounds<Pixels>>>)> =
                            RefCell::new((px(0.), HashMap::new()));
}

pub(super) fn char_string(c: char) -> SharedString {
    CHAR_STRINGS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 32_768 {
            m.clear();
        }
        m.entry(c)
            .or_insert_with(|| SharedString::from(c.to_string()))
            .clone()
    })
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum PowerlineShape {
    TriangleRight,
    TriangleLeft,
    HalfCircleRight,
    HalfCircleLeft,
    SlantLowerLeft,
    SlantLowerRight,
    SlantUpperLeft,
    SlantUpperRight,
}

impl PowerlineShape {
    pub(super) fn of(c: char) -> Option<Self> {
        Some(match c {
            '\u{e0b0}' => Self::TriangleRight,
            '\u{e0b2}' => Self::TriangleLeft,
            '\u{e0b4}' => Self::HalfCircleRight,
            '\u{e0b6}' => Self::HalfCircleLeft,
            '\u{e0b8}' => Self::SlantLowerLeft,
            '\u{e0ba}' => Self::SlantLowerRight,
            '\u{e0bc}' => Self::SlantUpperLeft,
            '\u{e0be}' => Self::SlantUpperRight,
            _ => return None,
        })
    }

    fn closing_edge_x(self, bounds: Bounds<Pixels>) -> Pixels {
        // Keep this match exhaustive so adding a shape cannot silently assign
        // its solid closing edge to the wrong side of the cell.
        match self {
            Self::TriangleRight
            | Self::HalfCircleRight
            | Self::SlantLowerLeft
            | Self::SlantUpperLeft => bounds.left(),
            Self::TriangleLeft
            | Self::HalfCircleLeft
            | Self::SlantLowerRight
            | Self::SlantUpperRight => bounds.right(),
        }
    }
}

pub(super) fn powerline_path(bounds: Bounds<Pixels>, shape: PowerlineShape) -> gpui::Path<Pixels> {
    let (x0, y0) = (bounds.origin.x, bounds.origin.y);
    let (x1, y1) = (x0 + bounds.size.width, y0 + bounds.size.height);
    let ymid = y0 + bounds.size.height / 2.;

    let tri = |a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>| {
        let mut p = gpui::Path::new(a);
        p.line_to(b);
        p.line_to(c);
        p
    };
    let half_circle = |anchor_x: Pixels, dir: f32| {
        const SEGS: usize = 12;

        let (rx, ry) = (bounds.size.width.as_f32(), bounds.size.height.as_f32() / 2.);

        let at = |scale: f32, theta: f32| {
            let x = anchor_x.as_f32() + dir * rx * scale * theta.cos();
            let y = ymid.as_f32() + ry * scale * theta.sin();
            point(
                px(x.clamp(x0.as_f32(), x1.as_f32())),
                px(y.clamp(y0.as_f32(), y1.as_f32())),
            )
        };

        let step = std::f32::consts::PI / SEGS as f32;
        let mut p = gpui::Path::new(at(1., -std::f32::consts::FRAC_PI_2));
        for i in 0..SEGS {
            let t0 = -std::f32::consts::FRAC_PI_2 + step * i as f32;
            let t1 = t0 + step;
            let ctrl = at(1. / (step / 2.).cos(), (t0 + t1) / 2.);
            p.curve_to(at(1., t1), ctrl);
        }
        p
    };
    match shape {
        PowerlineShape::TriangleRight => tri(point(x0, y0), point(x1, ymid), point(x0, y1)),
        PowerlineShape::TriangleLeft => tri(point(x1, y0), point(x0, ymid), point(x1, y1)),
        PowerlineShape::SlantLowerLeft => tri(point(x0, y0), point(x1, y1), point(x0, y1)),
        PowerlineShape::SlantLowerRight => tri(point(x1, y0), point(x1, y1), point(x0, y1)),
        PowerlineShape::SlantUpperLeft => tri(point(x0, y0), point(x1, y0), point(x0, y1)),
        PowerlineShape::SlantUpperRight => tri(point(x0, y0), point(x1, y0), point(x1, y1)),
        PowerlineShape::HalfCircleRight => half_circle(x0, 1.),
        PowerlineShape::HalfCircleLeft => half_circle(x1, -1.),
    }
}

pub(super) fn powerline_solid_edge(
    bounds: Bounds<Pixels>,
    shape: PowerlineShape,
    scale_factor: f32,
    fg_alpha: f32,
) -> Option<Bounds<Pixels>> {
    // A filled path anti-aliases a closing edge that falls between device
    // pixels. Cover exactly that partially occupied pixel with opaque
    // foreground color; an already aligned edge needs no extra primitive.
    //
    // Only opaque separators qualify. A translucent one (DIM) would composite
    // the cover quad and the path on top of each other, pushing that single
    // column past the glyph's own alpha and tinting the neighboring cell.
    if !fg_alpha.is_finite() || fg_alpha < 1. {
        return None;
    }
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0. {
        scale_factor
    } else {
        1.
    };
    let physical_edge = shape.closing_edge_x(bounds).as_f32() * scale_factor;
    let physical_left = physical_edge.floor();
    let physical_right = physical_edge.ceil();
    if physical_left == physical_right {
        return None;
    }

    Some(Bounds::from_corners(
        point(px(physical_left / scale_factor), bounds.top()),
        point(px(physical_right / scale_factor), bounds.bottom()),
    ))
}

pub(super) fn native_cell_residue(style: &GlyphStyle) -> Option<char> {
    // Special underlines are painted directly from the cell buffer, so a
    // shaped blank is only needed for decorations GPUI owns.
    (style.strikeout
        || matches!(
            style.underline,
            UnderlineKind::Single | UnderlineKind::Curly
        )
        || style.link_hover)
        .then_some(' ')
}

/// How much room a segment's glyph gets before it is made to shrink.
///
/// No monospace face is wide enough for an emoji at the sizes people read at.
/// Apple Color Emoji is a bitmap face whose strikes are quantised, so its ink
/// is widest relative to the em exactly where it hurts: 1.25em from 12 to 16px,
/// tapering to 1.0em by 24px. Two cells only cover 1.25em from 0.63em advance
/// up, and the common faces sit at 0.60em (Menlo, SF Mono) or 0.50em (Ubuntu
/// Mono). Every terminal therefore has to choose between shrinking the glyph
/// and letting it spill. This follows WezTerm, whose answer shows the glyph
/// whole most often: a quarter cell of slack always, and a whole extra cell
/// when the neighbouring cell is blank and has nothing to lose.
pub(super) fn seg_budget(solo: bool, measured: bool, cells: usize, room: bool, cell_width: Pixels) -> Pixels {
    if solo {
        // Single-cell glyphs are allowed to lean into the next cell — that is
        // what keeps the pile of symbols that look fine today from shrinking —
        // but only while that cell is empty. A Nerd Font icon pulled from a
        // fallback face inks well past a narrow primary's cell (1.6 cells is
        // typical), and where the next cell has a glyph of its own the lean is
        // not a lean, it is an overlap: the neighbour is painted afterwards
        // and lands on top of the overshoot. Hand those their own cell and let
        // `fit_scale` bring them down into it, the way kitty and ghostty do.
        //
        // Only where the ink was measured, though. This number is the clip as
        // well as the threshold to shrink at, so narrowing it for a segment
        // nothing could measure would cut the glyph instead of scaling it —
        // see `ink_covers_segment`.
        if room || !measured {
            cell_width * 2.
        } else {
            cell_width
        }
    } else if room {
        cell_width * (cells as f32 + 1.)
    } else {
        cell_width * (cells as f32 + 0.25)
    }
}

/// How much to shrink a segment so its glyph stops inside its budget.
///
/// Never more than 1. A fallback icon is not held to this at all:
/// [`icon_fit`] sizes it, up or down.
pub(super) fn fit_scale(ink: Pixels, budget: Pixels) -> f32 {
    if ink <= budget || ink <= px(0.) {
        1.
    } else {
        budget.as_f32() / ink.as_f32()
    }
}

/// Whether the platform's `typographic_bounds` is the glyph's ink.
///
/// CoreText and DirectWrite answer with the ink box. cosmic-text answers with
/// the advance box pinned to the baseline, which says nothing about where an
/// icon's ink sits; sizing and centring on that would misplace it, so the
/// Linux build keeps the shrink-only rule for icons too.
const INK_BOUNDS_ARE_INK: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The most an icon is ever enlarged by.
///
/// The cap keeps a glyph that is small by design — a dot, a bullet — from
/// being blown up to a cell-sized blob.
pub(super) const MAX_ICON_GROWTH: f32 = 2.;

/// Whether a character is an icon: a code point in one of Unicode's Private
/// Use Areas, where Nerd Fonts, Font Awesome, Codicons and Material Design
/// icons all live.
///
/// Nothing in these ranges is text, so sizing one to its cells cannot change
/// how prose reads — the reason the rule is keyed on the code point and not
/// on the face: a CJK or emoji fallback keeps its own metrics.
///
/// The Powerline separators (`U+E0B0`–`U+E0D7`) are not icons. They are
/// shapes meant to abut the cells around them, the solid ones are painted
/// natively, and the thin ones already reach the row's full height, so
/// fitting them to an em and centring them would only pull them away from
/// the segments they join.
pub(super) fn is_icon(c: char) -> bool {
    matches!(
        c as u32,
        0xE000..=0xE0AF | 0xE0D8..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD
    )
}

/// Whether the cell after an icon can lend it room, the way kitty lets a
/// Private Use Area glyph followed by a space take two cells.
///
/// Backgrounds and selection washes are painted before glyphs, so a blank
/// with the same background and highlight state can share the icon's ink.
/// That is the space after every icon in a coloured prompt segment; held back
/// to one cell there, a Nerd Font icon is clipped or shrunk (#866). Crossing a
/// highlight boundary would instead paint the icon in two states.
pub(super) fn lends_to_icon(row: &[RenderCell], start: usize, cells: usize) -> bool {
    let Some(icon) = row.get(start) else {
        return false;
    };
    let Some(next) = row.get(start + cells) else {
        return true;
    };
    is_blank(next)
        && next.selected == icon.selected
        && next.draw_bg == icon.draw_bg
        && (!icon.draw_bg || next.bg == icon.bg)
        && next.match_hit == icon.match_hit
        && next.match_current == icon.match_current
        && !GlyphStyle::of(next).draws_on_blanks()
}

/// How to size an icon a fallback face supplied, and how many cells it spans.
///
/// A fallback face is drawn at the primary's font size on its own metrics, so
/// what an icon looks like beside the text depends on how the two faces'
/// cells compare. Symbols Nerd Font Mono inks a full em per icon, which is
/// 1.6 cells of a 0.6em primary; a face with a narrower cell than the
/// primary's inks less than one. Neither is the icon at the text's size.
///
/// So fit the ink, aspect ratio kept, to the cells it is given and one em of
/// height — the size the icon was drawn at in its own face, and never taller
/// than the text's line. That may shrink or grow it, growth capped at
/// [`MAX_ICON_GROWTH`] and ignored under 2%, which is not worth a reshape.
/// When `lend` says the next cell is free the icon may take it too, but only
/// if that lets it come out bigger: a narrow glyph that already fits one cell
/// keeps to it and stays beside what follows.
pub(super) fn icon_fit(
    ink: Bounds<Pixels>,
    cell_width: Pixels,
    cells: usize,
    lend: bool,
    height: Pixels,
) -> (f32, usize) {
    let (w, h) = (ink.size.width.as_f32(), ink.size.height.as_f32());
    if !(w > 0. && h > 0.) {
        return (1., cells);
    }
    let fit = |cells: usize| {
        let scale = (cell_width.as_f32() * cells as f32 / w)
            .min(height.as_f32() / h)
            .min(MAX_ICON_GROWTH);
        if (1. ..=1.02).contains(&scale) {
            1.
        } else {
            scale
        }
    };
    let own = fit(cells);
    if lend && fit(cells + 1) > own {
        (fit(cells + 1), cells + 1)
    } else {
        (own, cells)
    }
}

/// Where to paint a fitted icon, relative to its cell's top-left corner, so
/// its ink sits centred in `room` — vertically always, and horizontally only
/// when `centre_x`. An icon spilling into a borrowed cell stays where its
/// face put it instead, as it would unfitted, so the gap before the text
/// after it is the one its face drew.
///
/// `ink` is the glyph's bounds at its native size (y up from the baseline);
/// `ascent` and `descent` are the line's as shaped at the fitted size, which
/// is what gpui centres the baseline in the row with.
pub(super) fn icon_offset(
    ink: Bounds<Pixels>,
    scale: f32,
    room: gpui::Size<Pixels>,
    centre_x: bool,
    ascent: Pixels,
    descent: Pixels,
) -> Point<Pixels> {
    let baseline = (room.height - ascent - descent) / 2. + ascent;
    let ink_top = baseline - (ink.origin.y + ink.size.height) * scale;
    let x = if centre_x {
        (room.width - ink.size.width * scale) / 2. - ink.origin.x * scale
    } else {
        // Nudged only as far as it takes to keep the ink inside the room.
        px(0.)
            .min(room.width - (ink.origin.x + ink.size.width) * scale)
            .max(-ink.origin.x * scale)
    };
    point(x, (room.height - ink.size.height * scale) / 2. - ink_top)
}

/// The ink bounds of a segment's first glyph, relative to its pen position,
/// y up from the baseline.
///
/// Advance is the wrong yardstick: Apple Color Emoji advances 1.31em but only
/// inks 1.25em, so going by advance shrinks glyphs that would have fit. Ask
/// for the glyph's own bounds instead, cached per (face, char) because the
/// lookup is a font query a screenful of CJK would otherwise repeat on every
/// cell of every frame.
fn ink_bounds(
    cx: &App,
    shaped: &gpui::ShapedLine,
    text: &str,
    font_size: Pixels,
) -> Option<Bounds<Pixels>> {
    let font_id = shaped.runs.first()?.font_id;
    let c = text.chars().next()?;
    INK_BOUNDS.with(|cache| {
        let hit = {
            let mut cache = cache.borrow_mut();
            if cache.0 != font_size {
                cache.0 = font_size;
                cache.1.clear();
            }
            if cache.1.len() > 32_768 {
                cache.1.clear();
            }
            cache.1.get(&(font_id, c)).copied()
        };
        if let Some(extent) = hit {
            return extent;
        }
        let bounds = cx
            .text_system()
            .typographic_bounds(font_id, font_size, c)
            .ok();
        cache.borrow_mut().1.insert((font_id, c), bounds);
        bounds
    })
}

/// Where a shaped run stops standing over the cells it came from.
///
/// `force_width` puts the *n*th glyph at *n × cell_width*, which is the whole
/// grid only while the shaper hands back one glyph per character. A ligature
/// substitution collapses several characters into one glyph, and from there on
/// every glyph is pulled left by the cells the substitution swallowed: the
/// text after it overlaps the ligature's tail, and the caret — drawn at the
/// grid column, not at the ink — stands past the end of the run with a gap.
///
/// Takes the glyphs' byte indices, which in a run are their columns, and
/// answers with the first column that has drifted. Only a glyph that arrives
/// later than its ordinal counts: a face that *adds* glyphs cannot be helped
/// by cutting the run, and treating it as drift would cut at column zero
/// forever.
pub(super) fn run_drift(indices: impl IntoIterator<Item = usize>) -> Option<usize> {
    indices
        .into_iter()
        .enumerate()
        .find_map(|(ordinal, index)| (index > ordinal).then_some(index))
}

/// One thing [`paint_run`] asks of the caller, in bytes into the run's text.
///
/// A run is ASCII, one byte per cell, so `at` is also a column offset and
/// `len` is also a width in cells.
#[derive(Debug, PartialEq)]
pub(super) enum RunStep {
    /// Shape `text[at..at + len]` and answer [`run_drift`] for it.
    Drift { at: usize, len: usize },
    /// Shape and paint `text[at..at + len]` starting at column `col`.
    Paint { col: usize, at: usize, len: usize },
}

/// Walk a run in the pieces that stand over their own cells.
///
/// A run that does not drift is one `Drift` and one `Paint` of the whole
/// thing, which is every run in a monospace face. Where it drifts, the part
/// in front of the drift is painted as a piece of its own — shaped on its
/// own, which is the point: clipping the tail away would not help, because
/// the tail drifted *left*, into the columns this piece is keeping. The
/// remainder then starts over in the column the grid puts it in.
///
/// The loop is separated from the shaping so it can be tested at all: gpui's
/// test text system hands back one glyph per character, so nothing shaped
/// through it ever drifts and the interesting half would never run.
pub(super) fn paint_run(start: usize, len: usize, mut step: impl FnMut(RunStep) -> Option<usize>) {
    let mut col = start;
    let mut at = 0;
    loop {
        match step(RunStep::Drift { at, len: len - at }) {
            None => {
                step(RunStep::Paint {
                    col,
                    at,
                    len: len - at,
                });
                return;
            }
            Some(drift) => {
                step(RunStep::Paint {
                    col,
                    at,
                    len: drift,
                });
                col += drift;
                at += drift;
            }
        }
    }
}

/// Shape one piece of a segment's text.
///
/// The whole of it keeps the string it arrived in; only a piece cut out of a
/// drifted run has to be copied, and that happens where a ligature forced the
/// cut. Repeat calls for the same piece within a frame are answered from
/// gpui's line-layout cache rather than shaped again.
fn shape_piece(
    window: &mut Window,
    run_buf: &mut [TextRun; 1],
    text: &SharedString,
    at: usize,
    len: usize,
    font_size: Pixels,
    force_width: Option<Pixels>,
) -> gpui::ShapedLine {
    let piece = if at == 0 && len == text.len() {
        text.clone()
    } else {
        SharedString::from(text[at..at + len].to_string())
    };
    run_buf[0].len = piece.len();
    window
        .text_system()
        .shape_line(piece, font_size, run_buf, force_width)
}

/// Whether [`ink_bounds`]'s answer speaks for the whole segment.
///
/// It measures the segment's first character in the run's first face. That is
/// all of a one-character segment and only part of anything longer: a
/// cluster's combining marks can ink well to the right of the base they hang
/// off — Devanagari `\u{915}` + `\u{93E}` — and are not in the number. `None`
/// is a face that answered no bounds at all, which is no measurement either.
///
/// Neither can be scaled to fit, so neither may be held to one cell: the
/// budget doubles as the clip, and a segment that cannot shrink into a
/// narrowed one is simply cut off at it.
pub(super) fn ink_covers_segment(ink: Option<Pixels>, text: &str) -> bool {
    ink.is_some() && text.chars().nth(1).is_none()
}

/// Whether the cell after a segment is free for its glyph to lean into.
///
/// A blank still owns its cell if it paints anything there: a background, a
/// selection, or a rule of its own. An underlined or hovered blank becomes a
/// `Run` of its own and is painted after the segment beside it, so lending it
/// out would drag a stroke straight across the borrowed glyph.
pub(super) fn has_room_after(row: &[RenderCell], start: usize, cells: usize) -> bool {
    match row.get(start + cells) {
        None => true,
        Some(next) => {
            is_blank(next)
                && !next.draw_bg
                && !next.selected
                && !GlyphStyle::of(next).draws_on_blanks()
        }
    }
}

pub(super) fn segment_has_room_after(row: &[RenderCell], start: usize, cells: usize, icon: bool) -> bool {
    if icon {
        lends_to_icon(row, start, cells)
    } else {
        has_room_after(row, start, cells)
    }
}

/// Builds the row-relative cached paint ops (`bg_runs`, selection/match washes,
/// special underlines, and shaped/native glyphs) for a single screen row.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_cached_row(
    window: &mut Window,
    cx: &mut App,
    row_cells: &[RenderCell],
    cell_width: Pixels,
    line_height: Pixels,
    font_size: Pixels,
    faces: &[Font; 4],
    primary_ids: &mut [Option<gpui::FontId>; 4],
    run_buf: &mut [TextRun; 1],
    out: &mut CachedRow,
) {
    out.clear_ops();
    let cols = row_cells.len();

    // 1. Background runs
    let mut col = 0;
    while col < cols {
        let cell = &row_cells[col];
        if !cell.draw_bg {
            col += 1;
            continue;
        }
        let bg = cell.bg;
        let start = col;
        while col < cols {
            let c = &row_cells[col];
            if c.spacer || (c.draw_bg && c.bg == bg) {
                col += 1;
            } else {
                break;
            }
        }
        out.bg_runs.push((start, col - start, bg));
    }

    // 2. Selection / search match runs
    let collect_runs = |target: &mut Vec<(usize, usize)>, pred: fn(&RenderCell) -> bool| -> bool {
        let mut any = false;
        let mut c = 0;
        while c < cols {
            if !pred(&row_cells[c]) {
                c += 1;
                continue;
            }
            any = true;
            let start = c;
            while c < cols {
                let cell = &row_cells[c];
                if pred(cell) || cell.spacer {
                    c += 1;
                } else {
                    break;
                }
            }
            target.push((start, c - start));
        }
        any
    };
    out.has_selection = collect_runs(&mut out.selection_runs, |c| c.selected);
    out.has_match = collect_runs(&mut out.match_runs, |c| c.match_hit && !c.match_current);
    out.has_current_match = collect_runs(&mut out.current_match_runs, |c| c.match_current);

    // 3. Special underlines (double / dotted / dashed)
    col = 0;
    while col < cols {
        let cell = &row_cells[col];
        if !matches!(
            cell.underline,
            UnderlineKind::Double | UnderlineKind::Dotted | UnderlineKind::Dashed
        ) {
            col += 1;
            continue;
        }
        let kind = cell.underline;
        let color = cell.underline_color.unwrap_or(cell.fg);
        let start = col;
        col += 1;
        while col < cols {
            let next = &row_cells[col];
            if next.spacer
                || (next.underline == kind && next.underline_color.unwrap_or(next.fg) == color)
            {
                col += 1;
            } else {
                break;
            }
        }
        out.special_underlines.push((start, col - start, kind, color));
    }

    // 4. Glyphs (batched ASCII Run, batched CJK WideRun, native Powerline/Boxdraw, Solo/Wide/Cluster)
    for seg in segment_row(row_cells) {
        match seg {
            RowSeg::Run { start, text, .. } => {
                let style = GlyphStyle::of(&row_cells[start]);
                let face_ix = (style.bold as usize) | ((style.italic as usize) << 1);
                let text = SharedString::from(text);
                run_buf[0] = TextRun {
                    len: text.len(),
                    font: faces[face_ix].clone(),
                    color: style.fg,
                    background_color: None,
                    underline: style.underline_style(),
                    strikethrough: style.strikethrough_style(),
                };
                let force_width = Some(cell_width);
                // Reuse the `ShapedLine` computed in `RunStep::Drift` when no
                // ligature drift occurred, eliminating the duplicate `shape_line`.
                let mut stashed: Option<(usize, usize, gpui::ShapedLine)> = None;
                paint_run(start, text.len(), |step| match step {
                    RunStep::Drift { at, len } => {
                        let shaped =
                            shape_piece(window, run_buf, &text, at, len, font_size, force_width);
                        let drift = run_drift(
                            shaped
                                .runs
                                .iter()
                                .flat_map(|r| r.glyphs.iter().map(|g| g.index)),
                        );
                        if drift.is_none() {
                            stashed = Some((at, len, shaped));
                        }
                        drift
                    }
                    RunStep::Paint { col, at, len } => {
                        let shaped = match stashed.take() {
                            Some((sa, sl, line)) if sa == at && sl == len => line,
                            _ => shape_piece(
                                window,
                                run_buf,
                                &text,
                                at,
                                len,
                                font_size,
                                force_width,
                            ),
                        };
                        let dx = cell_width * (col as f32);
                        out.glyphs.push(CachedGlyphOp::Shaped {
                            dx,
                            dy: px(0.),
                            clip_x: dx,
                            clip_w: cell_width * (len as f32),
                            line: shaped,
                        });
                        None
                    }
                });
            }
            RowSeg::WideRun { start, cells, text } => {
                let style = GlyphStyle::of(&row_cells[start]);
                let face_ix = (style.bold as usize) | ((style.italic as usize) << 1);
                let text = SharedString::from(text);
                run_buf[0] = TextRun {
                    len: text.len(),
                    font: faces[face_ix].clone(),
                    color: style.fg,
                    background_color: None,
                    underline: style.underline_style(),
                    strikethrough: style.strikethrough_style(),
                };
                let shaped = window.text_system().shape_line(
                    text,
                    font_size,
                    run_buf,
                    Some(cell_width * 2.),
                );
                let dx = cell_width * (start as f32);
                out.glyphs.push(CachedGlyphOp::Shaped {
                    dx,
                    dy: px(0.),
                    clip_x: dx,
                    clip_w: cell_width * (cells as f32),
                    line: shaped,
                });
            }
            RowSeg::Solo { col } => {
                let cell = &row_cells[col];
                let style = GlyphStyle::of(cell);
                let native = if let Some(shape) = PowerlineShape::of(cell.c) {
                    out.glyphs.push(CachedGlyphOp::Powerline {
                        col,
                        shape,
                        fg: style.fg,
                    });
                    true
                } else if matches!(cell.c, '\u{2500}'..='\u{259f}')
                    && crate::render::boxdraw::glyph(
                        cell.c,
                        Bounds::new(point(px(0.), px(0.)), size(cell_width, line_height)),
                        1.,
                    )
                    .is_some()
                {
                    out.glyphs.push(CachedGlyphOp::BoxDraw {
                        col,
                        c: cell.c,
                        fg: style.fg,
                    });
                    true
                } else {
                    false
                };
                let (text, solo) = if !native {
                    (char_string(cell.c), true)
                } else {
                    match native_cell_residue(&style) {
                        None => continue,
                        Some(c) => (char_string(c), false),
                    }
                };
                push_measured_glyph_op(
                    window,
                    cx,
                    row_cells,
                    col,
                    1,
                    text,
                    None,
                    solo,
                    style,
                    cell_width,
                    line_height,
                    font_size,
                    faces,
                    primary_ids,
                    run_buf,
                    out,
                );
            }
            RowSeg::Wide { start, cells, text } => {
                let style = GlyphStyle::of(&row_cells[start]);
                push_measured_glyph_op(
                    window,
                    cx,
                    row_cells,
                    start,
                    cells,
                    text,
                    Some(cell_width * 2.),
                    false,
                    style,
                    cell_width,
                    line_height,
                    font_size,
                    faces,
                    primary_ids,
                    run_buf,
                    out,
                );
            }
            RowSeg::Cluster {
                col,
                cells,
                text,
                wide_base,
            } => {
                let style = GlyphStyle::of(&row_cells[col]);
                let force_width =
                    (cells == 2).then(|| cell_width * if wide_base { 2. } else { 1. });
                push_measured_glyph_op(
                    window,
                    cx,
                    row_cells,
                    col,
                    cells,
                    SharedString::from(text),
                    force_width,
                    cells == 1,
                    style,
                    cell_width,
                    line_height,
                    font_size,
                    faces,
                    primary_ids,
                    run_buf,
                    out,
                );
            }
        }
    }
    out.dirty = false;
}

#[allow(clippy::too_many_arguments)]
fn push_measured_glyph_op(
    window: &mut Window,
    cx: &mut App,
    row_cells: &[RenderCell],
    start: usize,
    cells: usize,
    text: SharedString,
    force_width: Option<Pixels>,
    solo: bool,
    style: GlyphStyle,
    cell_width: Pixels,
    line_height: Pixels,
    font_size: Pixels,
    faces: &[Font; 4],
    primary_ids: &mut [Option<gpui::FontId>; 4],
    run_buf: &mut [TextRun; 1],
    out: &mut CachedRow,
) {
    let face_ix = (style.bold as usize) | ((style.italic as usize) << 1);
    run_buf[0] = TextRun {
        len: text.len(),
        font: faces[face_ix].clone(),
        color: style.fg,
        background_color: None,
        underline: style.underline_style(),
        strikethrough: style.strikethrough_style(),
    };

    let mut shaped = window
        .text_system()
        .shape_line(text.clone(), font_size, run_buf, force_width);
    let bounds = ink_bounds(cx, &shaped, &text, font_size);
    let ink = bounds.map(|b| b.origin.x + b.size.width);
    let measured = ink_covers_segment(ink, &text);
    let is_icon = measured && text.chars().next().is_some_and(is_icon);
    let room_after = segment_has_room_after(row_cells, start, cells, is_icon);
    let budget = seg_budget(solo, measured, cells, room_after, cell_width);
    let dx_base = cell_width * (start as f32);
    let mut dx = dx_base;
    let mut dy = px(0.);
    let mut clip_w = budget;
    if !solo && !measured && cells == 2 {
        // Multi-codepoint 2-cell clusters (e.g. `❤️`, `👨‍💻`, `👍🏽`, `🇺🇸`, `1️⃣`):
        // `ink_bounds` only measures the first codepoint, which misses the
        // full cluster extent (and unligated multi-glyph fallback widths).
        // Fit against the larger of `ink` and `shaped.width` so wide clusters
        // scale cleanly into their 2-cell budget instead of clipping.
        let extent = ink.map_or(shaped.width, |i| i.max(shaped.width));
        let scale = fit_scale(extent, budget);
        if scale != 1. {
            shaped = window.text_system().shape_line(
                text.clone(),
                font_size * scale,
                run_buf,
                force_width,
            );
        }
    } else if let (Some(ink), Some(bounds)) = (ink, bounds) {
        let icon = INK_BOUNDS_ARE_INK
            && is_icon
            && shaped.runs.first().is_some_and(|r| {
                r.font_id
                    != *primary_ids[face_ix]
                        .get_or_insert_with(|| window.text_system().resolve_font(&faces[face_ix]))
            });
        let (scale, room) = if icon {
            let height = font_size.min(line_height);
            let (scale, spans) = icon_fit(bounds, cell_width, cells, room_after, height);
            let room = size(cell_width * spans as f32, line_height);
            (scale, Some((room, spans == cells)))
        } else {
            (fit_scale(ink, budget), None)
        };
        if scale != 1. {
            shaped = window.text_system().shape_line(
                text.clone(),
                font_size * scale,
                run_buf,
                force_width,
            );
        }
        if let Some((room, centre_x)) = room {
            let off = icon_offset(bounds, scale, room, centre_x, shaped.ascent, shaped.descent);
            dx += off.x;
            dy += off.y;
            clip_w = room.width;
        }
    }
    out.glyphs.push(CachedGlyphOp::Shaped {
        dx,
        dy,
        clip_x: dx_base,
        clip_w,
        line: shaped,
    });
}

pub(super) fn build_faces(
    base_font: &Font,
    bold_font: Option<&Font>,
    italic_font: Option<&Font>,
) -> [Font; 4] {
    [
        build_font(base_font, false, false),
        build_font(bold_font.unwrap_or(base_font), true, false),
        build_font(italic_font.unwrap_or(base_font), false, true),
        build_font(bold_font.unwrap_or(base_font), true, true),
    ]
}

pub(super) fn paint_glyphs(
    window: &mut Window,
    cx: &mut App,
    geom: &CellGeom,
    buf: &[RenderCell],
    font_size: Pixels,
    base_font: &Font,
    bold_font: Option<&Font>,
    italic_font: Option<&Font>,
) {
    let faces = build_faces(base_font, bold_font, italic_font);
    let mut primary_ids: [Option<gpui::FontId>; 4] = [None; 4];
    let run_buf = &mut [TextRun {
        len: 0,
        font: base_font.clone(),
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    let scale = window.scale_factor();
    let mut scratch = CachedRow::default();
    for row in 0..geom.rows {
        let row_base = row * geom.cols;
        let row_cells = &buf[row_base..row_base + geom.cols];
        build_cached_row(
            window,
            cx,
            row_cells,
            geom.cell_width,
            geom.line_height,
            font_size,
            &faces,
            &mut primary_ids,
            run_buf,
            &mut scratch,
        );
        scratch.paint_glyphs(window, cx, geom, row, scale);
    }
}

pub(super) fn paint_cursor(
    window: &mut Window,
    geom: &CellGeom,
    cursor: Option<(usize, usize, anterm_ui_kit::config::CursorStyle)>,
    focused: bool,
    cursor_visible: bool,
    caret: Hsla,
) {
    use anterm_ui_kit::config::CursorStyle;
    let Some((row, col, style)) = cursor else {
        return;
    };
    let rect = geom.cell_rect(row, col, 1);
    if !focused {
        window.paint_quad(outline(rect, caret, BorderStyle::Solid));
        return;
    }
    if !cursor_visible {
        return;
    }
    match style {
        // A focused block is drawn as reverse video by `invert_cursor_cell`
        // before the glyphs go down, so the caret is opaque and the character
        // under it stays readable. Nothing left to paint here.
        CursorStyle::Block => {}
        CursorStyle::Bar => {
            let w = (geom.cell_width * 0.15).max(px(1.)).min(px(3.));
            let bar = Bounds::new(rect.origin, size(w, rect.size.height));
            window.paint_quad(fill(bar, caret));
        }
        CursorStyle::Underline => {
            let h = (geom.line_height * 0.12).max(px(1.)).min(px(3.));
            let y = rect.origin.y + rect.size.height - h;
            let line = Bounds::new(point(rect.origin.x, y), size(rect.size.width, h));
            window.paint_quad(fill(line, caret));
        }
    }
}

/// Turn the cell under a focused block cursor into reverse video: opaque caret
/// fill, glyph redrawn on top of it. Every other terminal paints a block this
/// way, and a translucent tint would give back most of the contrast the theme
/// conditioned the caret to carry.
pub(super) fn invert_cursor_cell(
    buf: &mut [RenderCell],
    cols: usize,
    row: usize,
    col: usize,
    colors: &PaintColors,
) {
    let ink = anterm_ui_kit::presets::caret_ink(colors.caret, colors.default_bg, colors.default_fg);
    // A TUI can park the cursor on the second half of a wide character. The
    // glyph lives on the lead cell, so inverting the spacer would fill half a
    // character with the caret and leave the glyph in its own colour across
    // both halves. Step back to the cell that owns the glyph.
    let col = match buf.get(row * cols + col) {
        Some(c) if c.spacer && col > 0 => col - 1,
        _ => col,
    };
    let Some(cell) = buf.get_mut(row * cols + col) else {
        return;
    };
    cell.bg = colors.caret;
    cell.draw_bg = true;
    cell.fg = ink;
    // Keep the underline's own alpha: an OSC 8 link's held-back dots stay
    // faint under the caret rather than turning into a full-strength line.
    if let Some(under) = cell.underline_color.as_mut() {
        *under = ink.opacity(under.a);
    }
    // A wide character's trailing spacer is absorbed into the lead cell's
    // background run by `paint_backgrounds`, so the fill already covers both
    // columns and only the lead cell carries the glyph.
}

/// Whether an IME preedit has anything to draw. `paint_marked` blanks the cell
/// under the cursor to the theme's background before it lays the preedit
/// down, which also covers the block cursor's reverse-video cell. A preedit of
/// nothing but blanks (a composition an IME opened and left empty-looking —
/// Windows IMEs can leave one behind, and gpui there never hears the
/// composition end) would therefore paint a theme-coloured hole with an
/// underline where the character and the caret should be, and hold it on
/// every cell the cursor visits (#966). Nothing to show means nothing to
/// paint: the grid, and the caret on it, stay as they are.
pub(super) fn preedit_has_ink(marked: &str) -> bool {
    use unicode_width::UnicodeWidthChar as _;
    marked
        .chars()
        .any(|c| !c.is_whitespace() && !c.is_control() && c.width().unwrap_or(0) > 0)
}

pub(super) fn paint_marked(
    window: &mut Window,
    cx: &mut App,
    geom: &CellGeom,
    cursor: Option<(usize, usize)>,
    marked: &str,
    font_size: Pixels,
    base_font: &Font,
    default_fg: Hsla,
    default_bg: Hsla,
) {
    if !preedit_has_ink(marked) {
        return;
    }
    let Some((row, col)) = cursor else {
        return;
    };
    let x = geom.origin.x + geom.cell_width * (col as f32);
    let y = geom.origin.y + geom.line_height * (row as f32);
    let run = TextRun {
        len: marked.len(),
        font: base_font.clone(),
        color: default_fg,
        background_color: None,
        underline: Some(gpui::UnderlineStyle {
            thickness: px(1.),
            color: Some(default_fg),
            wavy: false,
        }),
        strikethrough: None,
    };
    let shaped = window.text_system().shape_line(
        SharedString::from(marked.to_owned()),
        font_size,
        &[run],
        None,
    );
    let bg_rect = Bounds::new(point(x, y), size(shaped.width, geom.line_height));
    window.paint_quad(fill(bg_rect, default_bg));
    _ = shaped.paint(
        point(x, y),
        geom.line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}
