use std::sync::Arc;

use gpui::{
    BorderStyle, Bounds, Corners, Edges, Hsla, Pixels, Window, fill, point, px, quad,
};

use anterm_core::git::log::{CommitPage, Edge, GraphRow, Lane, RefKind};

use anterm_ui_kit::presets::{LANE_SLOTS, Lanes};

/// One commit per row. 24px rather than the file list's 26: a graph row has no
/// icon column, and the vertical pitch wants to stay close to the lane pitch or
/// the diagonal of a merge reads as a much shallower angle than it is.
///
/// It also has to contain the line the subject is set on — gpui leads text at
/// phi, so `TEXT` occupies `round(14 × 1.618) = 23px`, and 24 is the first
/// even pitch above it. Both numbers were 4 smaller while the section was on
/// the panel's old 12px step.
///
/// A taller row against an unchanged [`GRAPH_LANE_W`] steepens a merge's
/// diagonal, which is the safe direction: the failure this pitch is guarding
/// against is an angle read as *shallower* than the jump it draws.
///
/// Now 26, the file list's own pitch: the section reads as one more list of
/// rows under the groups rather than as a denser instrument bolted on below.
pub(super) const GRAPH_ROW_H: f32 = 26.;

/// Horizontal distance between lane centres.
pub(super) const GRAPH_LANE_W: f32 = 12.;

/// Inset before the first lane centre, and the gap between the gutter and the
/// text column.
const GRAPH_PAD_L: f32 = 6.;
const GRAPH_PAD_R: f32 = 6.;

/// An ordinary node is a 3px disc and a lane line is 1.5px wide: the dot is a
/// quarter of the row's height, which is enough to read as a bead on a string
/// without closing the gap to the row above. Merges and roots are drawn from
/// the same radius rather than from a second vocabulary.
///
/// It stays 3 while the row grows, because what a bead is measured against is
/// the string it is on — `GRAPH_LANE_W`, which has not moved. A quarter is the
/// floor the test below holds it to.
///
/// Since the section went hairline: a 7px node on a 1px string, and the ring a
/// node wears is a touch heavier than the string so it closes cleanly.
pub(super) const GRAPH_DOT_R: f32 = 3.5;
pub(super) const GRAPH_LINE_W: f32 = 1.;
const GRAPH_RING_W: f32 = 1.2;

/// Most of the panel's width belongs to the message. Thirty percent is what
/// leaves six lanes at the 280px default (five at 260) and still keeps a
/// readable column.
pub(super) const GRAPH_GUTTER_SHARE: f32 = 0.30;

/// Lanes are capped by what the panel can show, never by what history did.
pub(super) const GRAPH_MIN_LANES: usize = 3;
pub(super) const GRAPH_MAX_LANES: usize = LANE_SLOTS;

/// The cap can never exceed the palette, or two columns on screen would come
/// out the same colour and the whole point of colouring by lane is lost.
const _: () = assert!(GRAPH_MAX_LANES <= LANE_SLOTS);

/// How many lanes fit, given the panel's width.
///
/// A pure projection over the width, deliberately: dragging the panel narrower
/// must not re-run the layout pass or renumber a colour.
pub(super) fn max_lanes(panel_w: f32) -> usize {
    // The share buys the whole gutter, insets included — budgeting only the
    // lane strip would overrun it by a lane at every width.
    let fit = ((panel_w * GRAPH_GUTTER_SHARE - GRAPH_PAD_L - GRAPH_PAD_R) / GRAPH_LANE_W).floor();
    if !fit.is_finite() {
        return GRAPH_MIN_LANES;
    }
    (fit as usize).clamp(GRAPH_MIN_LANES, GRAPH_MAX_LANES)
}

/// Fold a true lane onto a visible column.
///
/// Everything past the cap shares the last column. Pure projection, never fed
/// back into the layout: the same page re-projects for free at any width, with
/// no recomputation and no colour changing under the reader.
pub(super) fn project(lane: Lane, max_lanes: usize) -> Lane {
    lane.min(max_lanes.saturating_sub(1) as Lane)
}

/// Snap a lane centre to a device pixel *before* the quad is built.
///
/// `paint_quad` snaps the bounds it is handed, but each edge independently: an
/// unsnapped centre makes `[cx - w/2, cx + w/2]` round out to one physical
/// pixel on some rows and two on others, and a column of lines that changes
/// width as it scrolls is the most visible artefact this element can produce.
/// Same shape as `powerline_solid_edge` in the terminal renderer.
fn snap(x: f32, scale: f32) -> f32 {
    if !scale.is_finite() || scale <= 0. {
        return x;
    }
    (x * scale).round() / scale
}

/// Centre of a visible column, relative to the gutter's left edge.
pub(super) fn lane_center_x(column: Lane, scale: f32) -> f32 {
    snap(
        GRAPH_PAD_L + GRAPH_LANE_W * column as f32 + GRAPH_LANE_W / 2.,
        scale,
    )
}

/// Total width of the gutter for a given cap.
pub(super) fn gutter_width(max_lanes: usize) -> f32 {
    GRAPH_PAD_L + GRAPH_LANE_W * max_lanes as f32 + GRAPH_PAD_R
}

/// Which colour a visible column draws with.
///
/// A column is the overflow bundle only when the page really is wider than the
/// cap. Deciding that per page rather than per row keeps a column from changing
/// colour as the reader scrolls past the one merge that widened history.
pub(super) fn column_ink(
    column: Lane,
    color: u16,
    max_lanes: usize,
    overflowing: bool,
    lanes: &Lanes,
) -> u32 {
    if overflowing && column as usize + 1 == max_lanes {
        return lanes.overflow;
    }
    lanes.ink[(color as usize).min(LANE_SLOTS - 1)]
}

/// The segments of one row's band, already folded onto visible columns.
///
/// Deduplicated by column, which is what makes the overflow bundle work: five
/// lanes sharing the last column produce one line, not five stacked on each
/// other at five different alphas. Later writers win, and the caller feeds
/// edges in `paint_rank` order, so the node's own line lands over anything
/// merely passing behind it.
#[derive(Default)]
pub(super) struct Band {
    pub(super) top: [Option<u32>; GRAPH_MAX_LANES],
    pub(super) bottom: [Option<u32>; GRAPH_MAX_LANES],
    /// `(left column, right column, colour)`, at most one per pair.
    pub(super) turns: Vec<(Lane, Lane, u32)>,
}

impl Band {
    fn turn(&mut self, a: Lane, b: Lane, ink: u32) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        match self.turns.iter_mut().find(|t| t.0 == lo && t.1 == hi) {
            Some(existing) => existing.2 = ink,
            None => self.turns.push((lo, hi, ink)),
        }
    }
}

/// Fold one row's edges into the segments that will be painted.
pub(super) fn band_of(row: &GraphRow, max_lanes: usize, overflowing: bool, lanes: &Lanes) -> Band {
    let mut band = Band::default();
    let node = project(row.node, max_lanes);
    for edge in &row.edges {
        let ink = |column: Lane| column_ink(column, edge.color(), max_lanes, overflowing, lanes);
        match *edge {
            Edge::Pass { lane, .. } => {
                let c = project(lane, max_lanes);
                band.top[c as usize] = Some(ink(c));
                band.bottom[c as usize] = Some(ink(c));
            }
            Edge::In { from, .. } => {
                let c = project(from, max_lanes);
                band.top[c as usize] = Some(ink(c));
                if c != node {
                    band.turn(c, node, ink(c));
                }
            }
            Edge::Out { to, .. } => {
                let c = project(to, max_lanes);
                band.bottom[c as usize] = Some(ink(c));
                if c != node {
                    band.turn(node, c, ink(c));
                }
            }
        }
    }
    band
}

/// Everything the paint closure needs, snapshotted at render time so nothing
/// reaches back into the view from inside the frame.
pub(super) struct GraphPaint {
    pub(super) page: Arc<CommitPage>,
    pub(super) max_lanes: usize,
    pub(super) overflowing: bool,
    pub(super) lanes: Lanes,
    /// The fill behind a hollow node, so a merge reads as a ring and not as a
    /// disc with a hole punched through to whatever is under the panel. It is
    /// the panel's own opaque sidebar fill rather than `theme.background`: the
    /// section is flush with the panel, and `background` carries the window's
    /// transparency when one is configured, which would let the lane line show
    /// straight down the middle of the node.
    pub(super) surface: Hsla,
    /// The selected row and the fill its band paints under the node, so a
    /// hollow node's hole matches the selection band it sits on instead of
    /// punching through to the resting surface. Hover is not covered — it
    /// lives in gpui's element state, which a paint closure cannot read — so
    /// a hovered ring keeps the resting hole; one step of fill under a 3px
    /// hole, against a whole selected band showing the wrong colour.
    pub(super) selected: Option<usize>,
    pub(super) selected_surface: Hsla,
    /// Whether a "load more" band follows the last row.
    pub(super) more: bool,
    /// Set when the whole page is one lane. A straight line of history has no
    /// branch identity to colour, so it is drawn in neutrals: a hairline
    /// string and muted rings, with only HEAD filled in ink.
    pub(super) linear: Option<Neutral>,
}

/// The inks a single-lane page is painted in.
#[derive(Clone, Copy)]
pub(super) struct Neutral {
    pub(super) line: Hsla,
    pub(super) ring: Hsla,
    pub(super) head: Hsla,
}

/// Paint the whole gutter in one pass.
///
/// Deliberately *not* wrapped in `paint_layer` per line, which is what Zed's
/// own graph does. A layer is a full-drawable render pass; a dozen of them per
/// frame is a dozen. Overlap ordering is already handled — `BoundsTree` hands
/// every overlapping primitive an increasing order — and within a row the
/// caller has sorted edges by `paint_rank` so the node's line is last. If a
/// future change makes something here look wrong in z, the fix is the sort
/// order, not a layer.
pub(super) fn paint_graph(p: &GraphPaint, bounds: Bounds<Pixels>, window: &mut Window) {
    let started = std::time::Instant::now();
    let scale = window.scale_factor();
    let top = bounds.origin.y.as_f32();
    let left = bounds.origin.x.as_f32();
    let rows = &p.page.rows;

    // The canvas is as tall as the whole list, so most of it is off screen.
    // The mask is the viewport; only the band it allows is worth iterating.
    let mask = window.content_mask().bounds;
    let first = (((mask.origin.y.as_f32() - top) / GRAPH_ROW_H).floor() as isize).max(0) as usize;
    let last = ((((mask.origin.y + mask.size.height).as_f32() - top) / GRAPH_ROW_H).ceil() as isize)
        .max(0) as usize;

    let cx_of = |column: Lane| left + lane_center_x(column, scale);
    let line_ink = |ink: u32| -> Hsla {
        match p.linear {
            Some(n) => n.line,
            None => gpui::rgb(ink).into(),
        }
    };
    let vline = |x: f32, y0: f32, y1: f32| {
        Bounds::from_corners(
            point(px(x - GRAPH_LINE_W / 2.), px(y0)),
            point(px(x + GRAPH_LINE_W / 2.), px(y1)),
        )
    };

    for (i, row) in rows
        .iter()
        .enumerate()
        .take(last.min(rows.len()))
        .skip(first)
    {
        let y0 = top + i as f32 * GRAPH_ROW_H;
        let mid = y0 + GRAPH_ROW_H / 2.;
        let band = band_of(row, p.max_lanes, p.overflowing, &p.lanes);

        for (column, ink) in band.top.iter().enumerate() {
            if let Some(ink) = ink {
                window.paint_quad(fill(vline(cx_of(column as Lane), y0, mid), line_ink(*ink)));
            }
        }
        for (column, ink) in band.bottom.iter().enumerate() {
            if let Some(ink) = ink {
                window.paint_quad(fill(
                    vline(cx_of(column as Lane), mid, y0 + GRAPH_ROW_H),
                    line_ink(*ink),
                ));
            }
        }
        // Cross-lane turns are right angles, which is what tig, lazygit and
        // `git log --graph` all draw and what reads unambiguously at a 12px
        // pitch. Curves would mean paths, and paths mean a render pass each.
        // Swapping them in later touches only this loop: a curve consumes the
        // same `(lo, hi, mid)` a right angle does.
        //
        // The horizontal runs half a line width past both centres, which is
        // exactly what fills the two outside corners the vertical stubs leave
        // open. Without it a turn shows a notch at every elbow.
        for (lo, hi, ink) in &band.turns {
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(
                        px(cx_of(*lo) - GRAPH_LINE_W / 2.),
                        px(mid - GRAPH_LINE_W / 2.),
                    ),
                    point(
                        px(cx_of(*hi) + GRAPH_LINE_W / 2.),
                        px(mid + GRAPH_LINE_W / 2.),
                    ),
                ),
                line_ink(*ink),
            ));
        }

        let node = project(row.node, p.max_lanes);
        let is_head = p
            .page
            .commits
            .get(i)
            .is_some_and(|c| c.refs.iter().any(|r| r.kind == RefKind::Head));
        let ink: Hsla = match p.linear {
            Some(n) if is_head => n.head,
            Some(n) => n.ring,
            None => gpui::rgb(column_ink(
                node,
                row.color,
                p.max_lanes,
                p.overflowing,
                &p.lanes,
            ))
            .into(),
        };
        let cx = cx_of(node);
        let dot = |r: f32| {
            Bounds::from_corners(
                point(px(cx - r), px(mid - r)),
                point(px(cx + r), px(mid + r)),
            )
        };
        // A rounded quad rather than a path: the quad shader's rounding is an
        // exact SDF with analytic anti-aliasing, where `PathBuilder` fills every
        // vertex's `st` with `(0, 1)` and so falls back on 4x MSAA alone. The
        // hollow ones are one bordered quad rather than two concentric fills,
        // because the border is part of that same SDF — stacking would blend the
        // inner edge over the outer one's already-blended edge, and a 3px hole
        // is where that shows.
        let hole = if p.selected == Some(i) {
            p.selected_surface
        } else {
            p.surface
        };
        if row.parents > 1 {
            // A merge is the larger ring. It is the one row shape a reader
            // scans for, and an outline reads at 9px where a second fill
            // colour does not.
            let r = GRAPH_DOT_R + 1.;
            window.paint_quad(quad(
                dot(r),
                Corners::all(px(r)),
                hole,
                Edges::all(px(GRAPH_RING_W)),
                ink,
                BorderStyle::Solid,
            ));
        } else if is_head {
            // Where you are is the one filled bead on the string.
            window.paint_quad(
                fill(dot(GRAPH_DOT_R), ink).corner_radii(Corners::all(px(GRAPH_DOT_R))),
            );
        } else {
            // Every other commit is a hollow bead, so HEAD is found by shape
            // before it is found by the chip beside it.
            window.paint_quad(quad(
                dot(GRAPH_DOT_R),
                Corners::all(px(GRAPH_DOT_R)),
                hole,
                Edges::all(px(GRAPH_RING_W)),
                ink,
                BorderStyle::Solid,
            ));
        }
    }

    // Past the last row the lanes that are still open get a stub. Without it a
    // page boundary reads as a row of root commits — every line simply ending.
    // Under a "load more" row the stubs run the full band instead, so the graph
    // reads as continuing through the control rather than being cut by it.
    if last > rows.len() && !p.page.open_lanes.is_empty() {
        let y0 = top + rows.len() as f32 * GRAPH_ROW_H;
        for lane in &p.page.open_lanes {
            let column = project(*lane, p.max_lanes);
            let ink = column_ink(column, *lane, p.max_lanes, p.overflowing, &p.lanes);
            let x = cx_of(column);
            let ink = line_ink(ink);
            if p.more {
                window.paint_quad(fill(vline(x, y0, y0 + GRAPH_ROW_H), ink.opacity(0.3)));
            } else {
                // Three steps rather than a gradient: a gradient would be a
                // second `Background` kind for four pixels of ink.
                const STEP: f32 = 3.;
                for (step, alpha) in [0.5f32, 0.3, 0.15].into_iter().enumerate() {
                    let a = y0 + step as f32 * STEP;
                    window.paint_quad(fill(vline(x, a, a + STEP), ink.opacity(alpha)));
                }
            }
        }
    }

    if anterm_ui_kit::perf::enabled() {
        anterm_ui_kit::perf::record("scm.graph.paint", started.elapsed());
    }
}
