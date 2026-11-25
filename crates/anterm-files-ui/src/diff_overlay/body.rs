use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    SharedString, div, prelude::*, px, rems,
};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};

use super::{DiffLoad, DiffOverlayView};
use anterm_ui_kit::config::{Config, DiffViewMode};
use anterm_core::git::status::DecoStatus;
use anterm_ui_kit::git::diff::{
    self as git_diff, CommitLabel, DiffSnapshot, DiffSource, DiffStats, FileDiff, FileStatus,
    LineKind, Truncation,
};
use crate::diff_list::{DiffRow, FileHead, RowAt};
use anterm_editor::diff::{DiffSelection, Side, SplitCell, SplitRow, UnifiedRow};
use anterm_i18n::{L10nKey, t, t_fmt, t_plural};
use crate::right_panel::{HEADING, META, META_MONO, TEXT_MONO, git_badge};
use anterm_ui_kit::rounding;
use crate::scm::path::relative_time;
use crate::scm::status::{status_color, status_glyph};

/// What a row needs to take part in a drag, for the frame it is drawn in.
///
/// Read off the overlay once and moved into the list's item builder, so a step
/// of a drag costs a refcount bump rather than a walk of the patch.
pub(super) struct Drag {
    pub(super) sel: Option<Rc<DiffSelection>>,
    /// Whether a drag is in flight. Rows only listen for pointer movement
    /// while one is: a diff runs to thousands of rows, and a listener each is
    /// worth paying for during a drag and not otherwise.
    pub(super) selecting: bool,
}

impl Drag {
    /// Whether the selection covers this cell. `side` names the column a split
    /// cell sits in, and is `None` for a unified row — a selection made in one
    /// column never lights up the other.
    fn covers(&self, at: &RowAt, side: Option<Side>) -> bool {
        self.sel
            .as_ref()
            .is_some_and(|sel| sel.covers(at.path.as_ref(), at.id, side))
    }

    /// Whether this row is inside the selection at all, whichever column the
    /// drag ran down. What decides whether the row offers to copy it.
    fn holds(&self, at: &RowAt) -> bool {
        self.sel
            .as_ref()
            .is_some_and(|sel| sel.covers(at.path.as_ref(), at.id, sel.side))
    }
}

/// What the overlay's scrolling area holds this frame.
pub(super) enum DiffBody {
    Message(&'static str),
    /// The rows are in [`DiffOverlayState::rows`]; the snapshot rides along
    /// for the few rows whose text is derived from it.
    Rows(Arc<DiffSnapshot>),
}

/// What this frame would flatten its rows from, borrowed from the overlay.
pub(super) struct RowsFrom<'a> {
    pub(super) snap: &'a Arc<DiffSnapshot>,
    pub(super) preview: Option<&'a Arc<FileDiff>>,
    pub(super) mode: DiffViewMode,
    pub(super) focused: Option<usize>,
    pub(super) oversized: bool,
    pub(super) expanded: &'a HashMap<String, bool>,
}

impl RowsFrom<'_> {
    pub(super) fn to_key(&self) -> RowsKey {
        RowsKey {
            snap: Arc::clone(self.snap),
            preview: self.preview.cloned(),
            mode: self.mode,
            focused: self.focused,
            oversized: self.oversized,
            expanded: self.expanded.clone(),
        }
    }
}

/// What [`DiffOverlayState::rows`] was flattened from, kept so the next frame
/// can tell whether it would flatten the same rows again.
pub(super) struct RowsKey {
    pub(super) snap: Arc<DiffSnapshot>,
    preview: Option<Arc<FileDiff>>,
    mode: DiffViewMode,
    focused: Option<usize>,
    oversized: bool,
    expanded: HashMap<String, bool>,
}

impl RowsKey {
    /// Whether the rows built from `from` would be the rows already held.
    ///
    /// The snapshot is compared by pointer first and by contents second: a
    /// probe that found nothing new still lands a fresh `Arc` over an equal
    /// snapshot, and rebuilding every row of the patch for that would undo the
    /// point of keeping them.
    ///
    /// The scalars go first so that the walk of the patch behind that second
    /// comparison is only ever paid to answer a question the cheap fields
    /// have not already answered.
    pub(super) fn describes(&self, from: &RowsFrom<'_>) -> bool {
        self.mode == from.mode
            && self.focused == from.focused
            && self.oversized == from.oversized
            && self.expanded == *from.expanded
            && self.same_preview(from)
            && (Arc::ptr_eq(&self.snap, from.snap) || self.snap == *from.snap)
    }

    /// The preview, by pointer and then by contents — a re-read of an
    /// untracked file lands a fresh `Arc` over bytes that did not change.
    fn same_preview(&self, from: &RowsFrom<'_>) -> bool {
        match (&self.preview, from.preview) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            _ => false,
        }
    }

    /// Points the key at the `Arc`s this frame was asked about, having just
    /// found them equal to the ones held.
    ///
    /// Without this the key goes on holding the snapshot from the last
    /// *rebuild*, so every frame after a probe that found nothing new proves
    /// the two equal the long way — a walk of every line of the patch, once
    /// per wheel event, which is the cost this key exists to avoid.
    pub(super) fn retarget(&mut self, from: &RowsFrom<'_>) {
        self.snap = Arc::clone(from.snap);
        self.preview = from.preview.cloned();
    }
}

/// Tells the list which rows changed, rather than that all of them did.
///
/// `ListState::reset` would drop the scroll position, so collapsing one file
/// would throw the reader back to the top of the tree. The rows either side of
/// an edit are untouched, so the shared prefix and suffix are kept and only
/// what is between them is spliced.
pub(super) fn resync_list(list: &gpui::ListState, old: &[DiffRow], new: &[DiffRow]) {
    let (replaced, with) = spliced_range(old, new);
    if replaced.is_empty() && with == 0 {
        return;
    }
    list.splice(replaced, with);
    if with > 0 {
        let _ = list.clone().with_uniform_item_height(DIFF_LINE_H);
    }
}

/// Which of the old rows were replaced, and by how many new ones.
pub(super) fn spliced_range(old: &[DiffRow], new: &[DiffRow]) -> (std::ops::Range<usize>, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    // Whatever is left of the shorter list once the shared head is off it —
    // the most the shared tail can be, and what keeps the two slices below in
    // step with each other.
    let rest = old.len().min(new.len()) - prefix;
    let suffix = old[old.len() - rest..]
        .iter()
        .rev()
        .zip(new[new.len() - rest..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (prefix..old.len() - suffix, new.len() - prefix - suffix)
}

/// How far a row's pill sits in from the edge of the list: the window's
/// `CONTENT_INSET`, the same 12px the rail and the right panel keep, and the
/// same inset the header's subject starts at.
pub(super) const LIST_INSET: f32 = anterm_ui_kit::window_chrome::CONTENT_INSET;

/// The padding a row keeps inside its pill — the rail's 8px — so a hovered
/// row's fill reaches past its text on both sides. The overlay is a second
/// view of the Changes tab's list, and the two stopped looking like one app
/// when this one drew cards.
pub(super) const ROW_INSET: Pixels = px(8.);

/// The height of a row that is a *file* rather than a line of one: the same
/// 26px the panel gives its file rows.
const FILE_ROW_H: Pixels = px(26.);

/// The radius on a row that lights up under the pointer: the 6px the Changes
/// tab's 26px rows use.
pub(super) const ROW_RADIUS: Pixels = px(6.);

/// The header's pills — the view switch and the way back to the list — are
/// one row tall, so the title bar's controls are the list's rows in another
/// place rather than a smaller widget of their own.
pub(super) const CONTROL_H: Pixels = FILE_ROW_H;

/// The header's close tile: the rail header's 26px tile, not the title bar's
/// 32. The diff header already carries a subject, a switch and a path; the
/// way out should be the quietest shape on it.
pub(super) const CLOSE_TILE: f32 = anterm_ui_kit::window_chrome::RAIL_TILE;

/// The fill under a neutral chip, the same faint ink the graph's HEAD chip
/// wears.
pub(super) const CHIP_FILL: f32 = 0.06;

/// A section label's line in the list — the untracked heading — at the
/// Changes tab's group-header height.
const GROUP_HEADER_H: Pixels = px(22.);

/// The width of every rule the diff views draw: the header's bottom edge,
/// the hunk breaks, the split view's centre and the unified gutter.
pub const HAIRLINE_W: Pixels = px(0.5);

/// The ink of those rules: a hairline at 8% of the foreground, which reads as
/// an edge on the window fill without ever drawing a box. Derived from the
/// foreground rather than `border`, the control-edge role, because a
/// half-pixel of a colour tuned for 1px outlines reads heavier than the rest
/// of the v4 chrome.
pub fn hairline(cx: &gpui::App) -> Hsla {
    cx.theme().foreground.opacity(0.08)
}

/// Tabular figures for counts set in the interface face, so `+12 −3` and a
/// file count keep their width as they change. Mono text needs none of this.
pub fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The height of one line of a patch, in either view.
///
/// Also what the list counts a row it has not laid out yet at. A list knows
/// only the rows it has measured, so without an estimate for the rest a patch
/// of any length reports itself as one screen long — and the scrollbar, which
/// reads that height, drags the reader one screen and stops. The file and hunk
/// rows are a few pixels taller, so the estimate runs a little short until
/// they have been measured; a diff is overwhelmingly its lines.
pub(super) const DIFF_LINE_H: Pixels = px(19.);

/// The rule between one hunk and the last line of the one before it.
///
/// Barely there on purpose: with the cards gone it is the only line left in
/// the list, and it is separating two parts of one file rather than two
/// files. It is the header's hairline, so the view has one kind of line.
fn hunk_rule(cx: &gpui::App) -> Hsla {
    hairline(cx)
}

/// One row, inset the way every row in the list is.
pub(super) fn diff_row_element(
    row: &DiffRow,
    ix: usize,
    drag: &Drag,
    font: &SharedString,
    snap: &Arc<DiffSnapshot>,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
    cx: &mut gpui::App,
) -> AnyElement {
    match row {
        // Stands in for the gap between the groups this list used to be a
        // flex column of.
        DiffRow::Gap => div().w_full().h(gpui::rems(0.75)).into_any_element(),
        DiffRow::Oversized => padded(diff_oversized_notice(snap, cx)),
        DiffRow::FileHeader(head) => padded(diff_file_header(head, font, overlay, cx)),
        DiffRow::HunkHeader { text, leads } => padded(
            div()
                .w_full()
                .px(ROW_INSET)
                .py_1()
                .when(!leads, |h| {
                    h.mt_1().border_t(HAIRLINE_W).border_color(hunk_rule(cx))
                })
                .text_size(rems(META_MONO))
                .font_family(font.clone())
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(text.clone())
                .into_any_element(),
        ),
        // The lines run the full width of the list. A diff is read as a
        // column of code, and code that is inset from both sides reads as a
        // quotation of itself.
        DiffRow::Split { row, at } => copy_menu(
            diff_split_row(row, at, drag, font, overlay, cx),
            ix,
            at,
            drag,
            overlay,
        ),
        DiffRow::Unified { row, at } => copy_menu(
            diff_unified_row(row, at, drag, font, overlay, cx),
            ix,
            at,
            drag,
            overlay,
        ),
        DiffRow::Truncated(reason) => {
            let note = match reason {
                Truncation::PerFile => t_fmt(
                    L10nKey::DiffTruncatedPerFile,
                    &[("limit", &git_diff::MAX_LINES_PER_FILE.to_string())],
                ),
                Truncation::Budget => t(L10nKey::DiffTruncatedBudget).to_string(),
            };
            padded(note_row(note, cx))
        }
        DiffRow::MoreFiles { rest } => {
            padded(note_row(t_plural(L10nKey::DiffMoreFiles, *rest, &[]), cx))
        }
        // A section label, in the shape the Changes tab gives its group
        // headings: 11.5px, medium, muted, sentence case — small, quiet, and
        // carried by the space around it rather than a bar of its own.
        DiffRow::UntrackedHeader { total } => padded(
            div()
                .w_full()
                .flex()
                .items_center()
                .h(GROUP_HEADER_H)
                .px(ROW_INSET)
                .text_size(rems(HEADING))
                .font_weight(FontWeight::MEDIUM)
                .font_features(tabular())
                .text_color(cx.theme().muted_foreground)
                .child(t_plural(L10nKey::DiffUntrackedHeader, *total, &[]))
                .into_any_element(),
        ),
        DiffRow::Untracked { index, path } => {
            padded(diff_untracked_row(*index, path, font, overlay, cx))
        }
        DiffRow::MoreUntracked { rest } => padded(note_row(
            t_plural(L10nKey::DiffMoreUntracked, *rest, &[]),
            cx,
        )),
    }
}

/// The one place a copy is offered by name, on the rows that would be copied.
///
/// A drag says what will be copied; the menu says that copying is a thing you
/// can do. It hangs on the selected rows themselves because with the cards
/// gone there is no longer an element that owns a file's lines, and the
/// overlay root already carries the header's own menu — two of them over one
/// right-click would open two popups.
fn copy_menu(
    row: gpui::Div,
    ix: usize,
    at: &RowAt,
    drag: &Drag,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
) -> AnyElement {
    if !drag.holds(at) {
        return row.into_any_element();
    }
    let overlay = overlay.clone();
    row.id(("diff-row-menu", ix))
        .context_menu(move |menu, _window, _cx| {
            menu.item(PopupMenuItem::new(t(L10nKey::DiffCopySelection)).on_click({
                let overlay = overlay.clone();
                move |_, _window, cx| {
                    overlay.update(cx, |this, cx| this.copy_selection(cx)).ok();
                }
            }))
        })
        .into_any_element()
}

/// Wire one drawn row into the drag: a press starts a selection there, and
/// while one is in flight a move across the row extends it.
fn diff_row_drag<E: InteractiveElement + Styled>(
    el: E,
    at: &RowAt,
    side: Option<Side>,
    drag: &Drag,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
) -> E {
    // From the row, not the overlay's view: a new or deleted file draws
    // unified whatever the view (`diff_list::file_mode`).
    let mode = match side {
        Some(_) => DiffViewMode::Split,
        None => DiffViewMode::Unified,
    };
    // The I-beam is the only standing sign that this text can be taken;
    // nothing else about a row says so until one is dragged.
    let el = el.cursor_text().on_mouse_down(MouseButton::Left, {
        let (overlay, at) = (overlay.clone(), at.clone());
        move |_: &MouseDownEvent, window, cx| {
            overlay
                .update(cx, |this, cx| {
                    this.start_selection(&at, mode, side, window, cx);
                })
                .ok();
        }
    });
    if !drag.selecting {
        return el;
    }
    el.on_mouse_move({
        let (overlay, at) = (overlay.clone(), at.clone());
        move |ev: &MouseMoveEvent, _window, cx| {
            let held = ev.pressed_button;
            overlay
                .update(cx, |this, cx| {
                    this.extend_selection(&at, side, held, cx);
                })
                .ok();
        }
    })
}

/// The margin the file rows keep from the edge of the list.
fn padded(row: AnyElement) -> AnyElement {
    div()
        .w_full()
        .px(px(LIST_INSET))
        .child(row)
        .into_any_element()
}

/// An aside in the list's own voice — a cap that was hit, a tail that was not
/// drawn. Never a row you can act on, so never one that lights up.
fn note_row(text: String, cx: &gpui::App) -> AnyElement {
    div()
        .w_full()
        .px(ROW_INSET)
        .py_1()
        .text_size(rems(META))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn diff_oversized_notice(snap: &DiffSnapshot, cx: &gpui::App) -> AnyElement {
    let stats = snap.stats();
    let text = t_fmt(
        L10nKey::DiffOversizedNotice,
        &[("summary", &oversized_summary(snap, &stats))],
    );
    // The row pill at rest, filled with the hover step: a notice sits in the
    // list's own shape, one quiet rung off the window, rather than in a card.
    let fill = gpui::rgb(cx.global::<anterm_ui_kit::presets::Surfaces>().window.hover);
    div()
        .w_full()
        .px(ROW_INSET)
        .py_2()
        .rounded(rounding::ROW_RADIUS)
        .bg(fill)
        .text_size(rems(META))
        .text_color(cx.theme().muted_foreground)
        .child(text)
        .into_any_element()
}

fn diff_file_header(
    head: &FileHead,
    font: &SharedString,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
    cx: &gpui::App,
) -> AnyElement {
    let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().window;
    let hover = gpui::rgb(sf.hover);
    let deco = deco_status(head.status);
    let (glyph, glyph_color) = (status_glyph(deco), status_color(deco, cx));
    let mut header = h_flex()
        .id(("diff-file-header", head.index))
        .w_full()
        .items_center()
        .gap_2()
        .h(FILE_ROW_H)
        .px(ROW_INSET)
        .rounded(ROW_RADIUS)
        .when(head.expandable, |h| {
            let path = head.path.clone();
            let want = !head.expanded;
            let overlay = overlay.clone();
            h.cursor_pointer()
                .hover(|s| s.bg(hover))
                .on_click(move |_, _window, cx| {
                    let path = path.clone();
                    overlay
                        .update(cx, |this, cx| {
                            this.set_expanded(path, want, cx);
                        })
                        .ok();
                })
                .child(
                    Icon::new(if head.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(anterm_ui_kit::window_chrome::TILE_GLYPH_XS))
                    .text_color(cx.theme().muted_foreground),
                )
        })
        // The status letter is the Changes tab's badge — same cell, weight
        // and ink — so M reads amber and A/U green here exactly as it does
        // one click away.
        .child(git_badge(glyph, glyph_color, font))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT_MONO))
                .font_family(font.clone())
                .text_color(gpui::rgb(sf.text_resting))
                .child(head.shown_path.clone()),
        );
    if head.binary {
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(rems(META))
                .text_color(cx.theme().muted_foreground)
                .child(t(L10nKey::Binary)),
        );
    }
    if head.added > 0 {
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(rems(META_MONO))
                .font_family(font.clone())
                .text_color(cx.theme().success)
                .child(format!("+{}", head.added)),
        );
    }
    if head.removed > 0 {
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(rems(META_MONO))
                .font_family(font.clone())
                .text_color(cx.theme().danger)
                .child(format!("−{}", head.removed)),
        );
    }
    header.into_any_element()
}

// An untracked file has no patch in the snapshot, so it cannot be expanded in
// place the way the files above it are — its contents are read one file at a
// time and shown on their own. The row asks for that read, which until now
// only the Source Control panel could: in the overlay these rows were the only
// files in a list of files that did nothing when clicked.
fn diff_untracked_row(
    index: usize,
    path: &str,
    font: &SharedString,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
    cx: &gpui::App,
) -> AnyElement {
    let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().window;
    let hover = gpui::rgb(sf.hover);
    let for_focus = path.to_string();
    let overlay = overlay.clone();
    h_flex()
        .id(("diff-untracked", index))
        .w_full()
        .items_center()
        .gap_2()
        .h(FILE_ROW_H)
        .px(ROW_INSET)
        .rounded(ROW_RADIUS)
        .cursor_pointer()
        .hover(|s| s.bg(hover))
        .on_click(move |_, window, cx| {
            let for_focus = for_focus.clone();
            overlay
                .update(cx, |this, cx| {
                    this.set_focus(Some(for_focus), window, cx);
                })
                .ok();
        })
        .child(git_badge(
            status_glyph(DecoStatus::Untracked),
            status_color(DecoStatus::Untracked, cx),
            font,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT_MONO))
                .font_family(font.clone())
                .text_color(gpui::rgb(sf.text_resting))
                .child(path.to_string()),
        )
        .into_any_element()
}

fn diff_split_row(
    row: &SplitRow,
    at: &RowAt,
    drag: &Drag,
    font: &SharedString,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
    cx: &gpui::App,
) -> gpui::Div {
    h_flex()
        .w_full()
        .h(DIFF_LINE_H)
        .items_stretch()
        .text_xs()
        .font_family(font.clone())
        .child(diff_split_cell(
            row.left.as_ref(),
            Side::Old,
            at,
            drag,
            overlay,
            cx,
        ))
        .child(div().flex_shrink_0().w(HAIRLINE_W).bg(hunk_rule(cx)))
        .child(diff_split_cell(
            row.right.as_ref(),
            Side::New,
            at,
            drag,
            overlay,
            cx,
        ))
}

fn diff_split_cell(
    cell: Option<&SplitCell>,
    side: Side,
    at: &RowAt,
    drag: &Drag,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
    cx: &gpui::App,
) -> AnyElement {
    let base = h_flex().flex_1().min_w_0().h_full().items_center();
    let Some(cell) = cell else {
        // Blank, but still this row's half of this column. Left inert it is a
        // dead band under the pointer — no I-beam, a press that starts
        // nothing, and a range that visibly stops at the padding and resumes
        // below it. A one-sided change is the ordinary shape of a diff, so
        // that band runs down most of one column.
        let fill = match drag.covers(at, Some(side)) {
            true => cx.theme().selection,
            false => cx.theme().muted.opacity(0.3),
        };
        return diff_row_drag(base, at, Some(side), drag, overlay)
            .bg(fill)
            .into_any_element();
    };
    let (marker, tint) = match (cell.changed, side) {
        (true, Side::Old) => ("−", Some(cx.theme().danger.opacity(0.12))),
        (true, Side::New) => ("+", Some(cx.theme().success.opacity(0.12))),
        (false, _) => (" ", None),
    };
    // A selected cell wears the theme's selection colour in place of its own
    // wash, the way selected text does anywhere else. The `+`/`−` in front of
    // the code still says which side of the change it is.
    let fill = match drag.covers(at, Some(side)) {
        true => Some(cx.theme().selection),
        false => tint,
    };
    diff_row_drag(base, at, Some(side), drag, overlay)
        .when_some(fill, |row, bg| row.bg(bg))
        .child(
            h_flex()
                .flex_shrink_0()
                .w(px(42.))
                .justify_end()
                .pr_1p5()
                .text_color(cx.theme().muted_foreground)
                .child(cell.no.map(|n| n.to_string()).unwrap_or_default()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(format!("{marker} {}", cell.text)),
        )
        .into_any_element()
}

/// One line of the unified view.
///
/// Every measurement it shares with [`diff_split_cell`] is shared on purpose —
/// the same 19px row, the same `text_xs` in the same family, and above all the
/// same `0.12` wash behind an addition and a removal. The two views are one
/// diff seen twice; a different green would read as a different thing.
///
/// What differs is forced by the shape. The line numbers get 34px a side
/// rather than 42 (there are two gutters here in front of one column of text,
/// not one in front of each), and the `+`/`−` gets a column of its own rather
/// than riding in the text: with three kinds of line stacked in one column, an
/// inlined marker would leave the context lines' code starting two characters
/// left of everything else.
fn diff_unified_row(
    row: &UnifiedRow,
    at: &RowAt,
    drag: &Drag,
    font: &SharedString,
    overlay: &gpui::WeakEntity<DiffOverlayView>,
    cx: &gpui::App,
) -> gpui::Div {
    let (marker_color, tint) = match row.kind {
        LineKind::Added => (cx.theme().success, Some(cx.theme().success.opacity(0.12))),
        LineKind::Removed => (cx.theme().danger, Some(cx.theme().danger.opacity(0.12))),
        LineKind::Context => (cx.theme().muted_foreground, None),
    };
    let gutter = |no: Option<u32>| {
        h_flex()
            .flex_shrink_0()
            .w(px(34.))
            .justify_end()
            .pr_1p5()
            .text_color(cx.theme().muted_foreground)
            .child(no.map(|n| n.to_string()).unwrap_or_default())
    };
    let fill = match drag.covers(at, None) {
        true => Some(cx.theme().selection),
        false => tint,
    };
    diff_row_drag(h_flex(), at, None, drag, overlay)
        .w_full()
        .h(DIFF_LINE_H)
        .items_center()
        .text_xs()
        .font_family(font.clone())
        .when_some(fill, |line, bg| line.bg(bg))
        .child(gutter(row.old))
        .child(gutter(row.new))
        // The split view's centre rule, in the one place it still means the
        // same thing: everything left of it is a number, everything right of
        // it is the file.
        .child(
            div()
                .flex_shrink_0()
                .w(HAIRLINE_W)
                .h_full()
                .bg(hunk_rule(cx)),
        )
        .child(
            div()
                .flex_shrink_0()
                .w(px(12.))
                .text_center()
                .text_color(marker_color)
                .child(unified_marker(row.kind)),
        )
        .child(div().flex_1().min_w_0().truncate().child(row.text.clone()))
}

/// Which layout the overlay draws. One setting for the window, not one per
/// overlay: VS Code's `diffEditor.renderSideBySide` is global for the same
/// reason — re-picking on every open is a chore, not a choice.
pub(super) fn view_mode(cx: &gpui::App) -> DiffViewMode {
    cx.try_global::<Config>()
        .map(|cfg| cfg.diff_view)
        .unwrap_or_default()
}

/// The change column. `−` is U+2212, matching the split view: the ASCII hyphen
/// is narrower than `+` and the two columns would not line up.
pub(super) fn unified_marker(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Added => "+",
        LineKind::Removed => "−",
        LineKind::Context => "",
    }
}

/// The git status letter and colour every part of the app agrees on.
///
/// `Copied` and `TypeChanged` have no decoration of their own — porcelain v2's
/// index folds them the same way — so they take the nearest one rather than
/// inventing a `C` and a `T` that appear in the overlay and nowhere else.
pub fn deco_status(status: FileStatus) -> DecoStatus {
    match status {
        FileStatus::Added => DecoStatus::Added,
        FileStatus::Modified => DecoStatus::Modified,
        FileStatus::Deleted => DecoStatus::Deleted,
        FileStatus::Renamed | FileStatus::Copied => DecoStatus::Renamed,
        FileStatus::TypeChanged => DecoStatus::Modified,
        FileStatus::Unmerged => DecoStatus::Conflict,
    }
}

/// The current epoch for a repository, or 0 where nothing has ever bumped one.
/// Zero is the same value a never-touched repository reports, so an overlay
/// that reads it before the global exists simply never looks stale.
pub(super) fn scm_epoch(cx: &gpui::App, host: anterm_ui_kit::host_ops::HostId, root: &Path) -> u64 {
    cx.try_global::<anterm_ui_kit::git::data::ScmData>()
        .map(|data| data.epoch(host, root))
        .unwrap_or(0)
}

/// What the header calls the patch it is showing.
pub(super) struct SourceSubject {
    pub(super) icon: &'static str,
    pub(super) text: String,
    /// Set only where the branch name alone would be ambiguous.
    pub(super) chip: Option<&'static str>,
    pub(super) is_rev: bool,
    /// What the commit is *about*, where whoever opened it knew. An object id
    /// is an address, not a name, and a header with nothing but eight hex
    /// digits leaves the reader to remember which commit that was.
    pub(super) label: Option<CommitLabel>,
}

pub(super) fn source_subject(source: &DiffSource, branch: String) -> SourceSubject {
    let branch_of = |chip| SourceSubject {
        icon: "icons/git-branch.svg",
        text: branch.clone(),
        chip,
        is_rev: false,
        label: None,
    };
    match source {
        // Worktree and Head are both "the branch, right now"; the header for
        // them is what it has always been.
        DiffSource::Worktree | DiffSource::Head => branch_of(None),
        // Staged is the branch too, but a patch that does not match the files
        // on disk — without the chip it is indistinguishable from the above.
        DiffSource::Staged => branch_of(Some(t(L10nKey::ScmChipStaged))),
        DiffSource::Commit { rev, label } => SourceSubject {
            icon: "icons/git-commit.svg",
            text: short_rev(rev),
            chip: None,
            is_rev: true,
            // An empty subject is no more use than no label at all, and a
            // `Default::default()` that leaked through would render as one.
            label: label.clone().filter(|l| !l.subject.is_empty()),
        },
        DiffSource::Range { base, head } => SourceSubject {
            icon: "icons/git-commit.svg",
            text: format!("{}…{}", short_rev(base), short_rev(head)),
            chip: None,
            is_rev: true,
            label: None,
        },
    }
}

/// `Ada · 2h`, the byline under a commit's subject.
///
/// One string rather than two elements: the separator has to disappear along
/// with whichever half is missing, and a `when_some` chain around a middle dot
/// says less than this does.
pub(super) fn label_byline(label: &CommitLabel, now: i64) -> String {
    let when = (label.at > 0).then(|| relative_time(now, label.at));
    match (label.author.trim(), when) {
        ("", Some(when)) => when,
        (author, Some(when)) => format!("{author} · {when}"),
        (author, None) => author.to_string(),
    }
}

pub(super) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Object ids get cut to eight characters; anything else is already a name a
/// person chose, and cutting `origin/main` in half would only hide which it is.
pub(super) fn short_rev(rev: &str) -> String {
    let is_oid = rev.len() >= 40 && rev.chars().all(|c| c.is_ascii_hexdigit());
    match is_oid {
        true => rev[..8].to_string(),
        false => rev.to_string(),
    }
}

pub(super) fn focused_file(snap: &DiffSnapshot, overlay: &DiffOverlayView) -> Option<usize> {
    let path = overlay.focus.as_deref()?;
    snap.files.iter().position(|f| f.path == path)
}

/// The focused path, when it is an *untracked* file — one the snapshot lists
/// by name but holds no patch for. A path that is both (staged half tracked,
/// say) prefers the real patch.
pub(super) fn untracked_focus<'a>(snap: &DiffSnapshot, focus: Option<&'a str>) -> Option<&'a str> {
    let path = focus?;
    if snap.files.iter().any(|f| f.path == path) {
        return None;
    }
    snap.untracked.iter().any(|u| u == path).then_some(path)
}

/// The file the overlay is focused on, for the header's way back to the list.
///
/// An untracked file is focused like any other but has no entry in `files` —
/// its card is synthesized from the file's own bytes — so reading only `files`
/// left the one view with no way out of it: the breadcrumb never drew, and the
/// list was reachable again only by closing the overlay and reopening it.
pub(super) fn focused_name(overlay: &DiffOverlayView) -> Option<String> {
    let DiffLoad::Ready(snap) = &overlay.load else {
        return None;
    };
    if let Some(idx) = focused_file(snap, overlay) {
        return Some(snap.files[idx].path.clone());
    }
    untracked_focus(snap, overlay.focus.as_deref()).map(str::to_string)
}

pub(super) fn empty_snapshot(snap: &DiffSnapshot) -> bool {
    snap.files.is_empty() && snap.untracked.is_empty()
}

pub(super) fn oversized_summary(snap: &DiffSnapshot, stats: &DiffStats) -> String {
    let mut parts = vec![t_plural(L10nKey::DiffChangedFiles, snap.files.len(), &[])];
    let (added, removed) = stats.totals;
    let total_lines = (added + removed) as usize;
    let loaded = stats.retained_lines;
    let budget = stats.budget_exhausted;
    let per_file = stats.per_file_truncated;
    parts.push(match (budget, per_file) {
        (false, false) => t_plural(L10nKey::DiffLines, total_lines, &[]),
        _ => {
            let cap_key = match (budget, per_file) {
                (true, true) => L10nKey::DiffBudgetAndCap,
                (true, false) => L10nKey::DiffBudget,
                _ => L10nKey::DiffPerFileCap,
            };
            t_fmt(
                L10nKey::DiffChangedLines,
                &[
                    ("total", &total_lines.to_string()),
                    ("loaded", &loaded.to_string()),
                    ("cap", t(cap_key)),
                ],
            )
        }
    });
    if stats.untracked_count > 0 {
        parts.push(t_plural(
            L10nKey::DiffUntrackedSummary,
            stats.untracked_count,
            &[],
        ));
    }
    parts.join(", ")
}

/// The de-duplication sets on `AntermApp` are keyed by `(HostId, PathBuf)`, so
/// the source rides along inside the path: two sources over one directory are
/// two independent probes and must not cancel one another.
///
/// `DiffSource::tag` rather than `Debug`, which is what this used to be built
/// from. `Debug` prints a commit's label too, so the same commit opened with a
/// subject in hand and without one would have been two keys and two probes for
/// one patch — the same split `DiffSource`'s own `PartialEq` is written to
/// avoid. The separator is a byte no path contains.
pub(super) fn probe_key(
    host: anterm_ui_kit::host_ops::HostId,
    cwd: &Path,
    source: &DiffSource,
) -> (anterm_ui_kit::host_ops::HostId, PathBuf) {
    let mut tagged = std::ffi::OsString::from(format!("{}\u{1}", source.tag()));
    tagged.push(cwd.as_os_str());
    (host, PathBuf::from(tagged))
}
