use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Background, Context, EventEmitter, FocusHandle, Focusable, FontWeight, Global,
    Hsla, KeyDownEvent, MouseButton, SharedString, WeakEntity, Window, div, prelude::*, px, rems,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use anterm_ui_kit::config::{Config, DiffViewMode, DocumentLayout};
use anterm_ui_kit::git::diff::{self as git_diff, DiffSnapshot, DiffSource, FileDiff};
use anterm_ui_kit::window_chrome::{
    DocumentChrome, document_layout_switch_control, document_terminal_button, glyph_split,
    glyph_unified, switch_cell, switch_ink, switch_track,
};
use crate::diff_list::{DiffRow, RowAt};
use crate::right_panel::{META, META_MONO, TEXT, TEXT_MONO, info_chip};
use anterm_ui_kit::host_ops::{HostId, HostOps, SharedHost};
use anterm_ui_kit::window_chrome::TitleStripRow as _;
use anterm_editor::diff::{DiffSelection, Side};
use anterm_i18n::{L10nKey, t, t_plural};

mod body;
#[cfg(test)]
mod tests;

pub use body::{HAIRLINE_W, deco_status, hairline, tabular};
use body::{
    CHIP_FILL, CLOSE_TILE, CONTROL_H, DIFF_LINE_H, DiffBody, Drag, LIST_INSET, ROW_INSET,
    ROW_RADIUS, RowsFrom, RowsKey, diff_row_element, empty_snapshot, focused_file, focused_name,
    label_byline, now_unix, probe_key, resync_list, scm_epoch, source_subject, untracked_focus,
    view_mode,
};

/// How much of an untracked file the preview will read. Past this the card
/// says the read failed rather than showing a silently cut-off file — and the
/// line budget below cuts rendering long before this does anyway.
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

/// Outbound events emitted by [`DiffOverlayView`] so the diff surface owns its
/// probes, rows, selection, and rendering without reaching into `AntermApp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffOverlayEvent {
    Close,
    OpenFile { path: PathBuf },
    SetDocumentLayout(DocumentLayout),
    SetDiffViewMode(DiffViewMode),
}

/// Workspace chrome snapshot supplied by the document column right before
/// rendering [`DiffOverlayView`] or its hoisted header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHeaderContext {
    pub chrome: DocumentChrome,
    pub left_panel_open: bool,
    pub terminal_label: String,
}

impl Default for DiffHeaderContext {
    fn default() -> Self {
        Self {
            chrome: DocumentChrome::Fill,
            left_panel_open: false,
            terminal_label: String::new(),
        }
    }
}

pub enum DiffLoad {
    Loading,
    Ready(Arc<DiffSnapshot>),
    NotARepo,
}

/// Process-wide probe deduplication and fan-out registry for
/// [`DiffOverlayView`], reclaimed from `AntermApp`.
#[derive(Default)]
pub struct DiffProbeTracker {
    inflight: HashSet<(HostId, PathBuf)>,
    restale: HashSet<(HostId, PathBuf)>,
    listeners: Vec<WeakEntity<DiffOverlayView>>,
}

impl Global for DiffProbeTracker {}

impl DiffProbeTracker {
    fn register(view: WeakEntity<DiffOverlayView>, cx: &mut App) {
        let tracker = cx.default_global::<Self>();
        tracker.listeners.retain(|w| w.upgrade().is_some());
        if !tracker
            .listeners
            .iter()
            .any(|w| w.entity_id() == view.entity_id())
        {
            tracker.listeners.push(view);
        }
    }

    pub fn spawn_probe(host: SharedHost, cwd: PathBuf, source: DiffSource, cx: &mut App) {
        let key = probe_key(host.id(), &cwd, &source);
        let already_inflight = {
            let tracker = cx.default_global::<Self>();
            if !tracker.inflight.insert(key.clone()) {
                tracker.restale.insert(key.clone());
                true
            } else {
                false
            }
        };
        if already_inflight {
            return;
        }
        let host_for_retry = host.clone();
        let probe_cwd = cwd.clone();
        let probe_source = source.clone();
        HostOps::run_on_app(
            host,
            cx,
            move |h| {
                let req = git_diff::DiffRequest {
                    source: probe_source,
                    ..Default::default()
                };
                git_diff::probe_diff(h, &probe_cwd, &req)
            },
            move |cx, result| {
                Self::land(
                    host_for_retry,
                    key,
                    cwd,
                    source,
                    result.map(Arc::new),
                    cx,
                );
            },
        );
    }

    fn land(
        host_for_retry: SharedHost,
        key: (HostId, PathBuf),
        cwd: PathBuf,
        source: DiffSource,
        snap: Option<Arc<DiffSnapshot>>,
        cx: &mut App,
    ) {
        let host_id = key.0;
        let (retry, listeners) = {
            let tracker = cx.default_global::<Self>();
            tracker.inflight.remove(&key);
            let retry = tracker.restale.remove(&key);
            tracker.listeners.retain(|w| w.upgrade().is_some());
            (retry, tracker.listeners.clone())
        };

        if let Some(snap) = snap.as_ref().filter(|s| !s.read_failed) {
            let counts = matches!(snap.source, DiffSource::Head).then(|| snap.totals());
            let root = snap.root.clone();
            let branch = snap.branch.clone();
            cx.default_global::<anterm_ui_kit::git::status::GitStatusCache>();
            cx.update_global::<anterm_ui_kit::git::status::GitStatusCache, _>(|cache, _| {
                cache.note_diff_read(host_id, &root, &branch, counts);
            });
        }

        let landing_epoch = snap.as_ref().map(|s| scm_epoch(cx, host_id, &s.root));
        for weak in listeners {
            let _ = weak.update(cx, |overlay, cx| {
                overlay.apply_snapshot(host_id, &cwd, &source, snap.clone(), landing_epoch, cx);
            });
        }

        if retry {
            Self::spawn_probe(host_for_retry, cwd, source, cx);
        }
    }
}

pub struct DiffOverlayView {
    pub host_id: HostId,
    pub cwd: PathBuf,
    pub source: DiffSource,
    pub focus_handle: FocusHandle,
    pub load: DiffLoad,
    pub loading: bool,
    pub expanded: HashMap<String, bool>,
    pub focus: Option<String>,
    pub preview: Option<(String, Option<Arc<FileDiff>>)>,
    pub preview_loading: Option<String>,
    pub selection: Option<DiffSelection>,
    pub selecting: bool,
    pub list: gpui::ListState,
    pub rows: Rc<Vec<DiffRow>>,
    rows_key: Option<RowsKey>,
    pub epoch: Option<u64>,
    pub helix_pending: Option<char>,
    header_ctx: DiffHeaderContext,
}

impl EventEmitter<DiffOverlayEvent> for DiffOverlayView {}

impl Focusable for DiffOverlayView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Paints the full-window diff surface without inheriting workspace opacity.
/// The preset's solid or gradient design remains intact, but neither it nor
/// the plain theme fallback may reveal the OS backdrop through diff text.
fn diff_overlay_background(
    active: Option<&anterm_ui_kit::presets::ActiveBackground>,
    fallback: Hsla,
) -> Background {
    match active {
        Some(bg) => anterm_ui_kit::theme::window_background_opaque(bg),
        None => fallback.alpha(1.0).into(),
    }
}

impl DiffOverlayView {
    pub fn new(
        host_id: HostId,
        cwd: PathBuf,
        source: DiffSource,
        focus: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            host_id,
            cwd,
            source,
            focus_handle: cx.focus_handle(),
            load: DiffLoad::Loading,
            loading: false,
            expanded: HashMap::new(),
            focus,
            preview: None,
            preview_loading: None,
            selection: None,
            selecting: false,
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(256.))
                .with_uniform_item_height(DIFF_LINE_H),
            rows: Rc::new(Vec::new()),
            rows_key: None,
            epoch: None,
            helix_pending: None,
            header_ctx: DiffHeaderContext::default(),
        };
        DiffProbeTracker::register(cx.weak_entity(), cx);
        view.spawn_probe(cx);
        view
    }

    pub fn matches_target(&self, host: HostId, cwd: &Path, source: &DiffSource) -> bool {
        self.host_id == host && self.cwd == cwd && self.source == *source
    }

    pub fn focus_path(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    pub fn focus_target(&self) -> Option<(HostId, PathBuf, DiffSource, String)> {
        Some((
            self.host_id,
            self.cwd.clone(),
            self.source.clone(),
            self.focus.clone()?,
        ))
    }

    /// Updates workspace chrome inputs before a render frame without calling
    /// `cx.notify()`.
    pub fn set_header_context(&mut self, ctx: DiffHeaderContext) {
        self.header_ctx = ctx;
    }

    pub fn set_focus(
        &mut self,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus = focus;
        self.selection = None;
        self.selecting = false;
        let handle = self.focus_handle.clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    pub fn set_expanded(&mut self, path: String, expanded: bool, cx: &mut Context<Self>) {
        self.expanded.insert(path, expanded);
        cx.notify();
    }

    pub fn spawn_probe(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let cwd = self.cwd.clone();
        let source = self.source.clone();
        let id = self.host_id;
        let epoch = match &self.load {
            DiffLoad::Ready(snap) => Some(scm_epoch(cx, id, &snap.root)),
            _ => None,
        };
        let Some(host) = anterm_ui_kit::host_registry::HostRegistry::lookup(cx, id) else {
            return;
        };
        self.loading = true;
        self.epoch = epoch;
        DiffProbeTracker::register(cx.weak_entity(), cx);
        DiffProbeTracker::spawn_probe(host, cwd, source, cx);
    }

    fn apply_snapshot(
        &mut self,
        host: HostId,
        cwd: &Path,
        source: &DiffSource,
        snap: Option<Arc<DiffSnapshot>>,
        landing_epoch: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        if !self.matches_target(host, cwd, source) {
            return;
        }
        self.loading = false;
        self.epoch = self.epoch.or(landing_epoch);
        self.load = match snap {
            Some(snap) => DiffLoad::Ready(snap),
            None => DiffLoad::NotARepo,
        };
        self.preview = None;
        self.selection = None;
        self.selecting = false;
        cx.notify();
    }

    pub fn maybe_refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let DiffLoad::Ready(snap) = &self.load else {
            return;
        };
        let stale = match self.source {
            DiffSource::Commit { .. } | DiffSource::Range { .. } => return,
            DiffSource::Head => {
                let Some(status) = cx
                    .try_global::<anterm_ui_kit::git::status::GitStatusCache>()
                    .and_then(|cache| cache.status_for(self.host_id, &self.cwd))
                else {
                    return;
                };
                status.branch != snap.branch || (status.added, status.removed) != snap.totals()
            }
            DiffSource::Worktree | DiffSource::Staged => {
                let Some(seen) = self.epoch else {
                    return;
                };
                scm_epoch(cx, self.host_id, &snap.root) != seen
            }
        };
        if stale {
            self.spawn_probe(cx);
        }
    }

    fn handle_helix_key(
        &mut self,
        ev: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        use gpui_kit::component::scroll::ScrollbarHandle as _;

        let key = ev.keystroke.key.as_str();
        let key_char = ev.keystroke.key_char.as_deref();
        let mods = &ev.keystroke.modifiers;
        if mods.platform || mods.alt {
            return false;
        }

        let scroll_by_px = |list: &gpui::ListState, delta_y: f32| {
            let cur = list.offset();
            let new_y = (cur.y - px(delta_y)).min(px(0.));
            list.set_offset(gpui::point(cur.x, new_y));
        };

        if let Some(pending) = self.helix_pending.take() {
            match (pending, key_char.unwrap_or(key)) {
                ('g', "g") => {
                    self.list.scroll_to_reveal_item(0);
                    cx.notify();
                    return true;
                }
                ('g', "e") => {
                    let len = self.rows.len();
                    if len > 0 {
                        self.list.scroll_to_reveal_item(len - 1);
                    }
                    cx.notify();
                    return true;
                }
                ('g', "n") | (']', "c") => {
                    self.helix_step_target(true, cx);
                    return true;
                }
                ('g', "p") | ('[', "c") => {
                    self.helix_step_target(false, cx);
                    return true;
                }
                ('g', "f") => {
                    self.helix_open_focused_file(cx);
                    return true;
                }
                _ => {
                    cx.notify();
                    return true;
                }
            }
        }

        if mods.control {
            match key {
                "d" => {
                    scroll_by_px(&self.list, f32::from(DIFF_LINE_H) * 14.0);
                    cx.notify();
                    return true;
                }
                "u" => {
                    scroll_by_px(&self.list, -f32::from(DIFF_LINE_H) * 14.0);
                    cx.notify();
                    return true;
                }
                _ => return false,
            }
        }

        match key_char.unwrap_or(key) {
            "q" => {
                cx.emit(DiffOverlayEvent::Close);
                true
            }
            "j" | "down" => {
                scroll_by_px(&self.list, f32::from(DIFF_LINE_H) * 3.0);
                cx.notify();
                true
            }
            "k" | "up" => {
                scroll_by_px(&self.list, -f32::from(DIFF_LINE_H) * 3.0);
                cx.notify();
                true
            }
            "pagedown" => {
                scroll_by_px(&self.list, f32::from(DIFF_LINE_H) * 14.0);
                cx.notify();
                true
            }
            "pageup" => {
                scroll_by_px(&self.list, -f32::from(DIFF_LINE_H) * 14.0);
                cx.notify();
                true
            }
            "G" => {
                let len = self.rows.len();
                if len > 0 {
                    self.list.scroll_to_reveal_item(len - 1);
                }
                cx.notify();
                true
            }
            "g" => {
                self.helix_pending = Some('g');
                cx.notify();
                true
            }
            "]" => {
                self.helix_pending = Some(']');
                cx.notify();
                true
            }
            "[" => {
                self.helix_pending = Some('[');
                cx.notify();
                true
            }
            "y" => {
                self.copy_selection(cx);
                true
            }
            "enter" => {
                self.helix_open_focused_file(cx);
                true
            }
            _ => false,
        }
    }

    fn helix_step_target(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(cur_focus) = self.focus.clone()
            && let DiffLoad::Ready(snap) = &self.load
            && !snap.files.is_empty()
            && let Some(idx) = snap.files.iter().position(|f| f.path == cur_focus)
        {
            let next_idx = if forward {
                (idx + 1) % snap.files.len()
            } else if idx == 0 {
                snap.files.len() - 1
            } else {
                idx - 1
            };
            self.focus = Some(snap.files[next_idx].path.clone());
            self.selection = None;
            self.selecting = false;
            cx.notify();
            return;
        }
        let top_ix = self.list.logical_scroll_top().item_ix;
        let targets: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(ix, row)| {
                matches!(row, DiffRow::FileHeader(_) | DiffRow::HunkHeader { .. }).then_some(ix)
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        let target_ix = if forward {
            targets
                .iter()
                .copied()
                .find(|&ix| ix > top_ix)
                .unwrap_or(targets[0])
        } else {
            targets
                .iter()
                .copied()
                .rev()
                .find(|&ix| ix < top_ix)
                .unwrap_or_else(|| targets.last().copied().unwrap_or(0))
        };
        self.list.scroll_to_reveal_item(target_ix);
        cx.notify();
    }

    fn helix_open_focused_file(&mut self, cx: &mut Context<Self>) {
        let top_ix = self.list.logical_scroll_top().item_ix;
        let row_path = self
            .rows
            .iter()
            .take(top_ix.saturating_add(1))
            .rev()
            .find_map(|row| match row {
                DiffRow::FileHeader(h) => Some(h.path.clone()),
                DiffRow::Split { at, .. } | DiffRow::Unified { at, .. } => {
                    Some(at.path.to_string())
                }
                DiffRow::Untracked { path, .. } => Some(path.clone()),
                _ => None,
            });
        let (root, rel_path) = match &self.load {
            DiffLoad::Ready(snap) => {
                let rel = self
                    .focus
                    .clone()
                    .or(row_path)
                    .or_else(|| snap.files.first().map(|f| f.path.clone()))
                    .or_else(|| snap.untracked.first().cloned());
                (snap.root.clone(), rel)
            }
            _ => (self.cwd.clone(), self.focus.clone().or(row_path)),
        };
        if let Some(rel) = rel_path {
            cx.emit(DiffOverlayEvent::OpenFile {
                path: root.join(rel),
            });
        }
    }

    pub(super) fn start_selection(
        &mut self,
        at: &RowAt,
        mode: DiffViewMode,
        side: Option<Side>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = Some(DiffSelection {
            path: at.path.to_string(),
            mode,
            side,
            anchor: at.id,
            head: at.id,
        });
        self.selecting = true;
        let handle = self.focus_handle.clone();
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(super) fn extend_selection(
        &mut self,
        at: &RowAt,
        side: Option<Side>,
        held: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        if held != Some(MouseButton::Left) {
            self.selecting = false;
            cx.notify();
            return;
        }
        let Some(sel) = self.selection.as_mut() else {
            return;
        };
        if sel.path != at.path.as_ref() || sel.side != side || sel.head == at.id {
            return;
        }
        sel.head = at.id;
        cx.notify();
    }

    pub(super) fn end_selection(&mut self, cx: &mut Context<Self>) {
        if self.selecting {
            self.selecting = false;
            cx.notify();
        }
    }

    pub(super) fn copy_selection(&self, cx: &mut Context<Self>) {
        let Some(sel) = self.selection.as_ref() else {
            return;
        };
        let hunks = match &self.load {
            DiffLoad::Ready(snap) => snap
                .files
                .iter()
                .find(|f| f.path == sel.path)
                .map(|f| f.hunks.as_slice()),
            _ => None,
        }
        .or_else(|| match &self.preview {
            Some((held, Some(file))) if *held == sel.path => Some(file.hunks.as_slice()),
            _ => None,
        });
        let Some(hunks) = hunks else {
            return;
        };
        let text = sel.text(hunks);
        if text.is_empty() {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    fn font_family(cx: &App) -> SharedString {
        cx.try_global::<Config>()
            .map(|c| SharedString::from(c.font_family.clone()))
            .unwrap_or_else(|| cx.theme().mono_font_family.clone())
    }

    /// Renders the diff header alone (used both inline in `Render` and hoisted
    /// into the spanning title bar on Windows/Linux).
    pub fn render_header(
        &self,
        ctx: &DiffHeaderContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chrome = ctx.chrome;
        let (branch, files, untracked, added, removed) = match &self.load {
            DiffLoad::Ready(s) => {
                let stats = s.stats();
                let (a, r) = stats.totals;
                (s.branch.clone(), s.files.len(), stats.untracked_count, a, r)
            }
            _ => (String::new(), 0, 0, 0, 0),
        };
        let lead = if ctx.left_panel_open || chrome.is_dock() {
            anterm_ui_kit::window_chrome::CONTENT_INSET
        } else {
            anterm_ui_kit::window_chrome::TITLE_BAR_LEAD
        };
        let mono = Self::font_family(cx);
        let subject = source_subject(&self.source, branch);
        let subject_takes_the_slack =
            chrome.is_dock() && !subject.is_rev && subject.label.is_none();
        let hover = gpui::rgb(cx.global::<anterm_ui_kit::presets::Surfaces>().window.hover);
        let row = h_flex().id("diff-overlay-header");
        let row = if chrome.header_is_title_strip() {
            anterm_ui_kit::window_chrome::title_bar_drag(row, "diff-overlay-header", window, cx)
        } else {
            row
        };
        let back_to_terminal = document_terminal_button(
            chrome,
            &ctx.terminal_label,
            cx.listener(|_, _, _, cx| {
                cx.emit(DiffOverlayEvent::SetDocumentLayout(DocumentLayout::Dock));
            }),
            window,
            cx,
        );
        let layout_switch = document_layout_switch_control(
            chrome,
            {
                let view = cx.entity().downgrade();
                move |layout, _window, cx| {
                    let _ = view.update(cx, |_, cx| {
                        cx.emit(DiffOverlayEvent::SetDocumentLayout(layout));
                    });
                }
            },
            cx,
        );

        row.flex_shrink_0()
            .title_strip_row(HAIRLINE_W, hairline(cx))
            .pl(px(lead))
            .pr(px(anterm_ui_kit::window_chrome::tile_trailing_inset()
                + (anterm_ui_kit::window_chrome::TILE_SIZE - CLOSE_TILE) / 2.))
            .gap_2()
            .items_center()
            .children(back_to_terminal)
            .child(
                gpui::svg()
                    .path(subject.icon)
                    .flex_shrink_0()
                    .size(px(13.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(if subject.is_rev {
                div()
                    .flex_shrink_0()
                    .text_size(rems(TEXT_MONO))
                    .font_weight(FontWeight::MEDIUM)
                    .font_family(mono.clone())
                    .child(subject.text)
                    .into_any_element()
            } else {
                div()
                    .when(subject_takes_the_slack, |d| d.flex_1().min_w_0().truncate())
                    .when(!subject_takes_the_slack, |d| d.flex_shrink_0())
                    .text_size(rems(TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .child(subject.text)
                    .into_any_element()
            })
            .when_some(subject.chip, |bar, text| {
                bar.child(info_chip(
                    text,
                    cx.theme().foreground.opacity(CHIP_FILL),
                    cx.theme().muted_foreground,
                    &mono,
                ))
            })
            .when_some(subject.label.as_ref(), |bar, label| {
                bar.child(
                    div()
                        .flex_auto()
                        .min_w_0()
                        .truncate()
                        .text_size(rems(TEXT))
                        .child(SharedString::from(label.subject.clone())),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_shrink(999.)
                        .truncate()
                        .text_size(rems(META))
                        .font_features(tabular())
                        .text_color(cx.theme().muted_foreground)
                        .child(label_byline(label, now_unix())),
                )
            })
            .when_some(focused_name(self), |bar, name| {
                let (head, leaf) = anterm_ui_kit::path_display::split_path_leaf(&name);
                bar.child(
                    div().occlude().min_w_0().flex_shrink(999.).child(
                        h_flex()
                            .id("diff-overlay-unfocus")
                            .items_center()
                            .min_w_0()
                            .gap_1()
                            .h(CONTROL_H)
                            .pl(px(4.))
                            .pr(ROW_INSET)
                            .rounded(ROW_RADIUS)
                            .cursor_pointer()
                            .hover(|s| s.bg(hover))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.focus = None;
                                cx.notify();
                            }))
                            .child(
                                Icon::new(IconName::ChevronLeft)
                                    .small()
                                    .flex_shrink_0()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .text_size(rems(META_MONO))
                                    .font_family(mono.clone())
                                    .child(div().min_w_0().flex_shrink(999.).truncate().child(head))
                                    .child(div().min_w_0().flex_shrink(1.).truncate().child(leaf)),
                            ),
                    ),
                )
            })
            .when(
                matches!(self.load, DiffLoad::Ready(_)) && self.focus.is_none(),
                |bar| {
                    let mut summary = t_plural(L10nKey::DiffChangedFiles, files, &[]);
                    if untracked > 0 {
                        summary.push_str(&t_plural(L10nKey::DiffUntrackedCount, untracked, &[]));
                    }
                    bar.when(!chrome.is_dock(), |bar| {
                        bar.child(
                            div()
                                .text_size(rems(META))
                                .font_features(tabular())
                                .text_color(cx.theme().muted_foreground)
                                .child(summary),
                        )
                    })
                    .when(added > 0, |bar| {
                        bar.child(
                            div()
                                .text_size(rems(META))
                                .font_features(tabular())
                                .text_color(cx.theme().success)
                                .child(format!("+{added}")),
                        )
                    })
                    .when(removed > 0, |bar| {
                        bar.child(
                            div()
                                .text_size(rems(META))
                                .font_features(tabular())
                                .text_color(cx.theme().danger)
                                .child(format!("−{removed}")),
                        )
                    })
                },
            )
            .when(
                self.loading && matches!(self.load, DiffLoad::Ready(_)),
                |bar| {
                    bar.child(
                        div()
                            .text_size(rems(META))
                            .text_color(cx.theme().muted_foreground)
                            .child(t(L10nKey::Refreshing)),
                    )
                },
            )
            .when(subject.label.is_none() && !subject_takes_the_slack, |bar| {
                bar.child(div().flex_1())
            })
            .child(
                div()
                    .occlude()
                    .flex_shrink_0()
                    .child(self.diff_view_switch(cx)),
            )
            .child(layout_switch)
            .child(
                div().occlude().flex_shrink_0().child(
                    anterm_ui_kit::window_chrome::chrome_tile_sized(
                        Button::new("diff-overlay-close").icon(Icon::new(IconName::Close)),
                        CLOSE_TILE,
                        anterm_ui_kit::window_chrome::RAIL_TILE_GLYPH,
                        false,
                        cx,
                    )
                    .rounded(px(anterm_ui_kit::window_chrome::RAIL_TILE_RADIUS))
                    .tooltip(t(L10nKey::DiffCloseTooltip))
                    .on_click(cx.listener(|_, _, _window, cx| {
                        cx.emit(DiffOverlayEvent::Close);
                    })),
                ),
            )
            .into_any_element()
    }

    fn diff_view_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = view_mode(cx);
        let cell =
            |i: usize, mode: DiffViewMode, glyph: fn(gpui::Hsla) -> AnyElement, tip: L10nKey| {
                let live = mode == current;
                let ink = switch_ink(live, cx);
                switch_cell(
                    ("diff-overlay-view-cell", i),
                    glyph(ink),
                    live,
                    tip,
                    cx.listener(move |_, _, _window, cx| {
                        cx.emit(DiffOverlayEvent::SetDiffViewMode(mode));
                    }),
                    cx,
                )
            };
        switch_track(
            "diff-overlay-view",
            vec![
                cell(0, DiffViewMode::Split, glyph_split, L10nKey::DiffViewSplit),
                cell(
                    1,
                    DiffViewMode::Unified,
                    glyph_unified,
                    L10nKey::DiffViewUnified,
                ),
            ],
            cx,
        )
    }

    fn spawn_untracked_preview_if_needed(&mut self, cx: &mut Context<Self>) {
        let want = match &self.load {
            DiffLoad::Ready(snap) => {
                untracked_focus(snap, self.focus.as_deref()).and_then(|path| {
                    let seen = self.preview.as_ref().is_some_and(|(held, _)| held == path)
                        || self.preview_loading.as_deref() == Some(path);
                    (!seen).then(|| (self.host_id, snap.root.clone(), path.to_string()))
                })
            }
            _ => None,
        };
        let Some((host_id, root, path)) = want else {
            return;
        };
        let Some(host) = anterm_ui_kit::host_registry::HostRegistry::lookup(cx, host_id) else {
            return;
        };
        self.preview_loading = Some(path.clone());
        let read_path = root.join(&path);
        let key_path = path.clone();
        HostOps::run(
            host,
            cx,
            move |h| {
                h.read_file(&read_path, MAX_PREVIEW_BYTES)
                    .ok()
                    .map(|bytes| {
                        Arc::new(git_diff::synthesize_added(
                            &path,
                            &bytes,
                            &git_diff::DiffBudget::SINGLE_FILE,
                        ))
                    })
            },
            move |this, file, cx| {
                if this.host_id != host_id {
                    return;
                }
                if this.preview_loading.as_deref() == Some(key_path.as_str()) {
                    this.preview_loading = None;
                }
                this.preview = Some((key_path, file));
                cx.notify();
            },
        );
    }

    fn diff_message(&self, text: &'static str, cx: &App) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(rems(TEXT))
            .text_color(cx.theme().muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn sync_rows(&mut self, cx: &App) -> DiffBody {
        let mode = view_mode(cx);
        let snap = match &self.load {
            DiffLoad::Loading => return DiffBody::Message(t(L10nKey::DiffReading)),
            DiffLoad::NotARepo => return DiffBody::Message(t(L10nKey::DiffNotARepo)),
            DiffLoad::Ready(snap) if empty_snapshot(snap) && snap.read_failed => {
                return DiffBody::Message(t(L10nKey::DiffReadFailed));
            }
            DiffLoad::Ready(snap) if empty_snapshot(snap) => {
                return DiffBody::Message(t(L10nKey::DiffWorkingTreeClean));
            }
            DiffLoad::Ready(snap) => Arc::clone(snap),
        };

        let preview = match untracked_focus(&snap, self.focus.as_deref()) {
            Some(path) => match &self.preview {
                Some((held, file)) if held == path => match file {
                    Some(file) => Some(Arc::clone(file)),
                    None => return DiffBody::Message(t(L10nKey::DiffReadFailed)),
                },
                _ => return DiffBody::Message(t(L10nKey::DiffReading)),
            },
            None => None,
        };

        let drawn = |path: &str| {
            preview
                .as_deref()
                .filter(|f| f.path == path)
                .or_else(|| snap.files.iter().find(|f| f.path == path))
                .map_or(mode, |f| crate::diff_list::file_mode(f, mode))
        };
        if self
            .selection
            .as_ref()
            .is_some_and(|s| s.mode != drawn(&s.path))
        {
            self.selection = None;
            self.selecting = false;
        }

        let focused = focused_file(&snap, self);
        let from = RowsFrom {
            snap: &snap,
            preview: preview.as_ref(),
            mode,
            focused,
            oversized: focused.is_none() && snap.stats().oversized,
            expanded: &self.expanded,
        };
        let stale = self
            .rows_key
            .as_ref()
            .is_none_or(|held| !held.describes(&from));
        if stale {
            let rows = match from.preview {
                Some(file) => crate::diff_list::preview_rows(file, from.mode),
                None => crate::diff_list::build_rows(
                    from.snap,
                    from.expanded,
                    from.focused,
                    from.mode,
                    from.oversized,
                ),
            };
            let key = from.to_key();
            resync_list(&self.list, &self.rows, &rows);
            self.rows = Rc::new(rows);
            self.rows_key = Some(key);
        } else if let Some(held) = self.rows_key.as_mut() {
            held.retarget(&from);
        }
        DiffBody::Rows(snap)
    }

    fn diff_rows_list(&self, snap: Arc<DiffSnapshot>, cx: &mut Context<Self>) -> AnyElement {
        let rows = Rc::clone(&self.rows);
        let font = Self::font_family(cx);
        let overlay = cx.entity().downgrade();
        let list = self.list.clone();
        let drag = Drag {
            sel: self.selection.clone().map(Rc::new),
            selecting: self.selecting,
        };
        let body = gpui::list(list.clone(), move |ix, _window, cx| {
            #[cfg(test)]
            row_probe::record();
            match rows.get(ix) {
                Some(row) => diff_row_element(row, ix, &drag, &font, &snap, &overlay, cx),
                None => div().into_any_element(),
            }
        })
        .size_full()
        .py(px(LIST_INSET));
        anterm_ui_kit::scrollbar::with_vertical_scrollbar("diff-overlay-scrollbar", body, &list)
    }
}

impl Render for DiffOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.spawn_untracked_preview_if_needed(cx);
        let body = self.sync_rows(cx);

        let content = match body {
            DiffBody::Message(text) => self.diff_message(text, cx),
            DiffBody::Rows(snap) => self.diff_rows_list(snap, cx),
        };

        let chrome = self.header_ctx.chrome;
        let header = chrome
            .renders_own_header()
            .then(|| self.render_header(&self.header_ctx.clone(), window, cx));
        let focus_handle = self.focus_handle.clone();

        let shell = v_flex();
        let shell = match chrome {
            DocumentChrome::Fill => shell
                .absolute()
                .inset_0()
                .occlude()
                .bg(diff_overlay_background(
                    cx.try_global::<anterm_ui_kit::presets::ActiveBackground>(),
                    cx.theme().background,
                ))
                .children(anterm_ui_kit::window_chrome::overlay_surface_layers(cx)),
            DocumentChrome::Dock | DocumentChrome::DockHoisted => shell.size_full().min_w_0(),
        };

        shell
            .text_color(cx.theme().foreground)
            .track_focus(&focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                if ev.keystroke.key.as_str() == "escape" {
                    if this.helix_pending.take().is_some() {
                        cx.notify();
                        return;
                    }
                    cx.emit(DiffOverlayEvent::Close);
                    return;
                }
                let mods = ev.keystroke.modifiers;
                if ev.keystroke.key.as_str() == "c"
                    && anterm_ui_kit::platform::is_secondary(mods)
                    && !mods.alt
                {
                    this.copy_selection(cx);
                    return;
                }
                if cx.global::<Config>().helix_mode && this.handle_helix_key(ev, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.end_selection(cx)),
            )
            .children(header)
            .child(content)
    }
}

/// Counts the rows the list actually built, so a test can tell that a patch of
/// any size costs the handful of rows on screen rather than all of them.
#[cfg(test)]
pub mod row_probe {
    use std::cell::Cell;

    thread_local! {
        static BUILT: Cell<u64> = const { Cell::new(0) };
    }

    pub fn record() {
        BUILT.set(BUILT.get() + 1);
    }

    /// The count since the last call, and zero from here.
    #[cfg(not(target_os = "windows"))]
    pub fn take() -> u64 {
        BUILT.replace(0)
    }
}
