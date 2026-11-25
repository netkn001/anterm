use std::path::PathBuf;

#[cfg(test)]
use gpui::{Pixels, px};
use gpui::{Hsla, SharedString, Window};
use gpui_kit::component::ActiveTheme as _;

#[cfg(test)]
use crate::core::tag::TagDef;
use crate::core::tag::{SidebarTags, TagColor, TagId};
use crate::ui::workspace::app::Tab;
use crate::ui::workspace::tab_strip::measure_text;
use anterm_i18n::{L10nKey, t};

/// Tabular numerals, so a column of diff counts lines up digit for digit.
pub(super) fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

/// The row chrome the text budget has to be measured around. These are the
/// numbers the layout below is built from, not a second guess at it — a row
/// that elides against a budget wider than it really has falls back to CSS
/// truncation, which drops the tail this whole module exists to keep.
pub(super) mod row_metrics {
    /// `border_r_1` on the sidebar itself.
    pub(in crate::ui::workspace::tab_sidebar) const BORDER: f32 = 1.;
    /// `px_3` on the scrolling list that holds the rows.
    pub(in crate::ui::workspace::tab_sidebar) const LIST_PAD: f32 = 12.;
    /// `pl_2` + `pr_2` on the row.
    pub(in crate::ui::workspace::tab_sidebar) const ROW_PAD: f32 = 8.;
    /// The avatar handed to `tab_avatar_plain`.
    pub(in crate::ui::workspace::tab_sidebar) const AVATAR: f32 = 16.;
    /// `gap_2p5` between the row's children.
    pub(in crate::ui::workspace::tab_sidebar) const GAP: f32 = 10.;
    /// The trailing status mark, when the row has one.
    pub(in crate::ui::workspace::tab_sidebar) const STATUS: f32 =
        crate::ui::workspace::tab_strip::ROW_STATUS_SLOT;
    /// The ⌘N badge, when one is shown.
    pub(in crate::ui::workspace::tab_sidebar) const BADGE: f32 = 20.;
    /// The zoom mark, when the tab has a pane zoomed over the others.
    pub(in crate::ui::workspace::tab_sidebar) const ZOOM: f32 = 16.;
    /// `gap_1p5`, between a branch and the counts after it.
    pub(in crate::ui::workspace::tab_sidebar) const META_GAP: f32 = 6.;

    /// What a row can spend on text, before the badge is taken out.
    pub(in crate::ui::workspace::tab_sidebar) const fn text_budget(width: f32) -> f32 {
        width - BORDER - 2. * LIST_PAD - 2. * ROW_PAD - AVATAR - GAP
    }
}

/// The narrowest a row's title is allowed to get before the working directory
/// beside it stops taking room, and the narrowest that path may be drawn at:
/// below this it is an ellipsis and a slash, which names no directory.
pub(super) const ROW_TITLE_FLOOR: f32 = 48.;
pub(super) const ROW_CWD_FLOOR: f32 = 24.;

/// Semantic theme color for a tag dot, derived strictly from `cx.theme()`
/// palette roles so every tag harmonizes with light, dark, and custom presets.
pub(crate) fn tag_color_dot(color: TagColor, cx: &gpui::App) -> Option<Hsla> {
    let theme = cx.theme();
    match color {
        TagColor::None => None,
        TagColor::Red => Some(theme.red),
        TagColor::Orange => Some(theme.red.blend(theme.yellow.opacity(0.45))),
        TagColor::Amber => Some(theme.yellow),
        TagColor::Green => Some(theme.green),
        TagColor::Teal => Some(theme.cyan),
        TagColor::Blue => Some(theme.blue),
        TagColor::Purple => Some(theme.magenta.blend(theme.blue.opacity(0.45))),
        TagColor::Pink => Some(theme.magenta),
    }
}

/// Complete theme-derived visual styling for a colored tag across filter pills,
/// section headers, the edit-tag popover, and the command palette.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TagVisual {
    pub(crate) dot: Hsla,
    pub(crate) pill_bg: Hsla,
    pub(crate) pill_bg_hover: Hsla,
    pub(crate) pill_bg_selected: Hsla,
    pub(crate) pill_border: Hsla,
    pub(crate) pill_border_selected: Hsla,
    pub(crate) text: Hsla,
    pub(crate) text_selected: Hsla,
    pub(crate) on_swatch: Hsla,
}

/// Computes contrast-verified semantic surface, border, dot, and text colors
/// for `color`, returning `None` for [`TagColor::None`].
pub(crate) fn tag_visual(color: TagColor, cx: &gpui::App) -> Option<TagVisual> {
    let dot = tag_color_dot(color, cx)?;
    let theme = cx.theme();
    let dark = theme.is_dark();
    let surface = theme.sidebar;

    let bg_alpha = if dark { 0.15 } else { 0.10 };
    let hover_alpha = if dark { 0.24 } else { 0.18 };
    let selected_alpha = if dark { 0.30 } else { 0.22 };

    Some(TagVisual {
        dot,
        pill_bg: dot.opacity(bg_alpha),
        pill_bg_hover: dot.opacity(hover_alpha),
        pill_bg_selected: dot.opacity(selected_alpha),
        pill_border: dot.opacity(if dark { 0.28 } else { 0.22 }),
        pill_border_selected: dot.opacity(if dark { 0.65 } else { 0.52 }),
        text: crate::ui::chrome::presets::tinted_ink(dot, surface, bg_alpha),
        text_selected: crate::ui::chrome::presets::tinted_ink(dot, surface, selected_alpha),
        on_swatch: crate::ui::chrome::presets::on_swatch_ink(dot),
    })
}

/// Localized display label for a [`TagColor`].
pub(crate) fn tag_color_label(color: TagColor) -> &'static str {
    let key = match color {
        TagColor::None => L10nKey::TagColorNone,
        TagColor::Red => L10nKey::TagColorRed,
        TagColor::Orange => L10nKey::TagColorOrange,
        TagColor::Amber => L10nKey::TagColorAmber,
        TagColor::Green => L10nKey::TagColorGreen,
        TagColor::Teal => L10nKey::TagColorTeal,
        TagColor::Blue => L10nKey::TagColorBlue,
        TagColor::Purple => L10nKey::TagColorPurple,
        TagColor::Pink => L10nKey::TagColorPink,
    };
    t(key)
}

/// Returns the valid [`TagDef`] entries attached to `tab`, in the tab's tag
/// order, dropping any id no longer in `catalog`.
#[cfg(test)]
pub(super) fn resolve_tab_tags<'a>(tab: &Tab, catalog: &'a SidebarTags) -> Vec<&'a TagDef> {
    tab.tags
        .borrow()
        .iter()
        .filter_map(|id| catalog.get(*id))
        .collect()
}

/// One sidebar section: either tabs grouped under a tag (`Some(tag_id)`) or
/// untagged tabs (`None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TagSection {
    pub(super) tag: Option<TagId>,
    pub(super) tabs: Vec<usize>,
}

/// Groups `tabs` into sidebar sections by tag.
///
/// - When `active_filter` is `Some(filter)`: returns a single section for
///   `filter` containing every tab carrying `filter` (empty vec if none match).
/// - When `active_filter` is `None`: assigns each tab to its primary valid tag
///   (first id in `tab.tags` that exists in `catalog`), emitting one section
///   per tag in `catalog.tags` order that has at least one tab, followed by a
///   trailing `None` section for untagged tabs (when non-empty).
pub(super) fn sidebar_tag_sections(
    tabs: &[Tab],
    catalog: &SidebarTags,
    active_filter: Option<TagId>,
) -> Vec<TagSection> {
    if let Some(filter) = active_filter {
        let matched: Vec<usize> = (0..tabs.len())
            .filter(|&i| tabs[i].tags.borrow().contains(&filter))
            .collect();
        return if matched.is_empty() {
            Vec::new()
        } else {
            vec![TagSection {
                tag: Some(filter),
                tabs: matched,
            }]
        };
    }

    let primary_tags: Vec<Option<TagId>> = tabs
        .iter()
        .map(|tab| {
            tab.tags
                .borrow()
                .iter()
                .copied()
                .find(|id| catalog.contains(*id))
        })
        .collect();

    let mut sections = Vec::new();
    for def in &catalog.tags {
        let grouped: Vec<usize> = primary_tags
            .iter()
            .enumerate()
            .filter_map(|(i, primary)| (*primary == Some(def.id)).then_some(i))
            .collect();
        if !grouped.is_empty() {
            sections.push(TagSection {
                tag: Some(def.id),
                tabs: grouped,
            });
        }
    }

    let untagged: Vec<usize> = primary_tags
        .iter()
        .enumerate()
        .filter_map(|(i, primary)| primary.is_none().then_some(i))
        .collect();
    if !untagged.is_empty() {
        sections.push(TagSection {
            tag: None,
            tabs: untagged,
        });
    }

    sections
}

/// What a diff's counts occupy on a line, measured against real glyphs: the
/// two numbers, the gap between them when both are drawn, and the gap that
/// separates them from the branch. They never wrap and never shrink, so this
/// is the width a branch has to be elided around.
pub(super) fn counts_width(
    ts: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    status: &crate::git::status::GitStatus,
) -> f32 {
    let mut w = 0.;
    if status.added > 0 {
        w += measure_text(ts, font, size, &format!("+{}", status.added));
    }
    if status.removed > 0 {
        w += measure_text(ts, font, size, &format!("−{}", status.removed));
    }
    if status.added > 0 && status.removed > 0 {
        w += row_metrics::META_GAP;
    }
    if w > 0. {
        w += row_metrics::META_GAP;
    }
    w
}

/// What a sidebar row rendered, next to what it had to leave out, so the
/// hover card can be built by comparison instead of deriving the same strings
/// a second time.
pub(super) struct SidebarRowShown {
    /// The elided title, and the full string it came from. `None` when the
    /// row is showing a placeholder (`Shell 3`) rather than a real title,
    /// which nothing can expand.
    pub(super) title: Option<(SharedString, SharedString)>,
    pub(super) branch: Option<(SharedString, SharedString, u32, u32)>,
    pub(super) cwd: Option<(SharedString, SharedString)>,
}

/// Every detail a sidebar row could not fit, collected so the hover card can
/// be rendered from cloneable data.
#[derive(Clone)]
pub(super) struct SidebarInfo {
    /// Full path, when the row's title was elided.
    pub(super) title: Option<SharedString>,
    /// Full branch plus diff counts, when the row's branch was elided.
    pub(super) branch: Option<(SharedString, u32, u32)>,
    /// Full working directory, when the row's inline cwd was elided.
    pub(super) cwd: Option<SharedString>,
    /// Remote host, when the avatar only shows a dot for it.
    pub(super) host: Option<SharedString>,
}

/// How far to move the sidebar's scroll offset so a newly active row shows.
/// A row with any part in view stays put — clicking a row must not move the
/// list under the pointer; one out of view comes in at the nearest edge.
#[cfg(test)]
pub(super) fn reveal_shift(row: (Pixels, Pixels), lead: Pixels, view: (Pixels, Pixels)) -> Pixels {
    let ((top, bottom), (view_top, view_bottom)) = (row, view);
    if bottom <= view_top {
        view_top - (top - lead)
    } else if top >= view_bottom {
        view_bottom - bottom
    } else {
        px(0.)
    }
}

/// Full tab-index permutation after visible slot `from` is dropped onto slot
/// `to`. When a tag filter hides some tabs, hidden tabs stay at their original
/// positions while the visible subset permutes across its own slots.
pub(super) fn reordered_rows(
    total: usize,
    visible: &[usize],
    from: usize,
    to: usize,
) -> Option<Vec<usize>> {
    let (&moved, &anchor) = (visible.get(from)?, visible.get(to)?);
    if moved == anchor {
        return None;
    }
    let mut reordered_visible: Vec<usize> = visible.to_vec();
    reordered_visible.remove(from);
    let at = reordered_visible.iter().position(|&i| i == anchor)? + usize::from(to > from);
    reordered_visible.insert(at, moved);

    let mut out: Vec<usize> = (0..total).collect();
    for (&slot_pos, &new_tab) in visible.iter().zip(&reordered_visible) {
        if slot_pos < out.len() {
            out[slot_pos] = new_tab;
        }
    }
    Some(out)
}

/// Where a click on a tab's diff counts opens the overlay: the focused pane's
/// repo.
pub(super) fn git_click(
    tab: &Tab,
    window: &Window,
    cx: &gpui::App,
) -> Option<(crate::ui::remote::host_ops::HostId, PathBuf)> {
    tab.pane.focused_or_first(window, cx).and_then(|leaf| {
        let view = leaf.read(cx);
        let cwd = view.git_status_cwd()?.to_path_buf();
        Some((view.host_id(), cwd))
    })
}
