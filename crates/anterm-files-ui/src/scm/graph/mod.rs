//! The history section at the foot of the panel.
//!
//! Its job is shape first and text second. 260px leaves room for roughly 26
//! characters beside the lanes, and this repository's commit subjects run to a
//! median of 64 — so what a reader gets here is where the branches are, where
//! they merged, which refs sit where, and how recently anything moved. Reading
//! a whole message is the commit detail view's job, one click away.
//!
//! # What size everything is
//!
//! Three, and no more: 12px for the commit subject and for the
//! conventional-commit type in front of it, which are one run of text and are
//! told apart by tone rather than by size; 11px for the section's own heading
//! and for the "load more" control; and 10.5px for the annotations — the
//! relative age, the commit count, the ref chip. A sidebar list is dense by
//! design, and the graph is the densest thing in this panel.
//!
//! That is also VS Code's own reading of a sidebar graph, and it is why the
//! conventional-commit prefix is cut down to its type and demoted to muted ink
//! rather than left to eat half the line. The lane gutter itself never folds
//! away: the lanes are the graph, and a graph without them is just a list.
//!
//! The section is not a surface of its own. It sits flush on the panel's
//! `theme.sidebar` fill and is separated from the file list above it by a
//! hairline, the way every other band of this panel is. The only colour that
//! gets to be saturated here is a lane's, and the lanes stay in the gutter,
//! below the text; the rows themselves are ink, muted ink and the sidebar's own
//! neutral hover and selection fills.
//!
//! # How it is drawn
//!
//! One `canvas` covering the whole list, absolutely positioned over ordinary
//! interactive rows — never one canvas per row. Every `PrimitiveBatch::Paths`
//! gpui emits ends the current encoder, opens a render pass, clears a
//! drawable-sized intermediate texture, rasterises, resolves MSAA and
//! composites back; forty visible rows would mean forty of those per frame.
//!
//! Being on top costs nothing in event terms: `Canvas::id` returns `None` and
//! it implements no interactivity, so it registers no hitbox in prepaint. The
//! rows underneath keep gpui's native hover, click, context menu and
//! scroll-into-view. This was measured, not assumed — see the G7·0 spike commit.

mod paint;
#[cfg(test)]
mod tests;

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, Focusable as _, Hsla, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, SharedString, Window, canvas, div, prelude::*, px, rems,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use anterm_core::git::log::{
    Commit, CommitPage, GRAPH_PAGE, GraphScope, MAX_GRAPH_COMMITS, RefDeco, RefKind,
};
use anterm_core::git::ops::{GitOp, ResetMode};

use anterm_ui_kit::presets::ActiveLanes;
use crate::right_panel::{
    META, META_MONO, ROW_INSET, TEXT, TEXT_INSET, info_chip, panel_empty_view, panel_search_box,
};
use crate::scm::panel::GROUP_CHEVRON;
use crate::scm::path::{elide_middle, relative_time};
use crate::scm::state::{RepoKey, ScmPanelView};
use anterm_ui_kit::window_chrome::CONTENT_INSET;
use anterm_i18n::{L10nKey, t};

use paint::{GRAPH_ROW_H, GraphPaint, Neutral, gutter_width, max_lanes, paint_graph};

#[cfg(test)]
pub mod row_probe {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static LAST_RENDERED: AtomicUsize = AtomicUsize::new(0);

    pub fn reset() {
        LAST_RENDERED.store(0, Ordering::SeqCst);
    }

    pub(super) fn record(count: usize) {
        LAST_RENDERED.store(count, Ordering::SeqCst);
    }

    pub fn last() -> usize {
        LAST_RENDERED.load(Ordering::SeqCst)
    }
}

/// Rows materialized above and below the visible band, so a fast scroll never
/// outruns the window into blank space, and the "load more" band — laid out
/// one row past the window's end — stays below the fold until it is real.
const GRAPH_WINDOW_MARGIN: usize = 4;

/// Header of the section itself: fold, title, count, filter tile, scope picker.
///
/// The truth, not a wish: the row sets this height explicitly, and what a
/// reader measures is the tallest thing inside it. That used to be the 18px
/// `GRAPH_TILE`; now it is the title's own line — [`META`] leads to
/// `round(12 × 1.618) = 20px` — so 24 is that line plus 2px of air on each
/// side. The resize constants below are counted against it, which is why it
/// has to stay honest.
///
/// 32 since the section took the panel's own band rhythm: the header is where
/// the section starts, and it wants more air than a row of it.
const GRAPH_HEADER_H: f32 = 32.;

/// The header's height while the section is folded. With no rows under it
/// the bar is a single line at the window's foot, and the expanded header's
/// band plus its padding made a strip twice the height of a group header for
/// one word of content.
const GRAPH_HEADER_FOLDED_H: f32 = 24.;

/// The header's controls: the filter tile and the scope picker beside it.
///
/// An 18px square with a 12px glyph, sized against the [`META`] title beside
/// it: two thirds of the box, which is the fill that keeps an icon from
/// rattling around inside its tile.
const GRAPH_TILE: f32 = 18.;
const GRAPH_TILE_GLYPH: f32 = 12.;

/// Space between the header's parts: chevron, title, count, and the controls.
const GRAPH_HEADER_GAP: f32 = 6.;

/// The scope picker's own leading/trailing padding — gpui-component's
/// `.xsmall()` button, `px_1p5`. The header's right padding gives it back so
/// the picker's caret, not its hover box, ends on the text column.
const GRAPH_SCOPE_PAD: f32 = 6.;

/// Space after the age column, inside the section's own inset: a row's own
/// padding, so the ages end on the text column like every other trailing
/// number in the panel.
const GRAPH_ROW_PAD_R: f32 = ROW_INSET;

/// The section's rows sit inside this much of the panel's edge on each side,
/// so a hovered or selected commit wears a rounded band like a file row does
/// rather than a full-bleed stripe. It is the lists' inset: the fill starts
/// where a file row's does, and the row's own padding takes the text on to
/// `TEXT_INSET`.
const GRAPH_SIDE: f32 = CONTENT_INSET;

/// The gutter of a history with no branches in it: the row's own padding, a
/// 9px column for the bead, and 10px to the text — a subject at
/// `TEXT_INSET + 19`. A lane budget sized for the widest history the panel can
/// draw put a single string of beads in a gutter five or six lanes wide, and
/// pushed every subject 50px off the text column for nothing.
const GRAPH_LINEAR_GUTTER: f32 = ROW_INSET + 9. + 10.;

/// Space between a row's subject, its ref chip and its age.
const GRAPH_ROW_GAP: f32 = 10.;

/// The age column's floor: `12m` right-aligned in it. A floor rather than a
/// width, because `11mo` and the CJK units run wider, and a column that clips
/// an age is worse than one that is a few pixels ragged at its leading edge.
const GRAPH_AGE_W: f32 = 24.;

/// Air above the section's header, under its rule, and below its last row,
/// above the window's bottom edge.
const GRAPH_PAD_TOP: f32 = 4.;
const GRAPH_PAD_BOTTOM: f32 = 12.;

/// Resting height of the history section and the range the divider drags it
/// through. The ceiling is a share of the window rather than a constant: the
/// file list has to keep a usable part of a short one.
///
/// Both of the fixed ones are counted in rows against the header and the
/// section's padding: what they mean is a number of commits, not a number of
/// pixels. 303 is `4 + 32 + 9.8 × 26 + 12` — nine commits and most of a tenth,
/// and the fraction is deliberate, because a row cut by the bottom edge is the
/// only honest way a fixed-height list says there is more below it. 132 is
/// `4 + 32 + 3.2 × 26 + 12`, three and a bit, which is the least that still
/// looks like history rather than like a mistake.
///
/// They grew with [`GRAPH_ROW_H`] — 220 and 88 around a 20px row, 260 and 100 around 24 — precisely
/// because they are counted in commits: holding the pixels would have quietly
/// bought the file list 40px by showing two fewer commits. They grew by 16
/// again when the section took the 4/12 padding above its header and below its
/// last row, for the same reason.
const GRAPH_H_DEFAULT: f32 = 303.;
const GRAPH_H_MIN: f32 = 132.;
const GRAPH_H_MAX_RATIO: f32 = 0.65;

/// The divider's grab area, matching `RESIZE_HANDLE_WIDTH` on the other axis.
const GRAPH_HANDLE_H: f32 = 6.;

/// Ref chips are the widest optional thing on a row, so they get a hard cap
/// and lose their middle rather than the message losing its column.
const GRAPH_REF_CHARS: usize = 14;

/// Split a conventional-commit prefix off the subject.
///
/// Returns the type, whether it was marked breaking, and what is left. This
/// repository's subjects spend an average of 12.7 characters on the prefix,
/// which is half of what a 260px panel has to give — and the scope in the
/// middle of it is the part nobody scans for, so it goes. The type stays, in
/// front of the subject and a shade quieter than it.
///
/// Strict on purpose. Only a lowercase ASCII type, an optional parenthesised
/// scope, an optional `!`, then `": "`. `Note: see below` and `TODO: fix` are
/// not conventional commits and keep their whole line.
fn split_conventional(subject: &str) -> (Option<(&str, bool)>, &str) {
    let bytes = subject.as_bytes();
    let type_len = bytes.iter().take_while(|b| b.is_ascii_lowercase()).count();
    // Two is `ci`; past twelve it is prose that happens to start lowercase.
    if !(2..=12).contains(&type_len) {
        return (None, subject);
    }
    let mut i = type_len;
    if bytes.get(i) == Some(&b'(') {
        match bytes[i..].iter().position(|b| *b == b')') {
            // An empty scope, `feat(): x`, is malformed; treat the line as prose.
            Some(0 | 1) => return (None, subject),
            Some(close) => i += close + 1,
            None => return (None, subject),
        }
    }
    let breaking = bytes.get(i) == Some(&b'!');
    if breaking {
        i += 1;
    }
    // The space matters: `fix:it` is not a conventional commit, and without it
    // a URL-bearing subject would be cut at `https:`.
    if bytes.get(i) != Some(&b':') || bytes.get(i + 1) != Some(&b' ') {
        return (None, subject);
    }
    let rest = subject[i + 2..].trim_start();
    if rest.is_empty() {
        return (None, subject);
    }
    (Some((&subject[..type_len], breaking)), rest)
}

impl ScmPanelView {
    /// The history section, when it is expanded and has something to draw.
    pub fn render_graph_section(
        &mut self,
        repo: &RepoKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.graph.expanded {
            return Some(
                div()
                    .flex_none()
                    .py(px(GRAPH_PAD_TOP))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(self.graph_header(repo, None, GRAPH_HEADER_FOLDED_H, cx))
                    .into_any_element(),
            );
        }
        self.scm_load_graph(repo, cx);

        if self.graph.height.get() <= 0. {
            self.graph.height.set(GRAPH_H_DEFAULT);
        }
        let ceiling = (window.viewport_size().height.as_f32() * GRAPH_H_MAX_RATIO).max(GRAPH_H_MIN);
        let height = self.graph.height.get().clamp(GRAPH_H_MIN, ceiling);

        let page = self.graph.page.clone();
        let header = self.graph_header(repo, page.as_deref(), GRAPH_HEADER_H, cx);
        let search = self.graph_search(cx);
        let naming = self.graph_naming_row(repo, cx);
        let query = self.graph_query(cx);
        let body = match page {
            None => panel_empty_view(t(L10nKey::PanelLoading), None, cx),
            Some(page) if page.commits.is_empty() => {
                panel_empty_view(t(L10nKey::ScmGraphEmpty), None, cx)
            }
            Some(page) => self.graph_body(repo, &page, query.as_deref(), height, cx),
        };
        let (backing, handle) = self.graph_resize(ceiling, cx);

        Some(
            v_flex()
                .relative()
                .flex_none()
                .h(px(height))
                .pt(px(GRAPH_PAD_TOP))
                .pb(px(GRAPH_PAD_BOTTOM))
                .border_t_1()
                .border_color(cx.theme().border)
                .child(backing)
                .child(header)
                .children(search)
                .children(naming)
                .child(body)
                .child(handle)
                .into_any_element(),
        )
    }

    fn graph_scope(&self) -> GraphScope {
        self.graph.scope.clone()
    }

    fn scm_load_graph(&mut self, repo: &RepoKey, cx: &mut Context<Self>) {
        if self
            .graph
            .page_key
            .as_ref()
            .is_some_and(|(r, _, _)| r != repo)
        {
            self.graph.page = None;
            self.graph.page_key = None;
            self.graph.requested = 0;
        }
        let epoch = cx
            .try_global::<anterm_ui_kit::git::data::ScmData>()
            .map_or(0, |data| data.epoch(repo.host, &repo.root));
        let scope = self.graph_scope();
        let want = self.graph.requested.clamp(GRAPH_PAGE, MAX_GRAPH_COMMITS);
        let key = (repo.clone(), epoch, scope.clone());
        let fresh = self.graph.page_key.as_ref() == Some(&key)
            && self.graph.page.as_ref().is_some_and(|p| p.requested >= want);
        if fresh || self.graph.loading || self.graph.failed_key.as_ref() == Some(&key) {
            return;
        }
        let Some(host) = anterm_ui_kit::host_registry::HostRegistry::get(cx, repo.host) else {
            return;
        };
        self.graph.loading = true;
        let root = repo.root.clone();
        let query = scope.clone();
        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            move |h| anterm_core::git::log::load_page(h, &root, &query, want),
            move |this, page, cx| {
                this.graph.loading = false;
                match page {
                    Some(page) => {
                        this.graph.failed_key = None;
                        this.graph.page = Some(Arc::new(page));
                        this.graph.page_key = Some(key);
                    }
                    None => this.graph.failed_key = Some(key),
                }
                cx.notify();
            },
        );
    }

    fn graph_visible_rows(
        &mut self,
        page: &Arc<CommitPage>,
        query: Option<&str>,
    ) -> Arc<Vec<usize>> {
        if let Some((held_query, held_page, rows)) = &self.graph.filter_cache {
            if held_page.ptr_eq(&Arc::downgrade(page)) && held_query.as_deref() == query {
                return rows.clone();
            }
        }
        let rows: Arc<Vec<usize>> = Arc::new(match query {
            None => (0..page.commits.len()).collect(),
            Some(q) => (0..page.commits.len())
                .filter(|i| matches_query(&page.commits[*i], q))
                .collect(),
        });
        self.graph.filter_cache = Some((
            query.map(str::to_string),
            Arc::downgrade(page),
            rows.clone(),
        ));
        rows
    }

    fn graph_query(&self, cx: &gpui::App) -> Option<String> {
        let input = self.graph.search.as_ref()?;
        let text = input.read(cx).value().trim().to_lowercase();
        (!text.is_empty()).then_some(text)
    }

    fn graph_search(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.graph.search.clone()?;
        Some(panel_search_box(&input, None, cx))
    }

    fn graph_toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.graph.search.take().is_some() {
            self.graph.search_sub = None;
            cx.notify();
            return;
        }
        let input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(t(L10nKey::ScmGraphFilterPlaceholder))
        });
        self.graph.search_sub =
            Some(
                cx.subscribe_in(&input, window, |_this, _input, ev, _window, cx| {
                    if matches!(ev, gpui_kit::component::input::InputEvent::Change) {
                        cx.notify();
                    }
                }),
            );
        let handle = input.read(cx).focus_handle(cx);
        self.graph.search = Some(input);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn graph_body(
        &mut self,
        repo: &RepoKey,
        page: &Arc<CommitPage>,
        query: Option<&str>,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel_w = if self.ctx.panel_width > 0. {
            self.ctx.panel_width
        } else {
            cx.global::<anterm_ui_kit::config::Config>().right_panel_width
        };
        let cap = max_lanes(panel_w);
        let gutter = match page.max_lanes <= 1 {
            true => GRAPH_LINEAR_GUTTER,
            false => gutter_width(cap),
        };
        let now = anterm_ui_kit::intent::now_secs() as i64;

        let filtering = query.is_some();
        let rows = self.graph_visible_rows(page, query);
        let more = query.is_none() && !page.complete && page.commits.len() < MAX_GRAPH_COMMITS;
        let bands = rows.len() + usize::from(more);

        let scrolled = (-self.graph.scroll.offset().y.as_f32()).max(0.);
        let first = ((scrolled / GRAPH_ROW_H) as usize)
            .saturating_sub(GRAPH_WINDOW_MARGIN)
            .min(rows.len());
        let visible = (height / GRAPH_ROW_H).ceil() as usize + GRAPH_WINDOW_MARGIN * 2;
        let last = first.saturating_add(visible).min(rows.len());
        #[cfg(test)]
        row_probe::record(last.saturating_sub(first));

        let indent = if filtering { ROW_INSET } else { gutter };
        let list = v_flex().pt(px(first as f32 * GRAPH_ROW_H)).children(
            rows[first..last]
                .iter()
                .map(|i| self.graph_row(repo, page, *i, indent, now, cx)),
        );
        let mut stack = div()
            .relative()
            .w_full()
            .h(px(bands as f32 * GRAPH_ROW_H))
            .child(list)
            .children(more.then(|| self.graph_load_more(gutter, cx)));

        if !filtering {
            let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
            let paint = GraphPaint {
                page: page.clone(),
                max_lanes: cap,
                overflowing: page.max_lanes as usize > cap,
                lanes: cx.global::<ActiveLanes>().0,
                surface: gpui::rgb(sf.base).into(),
                selected: self
                    .graph
                    .selected
                    .as_deref()
                    .and_then(|oid| page.commits.iter().position(|c| c.oid == oid)),
                selected_surface: gpui::rgb(sf.selected).into(),
                more,
                linear: (page.max_lanes <= 1).then(|| Neutral {
                    line: cx.theme().border,
                    ring: cx.theme().muted_foreground,
                    head: cx.theme().foreground,
                }),
            };
            stack = stack.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| paint_graph(&paint, bounds, window),
                )
                .absolute()
                .top_0()
                .left_0()
                .w(px(gutter))
                .h_full(),
            );
        }

        let scroller = div()
            .id("scm-graph")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.graph.scroll)
            .child(div().px(px(GRAPH_SIDE)).child(stack));
        anterm_ui_kit::scrollbar::with_vertical_scrollbar(
            "scm-graph-scrollbar",
            scroller,
            &self.graph.scroll,
        )
    }

    fn graph_row(
        &self,
        repo: &RepoKey,
        page: &Arc<CommitPage>,
        i: usize,
        gutter: f32,
        now: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let commit = &page.commits[i];
        let mono = cx.theme().mono_font_family.clone();
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let selected = self.graph.selected.as_deref() == Some(commit.oid.as_str());
        let (prefix, subject) = split_conventional(&commit.summary);
        let deco = commit.refs.first();
        let extra = commit.refs.len().saturating_sub(1);
        let oid = commit.oid.clone();

        h_flex()
            .id(SharedString::from(format!("scm-graph-row-{}", commit.oid)))
            .items_center()
            .gap(px(GRAPH_ROW_GAP))
            .h(px(GRAPH_ROW_H))
            .pl(px(gutter))
            .pr(px(GRAPH_ROW_PAD_R))
            .rounded(px(6.))
            .cursor_pointer()
            .when(selected, |d| d.bg(gpui::rgb(sf.selected)))
            .when(!selected, |d| d.hover(|s| s.bg(gpui::rgb(sf.hover))))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(4.))
                    .children(
                        prefix.map(|(kind, breaking)| self.graph_type_prefix(kind, breaking, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(rems(TEXT))
                            .text_color(cx.theme().foreground)
                            .child(SharedString::from(subject.to_string())),
                    ),
            )
            .children(deco.map(|r| self.graph_ref_chip(r, extra, &mono, cx)))
            .child(
                div()
                    .flex_none()
                    .min_w(px(GRAPH_AGE_W))
                    .text_right()
                    .text_size(rems(META_MONO))
                    .font_family(mono.clone())
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(relative_time(
                        now,
                        commit.author.at.unix,
                    ))),
            )
            .tooltip({
                let text = commit_tooltip(commit, now);
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                }
            })
            .on_click(cx.listener({
                let repo = repo.clone();
                let page = page.clone();
                move |this, _, _, cx| {
                    let seed = page.commits[i].clone();
                    this.graph_open_commit(repo.clone(), seed.oid.clone(), Some(seed), cx)
                }
            }))
            .context_menu({
                let view = cx.entity().downgrade();
                let repo = repo.clone();
                let oid = oid.clone();
                move |menu, _window, cx| {
                    let danger = cx.theme().danger;
                    ScmPanelView::graph_row_context_menu(menu, &repo, &oid, danger, &view)
                }
            })
            .into_any_element()
    }

    pub(super) fn graph_open_commit(
        &mut self,
        repo: RepoKey,
        oid: String,
        seed: Option<Commit>,
        cx: &mut Context<Self>,
    ) {
        self.graph.selected = Some(oid.clone());
        self.open_commit_detail(repo, oid, seed, cx);
    }
}

/// One "load more" click's worth of growth, stopped at what `load_page` will
/// actually answer.
///
/// Growing past [`MAX_GRAPH_COMMITS`] would ask for a page the loader clamps:
/// the answer would never satisfy `requested >= want`, and the panel would
/// refetch the same full page from every frame's render, forever.
fn next_page_request(requested: usize) -> usize {
    requested
        .max(GRAPH_PAGE)
        .saturating_add(GRAPH_PAGE)
        .min(MAX_GRAPH_COMMITS)
}

/// Whether a commit answers the filter box.
///
/// Subject, author and sha, all case-folded. Not the body: a search that
/// matches on text the row cannot show is a search whose results look wrong.
fn matches_query(commit: &Commit, query: &str) -> bool {
    commit.summary.to_lowercase().contains(query)
        || commit.author.name.to_lowercase().contains(query)
        || commit.oid.starts_with(query)
}

fn commit_tooltip(commit: &Commit, now: i64) -> SharedString {
    SharedString::from(format!(
        "{}\n{} · {} · {}",
        commit.summary,
        commit.short(),
        commit.author.name,
        relative_time(now, commit.author.at.unix)
    ))
}

/// What ink a conventional-commit prefix is set in.
///
/// One rule rather than a palette, and the type is not what decides it. The
/// lane gutter immediately to the left is already a column of colour, so a
/// green `feat` beside a green lane dot adds a second colour column that says
/// nothing the dot has not: the reader's eye is pulled twice and told the same
/// thing once. Muting the whole vocabulary — `feat`, `fix`, `perf`, `docs` and
/// whatever a repository invents — leaves the row with two levels of text
/// emphasis, which is all a sidebar has ever needed.
///
/// The one exception is a breaking change. A `!` is the single thing in a
/// subject line worth shouting about, whatever the type in front of it says.
fn type_tone(breaking: bool, cx: &gpui::App) -> Hsla {
    let theme = cx.theme();
    match breaking {
        true => theme.danger,
        false => theme.muted_foreground,
    }
}

impl ScmPanelView {
    /// The prefix, set inline as the first word of the subject.
    ///
    /// Deliberately not a chip. A filled pill on every row is a colour column
    /// running down the panel right beside the one the lanes already draw, and
    /// because the types are different lengths — `fix`, `chore`, `refactor` —
    /// the pills gave every subject a different left edge, so nothing in the
    /// list lined up vertically. With no fill and no padding of its own, the
    /// two halves read as one sentence and the column starts where the prefix
    /// does.
    ///
    /// Level with the subject at 12px and told apart by tone alone. The two
    /// halves are one sentence, and a size change mid-sentence is a seam: the
    /// muting already says which half a reader is meant to skip, and saying it
    /// twice buys nothing but a ragged line.
    ///
    /// Not mono either, for the same reason it is not a chip: the subject
    /// beside it is not, and a family change mid-run is a seam where the point
    /// is continuity.
    fn graph_type_prefix(&self, kind: &str, breaking: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            // Never the part that truncates. It is a handful of characters, and
            // clipping them to buy the subject the same handful is not a trade
            // — a half-eaten `refac…` costs a reader more than it gives back.
            .flex_none()
            .text_size(rems(TEXT))
            .text_color(type_tone(breaking, cx))
            .child(SharedString::from(match breaking {
                true => format!("{kind}!"),
                false => kind.to_string(),
            }))
            .into_any_element()
    }

    /// The highest-priority ref on a commit, plus a count of the rest.
    ///
    /// `load_page` already sorted them HEAD → local → tag → remote, so the
    /// first one is the one worth the width.
    fn graph_ref_chip(
        &self,
        deco: &RefDeco,
        extra: usize,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (bg, fg, weight) = match deco.kind {
            // Where you are is the one thing on this row worth a heavier
            // weight; everything else is context. The fill is grey, not brand:
            // `theme.accent` is a neutral surface tint here, and the only
            // saturated colour the section spends is a lane's.
            //
            // A faint neutral pill in muted ink: the filled bead in the gutter
            // already says "you are here", so the chip only has to name it.
            RefKind::Head => (
                theme.foreground.opacity(0.06),
                theme.muted_foreground,
                gpui::FontWeight::NORMAL,
            ),
            // Tags are yellow because tags are yellow — in git's own output,
            // in every other client, and in the reader's memory.
            RefKind::Tag => (
                theme.warning.opacity(0.16),
                theme.warning,
                gpui::FontWeight::NORMAL,
            ),
            // Everything else — a local branch you are not on, a remote
            // tracking ref — is context, and context does not get a box. A
            // filled grey pill here was a third block of surface on a row that
            // already carries a lane gutter and a subject; unfilled, it reads
            // as a label sitting after the message, which is what it is.
            _ => (
                gpui::transparent_black(),
                theme.muted_foreground,
                gpui::FontWeight::NORMAL,
            ),
        };
        let label = match extra {
            0 => elide_middle(&deco.short, GRAPH_REF_CHARS).into_owned(),
            n => format!("{} +{n}", elide_middle(&deco.short, GRAPH_REF_CHARS)),
        };
        // A hard cap, because the chip is the one part of the row whose width
        // comes from a branch name someone else chose. `GRAPH_REF_CHARS` elides
        // the middle first; this is the backstop for the widths that survive
        // it — fourteen characters of `info_chip`'s 10.5px mono plus its
        // padding, which is about 72.
        div()
            .flex_none()
            .max_w(px(72.))
            .truncate()
            .font_weight(weight)
            .child(info_chip(&label, bg, fg, mono))
            .into_any_element()
    }

    /// The band under the last row that asks for the next page.
    ///
    /// A row rather than a scroll trigger. A remote `git log` is an RPC across
    /// a host boundary, and scroll-to-load turns one flick of a trackpad into a
    /// burst of concurrent ones.
    fn graph_load_more(&self, gutter: f32, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.graph.loading;
        h_flex()
            .id("scm-graph-more")
            .items_center()
            .h(px(GRAPH_ROW_H))
            .pl(px(gutter))
            .pr(px(GRAPH_ROW_PAD_R))
            .cursor_pointer()
            .text_size(rems(META))
            .text_color(cx.theme().muted_foreground)
            .hover(|s| s.text_color(cx.theme().foreground))
            .child(SharedString::from(match loading {
                true => t(L10nKey::PanelLoading),
                false => t(L10nKey::ScmGraphLoadMore),
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                this.graph.requested = next_page_request(this.graph.requested);
                cx.notify();
            }))
            .into_any_element()
    }

    fn graph_header(
        &self,
        repo: &RepoKey,
        page: Option<&CommitPage>,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.graph.expanded;
        let muted = cx.theme().muted_foreground;
        let filtering = self.graph.search.is_some();
        let count = page.map(|p| match p.complete {
            true => p.commits.len().to_string(),
            false => format!("{}+", p.commits.len()),
        });

        h_flex()
            .flex_none()
            .items_center()
            .gap(px(GRAPH_HEADER_GAP))
            .h(px(height))
            .pl(px(TEXT_INSET))
            .pr(px(TEXT_INSET - GRAPH_SCOPE_PAD))
            .child(
                h_flex()
                    .id("scm-graph-fold")
                    .items_center()
                    .gap(px(GRAPH_HEADER_GAP))
                    .flex_1()
                    .min_w(px(0.))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_none()
                            .w(px(GROUP_CHEVRON))
                            .flex()
                            .justify_center()
                            .text_color(muted)
                            .child(
                                Icon::new(match expanded {
                                    true => IconName::ChevronDown,
                                    false => IconName::ChevronRight,
                                })
                                .size(px(GROUP_CHEVRON)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(rems(META))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(SharedString::from(t(L10nKey::ScmGraphTitle))),
                    )
                    .children(count.map(|c| {
                        div()
                            .text_size(rems(META_MONO))
                            .text_color(muted)
                            .child(SharedString::from(c))
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_graph(cx))),
            )
            .when(expanded, |row| {
                row.child(
                    anterm_ui_kit::window_chrome::chrome_tile_sized(
                        Button::new("scm-graph-filter").icon(Icon::new(IconName::Search)),
                        GRAPH_TILE,
                        GRAPH_TILE_GLYPH,
                        filtering,
                        cx,
                    )
                    .rounded(px(4.))
                    .tooltip(t(L10nKey::ScmGraphFilterPlaceholder))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.graph_toggle_search(window, cx)),
                    ),
                )
                .child(
                    Button::new("scm-graph-scope")
                        .ghost()
                        .xsmall()
                        .h(px(GRAPH_TILE))
                        .rounded(px(4.))
                        .dropdown_caret(true)
                        .label(scope_label(&self.graph.scope))
                        .text_color(muted)
                        .dropdown_menu_with_anchor(
                            gpui::Anchor::TopRight,
                            self.graph_scope_menu(repo, cx),
                        ),
                )
            })
            .into_any_element()
    }

    fn graph_scope_menu(
        &self,
        repo: &RepoKey,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static + use<>
    {
        let view = cx.entity().downgrade();
        let branches = self
            .branches
            .get(repo)
            .map(|(_, names)| names.clone())
            .unwrap_or_default();
        let scope = self.graph.scope.clone();

        move |menu, _window, _cx| {
            let mut menu = menu.min_w(px(180.));
            let pick = |view: &gpui::WeakEntity<ScmPanelView>, next: GraphScope| {
                let view = view.clone();
                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                    let _ = view.update(cx, |this, cx| this.graph_set_scope(next.clone(), cx));
                }
            };
            for (label, next) in [
                (
                    t(L10nKey::ScmGraphCurrentBranch),
                    GraphScope::HeadAndUpstream,
                ),
                (t(L10nKey::ScmGraphAllBranches), GraphScope::All),
            ] {
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .checked(scope == next)
                        .on_click(pick(&view, next)),
                );
            }
            if !branches.is_empty() {
                menu = menu.separator();
            }
            for name in &branches {
                let next = GraphScope::Refs(vec![format!("refs/heads/{name}")]);
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(scope == next)
                        .on_click(pick(&view, next)),
                );
            }
            menu
        }
    }

    pub(super) fn graph_set_scope(&mut self, scope: GraphScope, cx: &mut Context<Self>) {
        if self.graph.scope == scope {
            return;
        }
        self.graph.scope = scope;
        self.graph.requested = GRAPH_PAGE;
        self.graph.page = None;
        self.graph.page_key = None;
        cx.notify();
    }
}

fn scope_label(scope: &GraphScope) -> String {
    match scope {
        GraphScope::All => t(L10nKey::ScmGraphAllBranches).to_string(),
        GraphScope::Refs(refs) => refs
            .first()
            .map(|r| r.rsplit('/').next().unwrap_or(r).to_string())
            .unwrap_or_else(|| t(L10nKey::ScmGraphAllBranches).to_string()),
        _ => t(L10nKey::ScmGraphCurrentBranch).to_string(),
    }
}

impl ScmPanelView {
    fn graph_row_context_menu(
        menu: PopupMenu,
        repo: &RepoKey,
        oid: &str,
        danger: Hsla,
        view: &gpui::WeakEntity<ScmPanelView>,
    ) -> PopupMenu {
        let op =
            |view: &gpui::WeakEntity<ScmPanelView>, repo: &RepoKey, build: fn(String) -> GitOp| {
                let view = view.clone();
                let repo = repo.clone();
                let rev = oid.to_string();
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                    let _ = view.update(cx, |this, cx| {
                        this.scm_op(repo.clone(), build(rev.clone()), window, cx)
                    });
                }
            };

        let mut menu = menu
            .min_w(px(200.))
            .item(
                PopupMenuItem::new(t(L10nKey::ScmCheckoutCommit))
                    .on_click(op(view, repo, |rev| GitOp::CheckoutDetached { rev })),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::ScmCreateBranchHere)).on_click({
                    let view = view.clone();
                    let repo = repo.clone();
                    let rev = oid.to_string();
                    move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.graph_begin_branch_at(repo.clone(), rev.clone(), window, cx)
                        });
                    }
                }),
            )
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::ScmCherryPick)).on_click(op(view, repo, |rev| {
                    GitOp::CherryPick {
                        rev,
                        mainline: true,
                        no_commit: false,
                    }
                })),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::ScmRevertCommit)).on_click(op(view, repo, |rev| {
                    GitOp::Revert {
                        rev,
                        mainline: true,
                    }
                })),
            )
            .separator();

        for (label, mode) in [
            (t(L10nKey::ScmResetSoft), ResetMode::Soft),
            (t(L10nKey::ScmResetMixed), ResetMode::Mixed),
            (t(L10nKey::ScmResetHard), ResetMode::Hard),
        ] {
            let view = view.clone();
            let repo = repo.clone();
            let rev = oid.to_string();
            let base = match mode {
                ResetMode::Hard => PopupMenuItem::element(move |_window, _cx| {
                    div().text_color(danger).child(label)
                }),
                _ => PopupMenuItem::new(label),
            };
            let item = base.on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.scm_op(
                        repo.clone(),
                        GitOp::Reset {
                            rev: rev.clone(),
                            mode,
                        },
                        window,
                        cx,
                    )
                });
            });
            menu = menu.item(item);
        }

        menu.separator()
            .item(PopupMenuItem::new(t(L10nKey::ScmCopyCommitSha)).on_click({
                let rev = oid.to_string();
                move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(rev.clone()))
            }))
    }

    fn graph_resize(&self, ceiling: f32, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        let container: Rc<StdCell<Option<Bounds<Pixels>>>> = Rc::new(StdCell::new(None));
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| container.set(Some(bounds))
            },
            {
                let container = container.clone();
                let height = self.graph.height.clone();
                let dragging = self.graph.dragging.clone();
                move |_bounds, _state, window: &mut Window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let height = height.clone();
                        let dragging = dragging.clone();
                        move |ev: &MouseMoveEvent, _phase, window: &mut Window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            let Some(b) = container.get() else {
                                return;
                            };
                            let raw = (b.origin.y + b.size.height - ev.position.y).as_f32();
                            height.set(raw.clamp(GRAPH_H_MIN, ceiling));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let dragging = dragging.clone();
                        move |_ev: &MouseUpEvent, _phase, window: &mut Window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            dragging.set(false);
                            window.refresh();
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element();

        let active = self.graph.dragging.get();
        let handle = div()
            .group("scm-graph-resize")
            .occlude()
            .absolute()
            .left_0()
            .top(px(-(GRAPH_HANDLE_H / 2.)))
            .w_full()
            .h(px(GRAPH_HANDLE_H))
            .flex()
            .items_center()
            .cursor_row_resize()
            .child(
                div()
                    .w_full()
                    .h(px(1.))
                    .when(active, |d| d.bg(cx.theme().drag_border))
                    .group_hover("scm-graph-resize", |s| s.bg(cx.theme().drag_border)),
            )
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.graph.dragging.clone();
                move |_ev, window: &mut Window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            })
            .into_any_element();

        (backing, handle)
    }

    fn graph_begin_branch_at(
        &mut self,
        repo: RepoKey,
        rev: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(t(L10nKey::ScmCreateBranchHere))
        });
        let handle = input.read(cx).focus_handle(cx);
        self.graph.naming = Some((input, rev));
        self.repo_override = Some(repo);
        self.override_tab = Some(self.ctx.active_tab);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn graph_naming_row(&mut self, repo: &RepoKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (input, rev) = self.graph.naming.clone()?;
        let repo = repo.clone();
        Some(
            h_flex()
                .id("scm-graph-naming")
                .flex_none()
                .items_center()
                .h(px(30.))
                .px(px(CONTENT_INSET))
                .child(
                    div().flex_1().min_w(px(0.)).child(
                        gpui_kit::component::input::Input::new(&input)
                            .appearance(false)
                            .xsmall(),
                    ),
                )
                .on_key_down(
                    cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "escape" => {
                                this.graph.naming = None;
                                cx.notify();
                            }
                            "enter" => {
                                let Some((input, _)) = this.graph.naming.take() else {
                                    return;
                                };
                                let name = input.read(cx).value().trim().to_string();
                                cx.notify();
                                if name.is_empty() {
                                    return;
                                }
                                this.scm_op(
                                    repo.clone(),
                                    GitOp::CreateBranch {
                                        name,
                                        start: Some(rev.clone()),
                                        checkout: false,
                                    },
                                    window,
                                    cx,
                                );
                            }
                            _ => {}
                        }
                    }),
                )
                .into_any_element(),
        )
    }
}
