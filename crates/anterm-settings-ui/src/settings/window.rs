//! Autonomous Settings window.
//!
//! `SettingsWindow` owns [`SettingsState`] directly and observes only the global
//! [`Config`] and window appearance — never the workspace's [`AntermApp`].
//! Switching tabs, terminal output, and other main-window updates therefore do
//! not trigger any Settings redraws, and `SettingsWindow::render` never calls
//! `app.update(...)`.

use std::cell::Cell;
use std::collections::HashSet;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Focusable as _, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, Styled as _,
    Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowDecorations,
    WindowOptions, px, size,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, TitleBar};
use uuid::Uuid;

use super::{SettingsSection, SettingsState};
use anterm_ui_kit::actions::{CloseActiveTab, OpenSettings};
use anterm_ui_kit::config::Config;
use anterm_ui_kit::config_store::ConfigStore;
use anterm_i18n::{L10nKey, t};

/// Big enough for the navigation column and a comfortable page beside it.
const DEFAULT_SIZE: (f32, f32) = (960., 700.);
const MIN_SIZE: (f32, f32) = (720., 480.);

/// Remembered settings section & scroll offset across open/close cycles.
#[derive(Clone, Copy)]
pub struct LastSettingsLocation {
    pub section: SettingsSection,
    pub scroll_offset: Point<Pixels>,
}

impl Default for LastSettingsLocation {
    fn default() -> Self {
        Self {
            section: SettingsSection::General,
            scroll_offset: gpui::point(px(0.), px(0.)),
        }
    }
}

impl Global for LastSettingsLocation {}

/// Handle retained by `AntermApp` so workspace shortcuts can activate or route
/// explicit navigation commands to the open `SettingsWindow` without owning any
/// settings state.
#[derive(Clone)]
pub struct SettingsWindowHandle {
    pub window: AnyWindowHandle,
    pub view: WeakEntity<SettingsWindow>,
}

pub fn window_options(cx: &mut App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
            cx,
        ))),
        app_id: Some("anterm".to_owned()),
        titlebar: Some(TitlebarOptions {
            title: Some(t(L10nKey::SettingsWindowTitle).into()),
            traffic_light_position: Some(anterm_ui_kit::theme::traffic_light_position()),
            ..TitleBar::title_bar_options()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_background: anterm_ui_kit::theme::background_appearance(cx),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        ..Default::default()
    }
}

pub struct SettingsWindow {
    pub state: SettingsState,
    pub record_gen: u64,
    pub settings_row_width: Cell<f32>,
    pub settings_viewport_w: Cell<f32>,
    pub settings_hit_anchored: Cell<bool>,
    pub host: Option<SettingsHost>,
    last_gui_language: String,
    last_effective_preset: String,
    _config_watch: Subscription,
    _appearance_watch: Subscription,
    _release_watch: Option<Subscription>,
}

/// Callbacks supplied by the hosting workspace so [`SettingsWindow`] never
/// depends on `AntermApp` or `WindowRegistry`.
pub struct SettingsHost {
    pub owner_window: Option<AnyWindowHandle>,
    pub on_forget: Box<dyn Fn(&mut App)>,
    pub on_focus_owner: Box<dyn Fn(&mut Window, &mut App)>,
    pub on_open_palette: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
    pub on_connect_ssh: Box<dyn Fn(Uuid, &mut Window, &mut App)>,
    pub on_restart_daemon: Box<dyn Fn(&mut Window, &mut App)>,
    pub detected_shells: Box<dyn Fn(&App) -> Vec<anterm_core::shells::DetectedShell>>,
    pub live_ssh_profiles: Box<dyn Fn(&App) -> HashSet<Uuid>>,
}

impl SettingsWindow {
    pub fn new(
        host: Option<SettingsHost>,
        release_watch: Option<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let mut subs = Vec::new();
        let font_names = std::rc::Rc::new(Self::settings_font_names(cx));
        let (shell_program_input, shell_args_input, wd_path_input) =
            Self::build_shell_inputs(&mut subs, window, cx);
        let link_file_command_input = Self::build_link_file_command_input(&mut subs, window, cx);
        let http_proxy_input = Self::build_http_proxy_input(&mut subs, window, cx);
        let mobile_store = Self::init_mobile_store();
        let mobile_relay_input =
            Self::build_mobile_relay_input(mobile_store.as_ref(), &mut subs, window, cx);
        let mobile_rename_input = Self::build_mobile_rename_input(&mut subs, window, cx);

        let menu_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SearchTheme)));
        subs.push(
            cx.subscribe_in(&menu_query, window, |this, input, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    let typed = !input.read(cx).value().is_empty();
                    if typed {
                        this.state.menu_hi = 0;
                    }
                    cx.notify();
                }
            }),
        );

        let agent_query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SettingsSearchAgents)));
        subs.push(
            cx.subscribe_in(&agent_query, window, |_this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );

        let settings_search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SearchSettings)));
        subs.push(
            cx.subscribe_in(&settings_search, window, |this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.autoselect_settings_search(cx);
                    this.state.reveal_first_hit.set(true);
                    cx.notify();
                }
            }),
        );

        let shortcut_search = cx
            .new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::SettingsSearchShortcuts)));
        subs.push(
            cx.subscribe_in(&shortcut_search, window, |_, _, ev, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );

        let ssh_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::FilterHosts)));
        subs.push(
            cx.subscribe_in(&ssh_filter, window, |_this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            }),
        );

        let last_loc = *cx.default_global::<LastSettingsLocation>();
        let content_scroll = gpui::ScrollHandle::new();
        content_scroll.set_offset(last_loc.scroll_offset);
        let search_anchor = gpui::ScrollAnchor::for_handle(content_scroll.clone());

        let search_focus = settings_search.read(cx).focus_handle(cx);
        let cfg = cx.global::<Config>().clone();
        let last_gui_language = cfg.gui_language.clone();
        let last_effective_preset = anterm_ui_kit::theme::effective_preset_id(cx);

        let state = SettingsState {
            focus_handle: focus_handle.clone(),
            section: last_loc.section,
            search: settings_search,
            shortcut_search,
            search_active: false,
            search_return_offset: last_loc.scroll_offset,
            search_selection: 0,
            search_rows: std::cell::RefCell::new(None),
            focused_setting: None,
            content_scroll,
            search_anchor,
            reveal_first_hit: Cell::new(false),
            font_names,
            shell_program_input,
            shell_args_input,
            wd_path_input,
            link_file_command_input,
            http_proxy_input,
            theme_editor: None,
            theme_draft: None,
            theme_draft_error: None,
            save_error: ConfigStore::save_error(cx).map(str::to_owned),
            saved_config: cfg,
            menu: None,
            menu_query,
            menu_hi: 0,
            menu_scroll: gpui::ScrollHandle::new(),
            focused: std::cell::RefCell::new(None),
            kb_conflict: None,
            recording: None,
            rebinding_note: None,
            ssh_form: None,
            ssh_open: None,
            ssh_show_all: false,
            ssh_confirm_remove: false,
            ssh_copied: None,
            ssh_filter,
            ssh_collapsed_groups: HashSet::new(),
            ssh_list_scroll: gpui::ScrollHandle::new(),
            agent_hooks_host: anterm_ui_kit::host_ops::HostId::LOCAL,
            agent_hooks_states: super::AgentHooksView::Loading,
            agent_hooks_seq: 0,
            agent_hooks_note: None,
            agent_query,
            agent_list_scroll: gpui::ScrollHandle::new(),
            agent_show_all: false,
            agent_touched: HashSet::new(),
            agent_busy: HashSet::new(),
            mobile_store,
            mobile_relay_input,
            mobile_pair_role: anterm_gateway::MobileRole::Interactive,
            mobile_pairing_qr: None,
            mobile_status_note: None,
            mobile_rename_target: None,
            mobile_rename_input,
            _subs: subs,
        };

        // Observe global Config (never AntermApp) so Settings stays in sync with
        // external config edits or command-palette changes without redrawing on
        // unrelated workspace activity.
        let config_watch = cx.observe_global_in::<Config>(window, |this, window, cx| {
            this.on_config_changed(window, cx);
        });

        let weak_for_appearance = cx.weak_entity();
        let appearance_watch = window.observe_window_appearance(move |window, cx| {
            anterm_ui_kit::theme::note_system_appearance(window, cx);
            if !cx.global::<Config>().theme_follow_system {
                return;
            }
            anterm_ui_kit::theme::apply_theme(Some(window), cx);
            let _ = weak_for_appearance.update(cx, |this, cx| {
                this.rebuild_theme_editor(window, cx);
                cx.notify();
            });
        });

        let weak_self = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| match weak_self.upgrade() {
            Some(view) => {
                view.update(cx, |this, cx| this.close_settings_checked(window, cx));
                false
            }
            None => true,
        });

        window.focus(&search_focus, cx);

        let mut this = Self {
            state,
            record_gen: 0,
            settings_row_width: Cell::new(f32::MAX),
            settings_viewport_w: Cell::new(f32::MAX),
            settings_hit_anchored: Cell::new(false),
            host,
            last_gui_language,
            last_effective_preset,
            _config_watch: config_watch,
            _appearance_watch: appearance_watch,
            _release_watch: release_watch,
        };
        this.rebuild_theme_editor(window, cx);
        this.ensure_agent_hooks_loaded(cx);
        this
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new_standalone(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(None, None, window, cx)
    }

    pub fn active_settings(&self) -> Option<&SettingsState> {
        Some(&self.state)
    }

    pub fn active_settings_mut(&mut self) -> Option<&mut SettingsState> {
        Some(&mut self.state)
    }

    fn on_config_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = cx.global::<Config>().clone();
        let error = ConfigStore::save_error(cx).map(str::to_owned);
        if error.is_none() {
            self.state.saved_config = cfg.clone();
        }
        self.state.save_error = error;

        if cfg.gui_language != self.last_gui_language {
            self.last_gui_language = cfg.gui_language.clone();
            self.refresh_locale(window, cx);
        }

        let effective = anterm_ui_kit::theme::effective_preset_id(cx);
        if effective != self.last_effective_preset {
            self.last_effective_preset = effective;
            self.rebuild_theme_editor(window, cx);
        }
        cx.notify();
    }

    pub fn refresh_locale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = &self.state;
        s.search.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::SearchSettings), window, cx)
        });
        s.ssh_filter.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::FilterHosts), window, cx)
        });
        s.shortcut_search.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::SettingsSearchShortcuts), window, cx)
        });
        s.agent_query.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::SettingsSearchAgents), window, cx)
        });
        s.shell_args_input.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::AppPlaceholderNone), window, cx)
        });
        if !cfg!(windows) {
            s.shell_program_input.update(cx, |state, cx| {
                state.set_placeholder(t(L10nKey::AppPlaceholderLoginShell), window, cx)
            });
        }
        s.link_file_command_input.update(cx, |state, cx| {
            state.set_placeholder(t(L10nKey::AppPlaceholderOpenInDefaultApp), window, cx)
        });
        cx.notify();
    }

    /// Saves the current section/scroll location, detaches from the owner
    /// workspace, closes this window, and returns focus to the owner workspace.
    pub fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scroll_offset = if self.state.search_active {
            self.state.search_return_offset
        } else {
            self.state.content_scroll.offset()
        };
        *cx.default_global::<LastSettingsLocation>() = LastSettingsLocation {
            section: self.state.section,
            scroll_offset,
        };
        let host = self.host.take();
        if let Some(h) = host.as_ref() {
            (h.on_forget)(cx);
        }
        window.remove_window();
        if let Some(host) = host {
            if let Some(owner) = host.owner_window {
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |_, window, cx| {
                        window.activate_window();
                        (host.on_focus_owner)(window, cx);
                    });
                });
            }
        }
    }

    /// Closes Settings and opens the chosen SSH profile in the owner workspace.
    pub fn close_and_connect_ssh_profile(
        &mut self,
        profile_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scroll_offset = if self.state.search_active {
            self.state.search_return_offset
        } else {
            self.state.content_scroll.offset()
        };
        *cx.default_global::<LastSettingsLocation>() = LastSettingsLocation {
            section: self.state.section,
            scroll_offset,
        };
        let host = self.host.take();
        if let Some(h) = host.as_ref() {
            (h.on_forget)(cx);
        }
        window.remove_window();
        if let Some(host) = host {
            if let Some(owner) = host.owner_window {
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |_, window, cx| {
                        window.activate_window();
                        (host.on_connect_ssh)(profile_id, window, cx);
                    });
                });
            }
        }
    }

    /// Routes the Maintenance page's "Restart server" action to the owner
    /// workspace without holding a reactive subscription to it.
    pub fn restart_daemon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(host) = self.host.as_ref() {
            (host.on_restart_daemon)(window, cx);
        }
    }

    /// Read-only snapshot of detected local shells from the owner workspace.
    pub fn detected_shells(&self, cx: &App) -> Vec<anterm_core::shells::DetectedShell> {
        self.host
            .as_ref()
            .map(|h| (h.detected_shells)(cx))
            .unwrap_or_default()
    }

    /// Read-only snapshot of SSH profile UUIDs currently connected in the owner
    /// workspace.
    pub fn live_ssh_profiles(&self, cx: &App) -> HashSet<Uuid> {
        self.host
            .as_ref()
            .map(|h| (h.live_ssh_profiles)(cx))
            .unwrap_or_default()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-support"))]
        render_probe::record();

        anterm_ui_kit::kit::note_scale(window);
        let page = self.render_settings(window, cx);
        gpui::div()
            .size_full()
            .bg(anterm_ui_kit::theme::overlay_background(cx))
            .text_color(cx.theme().foreground)
            // ⌘W closes this window the way it closes a tab in a workspace.
            .on_action(
                cx.listener(|this, _: &CloseActiveTab, window, cx| {
                    this.close_settings_checked(window, cx)
                }),
            )
            // ⌘, lands here while this window has focus; it is already open.
            .on_action(cx.listener(|_, _: &OpenSettings, window, _| window.activate_window()))
            .child(page)
    }
}

/// Counts `SettingsWindow::render` invocations in tests so we can assert that
/// main-window tab switches and workspace mutations cause zero Settings redraws.
#[cfg(any(test, feature = "test-support"))]
pub mod render_probe {
    use std::cell::Cell;

    thread_local! {
        static RENDERS: Cell<u64> = const { Cell::new(0) };
    }

    pub fn record() {
        RENDERS.set(RENDERS.get() + 1);
    }

    pub fn take() -> u64 {
        RENDERS.replace(0)
    }
}
