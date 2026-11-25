//! Standalone right panel host entity (`RightPanelView`).
//!
//! Owns the Files / Source Control / Info / SFTP tab bar, panel width and
//! resize drag state, child panel entities (`FileTreeView`, `ScmPanelView`,
//! `SftpPanelView`, `PanelSearchView`, `ForwardsPanel`, `file_search`), and
//! the Info / Processes / Ports tab.

mod info;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use anterm_i18n::{L10nKey, t};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FontWeight, Subscription, Window, div, prelude::*,
    px, rems,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, InteractiveElementExt as _, h_flex, v_flex,
};

use anterm_ui_kit::config::{Config, RightPanelTab};
use anterm_ui_kit::config_store::ConfigStore;
use anterm_core::daemon::protocol::{ManagedForward, PaneProcs};
use anterm_ui_kit::git::diff::DiffSource;
use anterm_ui_kit::scrollbar::with_vertical_scrollbar;
use crate::file_tree::{FileTreeEvent, FileTreeView};
use crate::panel_search::PanelSearchView;
use crate::scm::{ScmEvent, ScmPanelView};
use anterm_remote_ui::forwards::{ForwardsEvent, ForwardsPanel};
use anterm_ui_kit::host_ops::{HostId, SharedHost};
use anterm_remote_ui::sftp::{SftpEvent, SftpPanelView};
use anterm_ui_kit::daemon_rpc::{ForwardOwnerKey, ForwardRoute};
use anterm_ui_kit::window_chrome::{
    CONTENT_INSET, TILE_SIZE_SM, TITLE_BAR_BORDER, TitleStripRow as _, tile_trailing_inset,
};
use anterm_ui_kit::intent::WorkspaceIntent;
use anterm_ui_kit::window_chrome::{
    RIGHT_PANEL_TABS, TAB_INNER_PAD, TAB_OUTER_PAD, chrome_tile,
};

pub use info::PaneForwardCtx;

/// The tab a stored choice shows. Search was a tab of its own until the
/// Files tab took over searching file contents; a config that still says
/// `"search"` opens on Files.
pub fn shown_tab(tab: RightPanelTab) -> RightPanelTab {
    match tab {
        RightPanelTab::Search => RightPanelTab::Files,
        tab => tab,
    }
}

/// Wide enough for the word tabs, bare, beside the panel toggle at
/// the default interface size — the Changes count is dropped before a label is
/// ever cut. A larger UI font raises the floor past this; see
/// [`right_panel_tabs_floor`].
pub const MIN_WIDTH: f32 = 240.;

fn right_panel_tabs_floor(window: &Window, cx: &gpui::App) -> f32 {
    let chrome = match cfg!(target_os = "macos") {
        true => anterm_ui_kit::window_chrome::trailing_chrome_tiles_w(),
        false => tile_trailing_inset(),
    };
    (TAB_ROW_LEAD + anterm_ui_kit::window_chrome::right_panel_tab_labels_w(window, cx) + chrome)
        .ceil()
}

const TAB_ROW_LEAD: f32 = CONTENT_INSET - TAB_OUTER_PAD;
const TAB_ROW_HEIGHT: f32 = TILE_SIZE_SM + 6.;
pub use anterm_ui_kit::panel_tokens::{
    HEADING, INFO_ROW_H, META, META_MONO, PROC_ROW_H, RESIZE_HANDLE_WIDTH,
    ROW_FILL_RADIUS, ROW_GLYPH, ROW_INSET, SEARCH_H, SECTION_GAP, TAB_TEXT, TEXT, TEXT_INSET,
    TEXT_MONO, action_strip, git_badge, info_chip, panel_search_box, reveal_label,
};
#[cfg(test)]
pub use anterm_ui_kit::panel_tokens::BADGE_W;
const FILES_SEARCH_GAP: f32 = 10.;

/// Active terminal pane snapshot pushed to [`RightPanelView`] for the Info tab.
#[derive(Clone)]
pub struct InfoPaneSnapshot {
    pub pane_id: u64,
    pub forward_ctx: PaneForwardCtx,
    pub cwd: Option<PathBuf>,
    pub display_home: Option<PathBuf>,
    pub is_local: bool,
    pub shell_program: Option<String>,
    pub default_shell_label: String,
    pub ssh_host: Option<String>,
    pub git_status: Option<anterm_ui_kit::git::status::GitStatus>,
    pub diff_target: Option<(HostId, PathBuf)>,
}

/// Workspace context synchronized into [`RightPanelView`] before rendering.
#[derive(Clone, Default)]
pub struct RightPanelContext {
    pub has_tabs: bool,
    pub others_floor: f32,
    pub remote_pane: Option<(u64, String)>,
    pub remote_shell_cwd: Option<String>,
    pub info_snapshot: Option<InfoPaneSnapshot>,
}

/// Events emitted by [`RightPanelView`] to its host workspace (`AntermApp`).
#[derive(Clone)]
pub enum RightPanelEvent {
    CloseAndFocusTerminal,
    ToggleDiffOverlay {
        host: HostId,
        cwd: PathBuf,
    },
    OpenDiff {
        host: HostId,
        root: PathBuf,
        source: DiffSource,
        focus: Option<String>,
    },
    OpenFile(PathBuf),
    OpenRemoteFile {
        host: SharedHost,
        path: PathBuf,
    },
    GotoRemoteShellCwd {
        pane_id: u64,
    },
    FileTree(FileTreeEvent),
    RememberForwardEnabled {
        pane_id: u64,
        forward: ManagedForward,
        enabled: bool,
    },
    Intent(WorkspaceIntent),
}

/// Standalone GPUI entity owning the right sidebar panel and all of its tabs.
pub struct RightPanelView {
    pub ctx: RightPanelContext,
    pub width: Rc<Cell<f32>>,
    pub dragging: Rc<Cell<bool>>,
    pub visible: bool,
    pub tab: RightPanelTab,
    pub file_search: Entity<InputState>,
    pub panel_search: Entity<PanelSearchView>,
    pub file_tree: Entity<FileTreeView>,
    pub scm: Entity<ScmPanelView>,
    pub sftp_panel: Entity<SftpPanelView>,
    pub forwards_panel: Entity<ForwardsPanel>,
    _subscriptions: Vec<Subscription>,
    // Info / Processes / Ports tab state
    pub(super) procs_pane: Option<u64>,
    pub(super) procs: Option<PaneProcs>,
    pub(super) cpu: crate::proc_usage::CpuTracker,
    pub(super) procs_loading: bool,
    pub(super) procs_gen: u64,
    pub(super) procs_forwards: Option<ForwardRoute>,
    pub(super) procs_host: Option<SharedHost>,
    pub(super) procs_unsupported: bool,
    pub(super) link_rtt: Option<std::time::Duration>,
    pub(super) link_host: Option<HostId>,
    pub(super) port_route: anterm_ui_kit::daemon_rpc::PortRoute,
    pub(super) auto_forwarded: HashMap<ForwardOwnerKey, HashSet<u16>>,
    pub(super) scroll: gpui::ScrollHandle,
}

impl EventEmitter<RightPanelEvent> for RightPanelView {}

impl RightPanelView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cfg = cx.global::<Config>();
        let width = cfg.right_panel_width;
        let visible = cfg.right_panel_visible;
        let tab = shown_tab(cfg.right_panel_tab);
        let scm_graph_expanded = cfg.scm_graph_expanded;

        let mut subscriptions = Vec::new();

        let file_search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::SearchFiles))
        });
        subscriptions.push(cx.subscribe_in(
            &file_search,
            window,
            |_this, _input, ev, _window, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));

        let panel_search =
            cx.new(|cx| PanelSearchView::new(file_search.clone(), window, cx));
        subscriptions.push(cx.subscribe_in(
            &panel_search,
            window,
            |_this, _, intent: &WorkspaceIntent, _window, cx| {
                cx.emit(RightPanelEvent::Intent(intent.clone()));
            },
        ));

        let file_tree = cx.new(|cx| {
            FileTreeView::new(file_search.clone(), panel_search.clone(), window, cx)
        });
        subscriptions.push(cx.subscribe_in(
            &file_tree,
            window,
            |this, _, ev: &FileTreeEvent, _window, cx| {
                if matches!(ev, FileTreeEvent::ShowPanel) {
                    this.set_tab(RightPanelTab::Files, cx);
                } else {
                    cx.emit(RightPanelEvent::FileTree(ev.clone()));
                }
            },
        ));
        subscriptions.push(cx.observe(&file_tree, |_, _, cx| cx.notify()));

        let scm = cx.new(|_| ScmPanelView::new(scm_graph_expanded));
        subscriptions.push(cx.subscribe_in(
            &scm,
            window,
            |_this, _, ev: &ScmEvent, _window, cx| match ev {
                ScmEvent::OpenDiff {
                    host,
                    root,
                    source,
                    focus,
                } => cx.emit(RightPanelEvent::OpenDiff {
                    host: *host,
                    root: root.clone(),
                    source: source.clone(),
                    focus: focus.clone(),
                }),
                ScmEvent::OpenFile(path) => cx.emit(RightPanelEvent::OpenFile(path.clone())),
            },
        ));
        subscriptions.push(cx.observe(&scm, |_, _, cx| cx.notify()));

        let sftp_panel = cx.new(|cx| SftpPanelView::new(window, cx));
        subscriptions.push(cx.subscribe_in(
            &sftp_panel,
            window,
            |_this, _, ev: &SftpEvent, _window, cx| match ev {
                SftpEvent::OpenRemoteFile { host, path } => {
                    cx.emit(RightPanelEvent::OpenRemoteFile {
                        host: host.clone(),
                        path: path.clone(),
                    });
                }
                SftpEvent::GotoShellCwd { pane_id } => {
                    cx.emit(RightPanelEvent::GotoRemoteShellCwd { pane_id: *pane_id });
                }
                SftpEvent::Intent(intent) => {
                    cx.emit(RightPanelEvent::Intent(intent.clone()));
                }
            },
        ));
        subscriptions.push(cx.observe(&sftp_panel, |_, _, cx| cx.notify()));

        let forwards_panel = cx.new(|cx| ForwardsPanel::new(window, cx));
        subscriptions.push(cx.subscribe_in(
            &forwards_panel,
            window,
            |_this, _, ev: &ForwardsEvent, _window, cx| match ev {
                ForwardsEvent::Intent(intent) => {
                    cx.emit(RightPanelEvent::Intent(intent.clone()));
                }
                ForwardsEvent::RememberRuleEnabled {
                    pane_id,
                    forward,
                    enabled,
                } => {
                    cx.emit(RightPanelEvent::RememberForwardEnabled {
                        pane_id: *pane_id,
                        forward: forward.clone(),
                        enabled: *enabled,
                    });
                }
            },
        ));
        subscriptions.push(cx.observe(&forwards_panel, |_, _, cx| cx.notify()));

        Self {
            ctx: RightPanelContext::default(),
            width: Rc::new(Cell::new(width)),
            dragging: Rc::new(Cell::new(false)),
            visible,
            tab,
            file_search,
            panel_search,
            file_tree,
            scm,
            sftp_panel,
            forwards_panel,
            _subscriptions: subscriptions,
            procs_pane: None,
            procs: None,
            cpu: Default::default(),
            procs_loading: false,
            procs_gen: 0,
            procs_forwards: None,
            procs_host: None,
            procs_unsupported: false,
            link_rtt: None,
            link_host: None,
            port_route: Default::default(),
            auto_forwarded: HashMap::new(),
            scroll: gpui::ScrollHandle::new(),
        }
    }

    pub fn is_open(&self, has_tabs: bool) -> bool {
        self.visible && has_tabs
    }

    pub fn set_width(&self, w: f32) {
        self.width.set(w);
    }

    pub fn min_width(window: &Window, cx: &gpui::App) -> f32 {
        MIN_WIDTH.max(right_panel_tabs_floor(window, cx))
    }

    pub fn min_px(&self, window: &Window, cx: &gpui::App) -> f32 {
        Self::min_width(window, cx)
    }

    pub fn max_px(&self, others_floor: f32, window: &Window, cx: &gpui::App) -> f32 {
        anterm_ui_kit::window_chrome::side_panel_max(
            window.viewport_size().width.as_f32(),
            self.min_px(window, cx),
            others_floor,
        )
    }

    pub fn width_px(&self, others_floor: f32, window: &Window, cx: &gpui::App) -> f32 {
        self.width.get().clamp(
            self.min_px(window, cx),
            self.max_px(others_floor, window, cx),
        )
    }

    pub fn toggle_visible(&mut self, cx: &mut Context<Self>) {
        let next = !self.visible;
        self.visible = next;
        let _ = ConfigStore::update(cx, |cfg| cfg.right_panel_visible = next);
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: RightPanelTab, cx: &mut Context<Self>) {
        if tab == RightPanelTab::Search {
            self.panel_search.update(cx, |s, _| s.request_focus());
        }
        let tab = shown_tab(tab);
        self.tab = tab;
        self.visible = true;
        let _ = ConfigStore::update(cx, |cfg| {
            cfg.right_panel_tab = tab;
            cfg.right_panel_visible = true;
        });
        cx.notify();
    }

    pub fn refresh_locale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_search.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::SearchFiles), window, cx)
        });
        self.sftp_panel
            .update(cx, |sftp, cx| sftp.refresh_locale(window, cx));
        cx.notify();
    }

    /// Reconcile workspace context & SFTP open state right before rendering.
    pub fn sync_context(&mut self, ctx: RightPanelContext, cx: &mut Context<Self>) -> bool {
        self.ctx = ctx;
        let panel_open = self.is_open(self.ctx.has_tabs);
        if let Some(open) = self.sftp_panel.read(cx).open_pane_id()
            && (!panel_open || self.ctx.remote_pane.as_ref().map(|(id, _)| *id) != Some(open))
        {
            self.sftp_panel
                .update(cx, |sftp, cx| sftp.close_browser(cx));
        }
        if !panel_open {
            self.sftp_panel
                .update(cx, |sftp, _| sftp.mark_panel_closed());
        }
        panel_open
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let active_tab = self.tab;
        let body_ink = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        RIGHT_PANEL_TABS
            .into_iter()
            .map(|(tab, label_key)| {
                let current = active_tab == tab;
                let ink = match current {
                    true => body_ink,
                    false => muted,
                };
                div()
                    .id(("right-panel-tab", tab as usize))
                    .role(gpui::Role::Tab)
                    .aria_label(t(label_key))
                    .aria_selected(current)
                    .occlude()
                    .flex_shrink_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .px(px(TAB_OUTER_PAD))
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(TAB_INNER_PAD))
                            .py_0p5()
                            .rounded_md()
                            .when(current, |s| s.bg(cx.theme().secondary))
                            .text_size(rems(TAB_TEXT))
                            .font_weight(match current {
                                true => FontWeight::MEDIUM,
                                false => FontWeight::NORMAL,
                            })
                            .text_color(ink)
                            .hover(move |s| s.text_color(body_ink))
                            .child(t(label_key)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.tab != tab {
                            this.set_tab(tab, cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn render_close_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let trailing = match cfg!(target_os = "macos") {
            true => tile_trailing_inset(),
            false => 4.,
        };
        h_flex()
            .flex_shrink_0()
            .items_center()
            .pr(px(trailing))
            .child(
                div().occlude().flex_shrink_0().child(
                    chrome_tile(
                        Button::new("titlebar-right-panel")
                            .icon(Icon::empty().path("icons/panel-right.svg")),
                        false,
                        cx,
                    )
                    .rounded_lg()
                    .accessibility_label(t(L10nKey::TabTooltipHideDetailPanel))
                    .tooltip_with_action(
                        t(L10nKey::TabTooltipHideDetailPanel),
                        &anterm_ui_kit::actions::ToggleRightPanel,
                        None,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.toggle_visible(cx);
                        cx.emit(RightPanelEvent::CloseAndFocusTerminal);
                    })),
                ),
            )
            .into_any_element()
    }

    fn right_panel_resize(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        use gpui::{Bounds, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, canvas};
        use std::cell::Cell as StdCell;

        let container: Rc<StdCell<Option<Bounds<Pixels>>>> = Rc::new(StdCell::new(None));
        let others_floor = self.ctx.others_floor;
        let own_floor = self.min_px(window, cx);
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| container.set(Some(bounds))
            },
            {
                let container = container.clone();
                let width_cell = self.width.clone();
                let dragging = self.dragging.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |ev: &MouseMoveEvent, _phase, window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            let Some(b) = container.get() else {
                                return;
                            };
                            let right = b.origin.x + b.size.width;
                            let raw = (right - ev.position.x).as_f32();
                            let max = anterm_ui_kit::window_chrome::side_panel_max(
                                window.viewport_size().width.as_f32(),
                                own_floor,
                                others_floor,
                            );
                            width_cell.set(raw.clamp(own_floor, max));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |_ev: &MouseUpEvent, _phase, window, cx| {
                            if !dragging.get() {
                                return;
                            }
                            dragging.set(false);
                            let w = width_cell.get();
                            let cfg = cx.global_mut::<Config>();
                            if cfg.right_panel_width != w {
                                cfg.right_panel_width = w;
                                cfg.save();
                            }
                            window.refresh();
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element();

        let active = self.dragging.get();
        let handle = div()
            .group("right-panel-resize")
            .occlude()
            .absolute()
            .top_0()
            .left(px(-(RESIZE_HANDLE_WIDTH / 2.)))
            .w(px(RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .when(active, |d| d.bg(cx.theme().drag_border))
                    .group_hover("right-panel-resize", |s| s.bg(cx.theme().drag_border)),
            )
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.dragging.clone();
                move |_ev, window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            })
            .into_any_element();

        (backing, handle)
    }

    pub(super) fn panel_title(
        &self,
        text: &str,
        count: Option<String>,
        trailing: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_trailing = trailing.is_some();
        let tabs = (!cfg!(target_os = "macos")).then(|| self.render_tabs(cx));
        if tabs.is_none() && !has_trailing {
            return div().flex_none().into_any_element();
        }
        let row = anterm_ui_kit::window_chrome::window_move_gesture(
            h_flex().id("panel-title"),
            "panel-title-drag",
            window,
            cx,
        );
        row.flex_none()
            .h(px(if tabs.is_some() { TAB_ROW_HEIGHT } else { 32. }))
            .items_center()
            .pl(px(TEXT_INSET))
            .pr(px(match (&tabs, has_trailing) {
                (Some(_), _) => tile_trailing_inset(),
                (None, true) => TEXT_INSET - anterm_ui_kit::window_chrome::TILE_PAD_SM,
                (None, false) => TEXT_INSET,
            }))
            .when(tabs.is_some(), |row| row.pl(px(TAB_ROW_LEAD)))
            .when(tabs.is_none(), |row| {
                row.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_baseline()
                        .gap(px(7.))
                        .child(
                            div()
                                .text_size(rems(META))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child(text.to_string()),
                        )
                        .when_some(count, |this, c| {
                            this.child(
                                div()
                                    .text_size(rems(META_MONO))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(cx.theme().muted_foreground)
                                    .child(c),
                            )
                        }),
                )
            })
            .when_some(tabs, |this, tiles| {
                this.child(
                    h_flex()
                        .flex_shrink_0()
                        .h_full()
                        .items_center()
                        .children(tiles),
                )
            })
            .child(div().flex_1().min_w_0())
            .when_some(trailing, |this, t| this.child(t))
            .into_any_element()
    }

    pub(super) fn panel_scroll(&self, inner: AnyElement, title: AnyElement) -> AnyElement {
        let body = div()
            .id("right-panel-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(inner);
        v_flex()
            .flex_1()
            .min_h_0()
            .child(title)
            .child(with_vertical_scrollbar(
                "right-panel-body-scrollbar",
                body,
                &self.scroll,
            ))
            .into_any_element()
    }

    fn render_panel_files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let remote = self.ctx.remote_pane.clone();
        let shell_cwd = self.ctx.remote_shell_cwd.clone();
        let host = remote.as_ref().map(|(_, host)| host.clone());
        let is_sftp = self
            .sftp_panel
            .update(cx, |sftp, cx| sftp.sync_pane(remote, shell_cwd, cx));
        if is_sftp {
            let controls = self
                .sftp_panel
                .update(cx, |sftp, cx| sftp.render_controls(cx));
            let title = self.panel_title(
                t(L10nKey::SftpPanelTitleFiles),
                Some(host.unwrap_or_default()),
                Some(controls),
                window,
                cx,
            );
            return v_flex()
                .flex_1()
                .min_h_0()
                .child(title)
                .child(self.sftp_panel.clone())
                .into_any_element();
        }

        self.panel_search
            .update(cx, |s, cx| s.consume_pending_focus(window, cx));
        let title = self.panel_title(t(L10nKey::PanelFilesTitle), None, None, window, cx);
        let toggles = self.panel_search.update(cx, |s, cx| s.render_toggles(cx));
        let search = panel_search_box(&self.file_search, Some(toggles), cx);
        v_flex()
            .flex_1()
            .min_h_0()
            .child(title)
            .child(div().flex_none().pb(px(FILES_SEARCH_GAP)).child(search))
            .child(self.file_tree.clone())
            .into_any_element()
    }
}

impl gpui::Render for RightPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open(self.ctx.has_tabs) {
            return div().into_any_element();
        }
        let width = self.width_px(self.ctx.others_floor, window, cx);
        let tab = self.tab;

        let body = match tab {
            RightPanelTab::Info => self.render_panel_info(window, cx),
            RightPanelTab::Scm => {
                let count = self.scm.read(cx).total_changes(cx).map(|n| n.to_string());
                let title = self.panel_title(t(L10nKey::PanelScmTitle), count, None, window, cx);
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(title)
                    .child(self.scm.clone())
                    .into_any_element()
            }
            RightPanelTab::Files | RightPanelTab::Search => self.render_panel_files(window, cx),
        };
        let (backing, handle) = self.right_panel_resize(window, cx);

        v_flex()
            .id("right-panel")
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .child(backing)
            .bg(anterm_ui_kit::theme::workspace_surface_color(cx))
            .border_l(anterm_ui_kit::theme::hairline(window))
            .border_color(cx.theme().sidebar_border)
            .children(cfg!(target_os = "macos").then(|| {
                let row = h_flex()
                    .id("right-panel-titlebar-drag")
                    .flex_none()
                    .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent);
                anterm_ui_kit::window_chrome::window_move_gesture(
                    row,
                    "right-panel-titlebar-drag",
                    window,
                    cx,
                )
                .on_double_click(|_, window, _| window.titlebar_double_click())
                .items_center()
                .pl(px(TAB_ROW_LEAD))
                .relative()
                .children(self.render_tabs(cx))
                .child(div().flex_1())
                .child(self.render_close_toggle(cx))
            }))
            .children(
                (cfg!(target_os = "macos")
                    && matches!(tab, RightPanelTab::Files | RightPanelTab::Search))
                .then(|| div().flex_none().h(px(8.))),
            )
            .child(body)
            .children(
                self.sftp_panel
                    .update(cx, |sftp, cx| sftp.render_transfers_footer(cx)),
            )
            .child(handle)
            .into_any_element()
    }
}

/// Shared empty-state block usable by any standalone panel entity.
pub fn panel_empty_view(text: &str, hint: Option<&str>, cx: &gpui::App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .px(px(TEXT_INSET))
        .py(px(4.))
        .gap(px(3.))
        .text_size(rems(TEXT))
        .text_color(muted)
        .child(text.to_string())
        .children(hint.map(|h| {
            div()
                .text_size(rems(META))
                .text_color(muted)
                .child(h.to_string())
        }))
        .into_any_element()
}


