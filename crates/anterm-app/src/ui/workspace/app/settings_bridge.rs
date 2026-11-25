//! Workspace-side config reaction, sidebar toggles, typed SSH connect, and thin
//! launcher for the autonomous [`SettingsWindow`].
//!
//! All settings state, page rendering, form validation, keybinding recording,
//! and config mutations now live inside [`crate::ui::chrome::settings`].

use gpui::{Context, Window, prelude::*, px};

use crate::core::config::{Config, TabBarPosition};
use crate::core::config_store::ConfigStore;
use crate::core::ssh_config;
use crate::ui::chrome::settings::SettingsSection;
use crate::ui::chrome::settings::window::{SettingsWindow, SettingsWindowHandle};
use crate::ui::chrome::theme::apply_theme;

use super::AntermApp;
use super::session_spawn::parse_ssh_connect_input;

type SettingsWindowAction =
    Box<dyn FnOnce(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>)>;

impl AntermApp {
    fn apply_terminal_config_to_panes(&self, config: &Config, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                leaf.update(cx, |v, cx| {
                    v.terminal.apply_user_config(config);
                    cx.notify();
                });
            }
        }
    }

    pub(crate) fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut Config),
    ) {
        let _ = ConfigStore::update(cx, mutate);
        cx.notify();
    }

    pub(crate) fn set_notify_mode(
        &mut self,
        mode: crate::core::config::NotifyMode,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.notify_on_command_finish = mode);
    }

    pub(super) fn open_typed_ssh_connect(
        &mut self,
        input: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match parse_ssh_connect_input(input) {
            Ok(parsed) => {
                let (profile, proxy_jump) =
                    match ssh_config::resolve_alias_to_profile(&parsed.profile.host) {
                        Some(resolved) => {
                            let mut p = resolved.profile;
                            if !parsed.profile.user.is_empty() {
                                p.user = parsed.profile.user;
                            }
                            if parsed.profile.port != 22 {
                                p.port = parsed.profile.port;
                            }
                            if !parsed.profile.identity_files.is_empty() {
                                p.identity_files = parsed.profile.identity_files;
                            }
                            (p, parsed.proxy_jump.or(resolved.proxy_jump))
                        }
                        None => (parsed.profile, parsed.proxy_jump),
                    };
                let verify = cx.global::<Config>().verify_host_keys;
                let spec = crate::ui::remote::ssh_connect::native_spec_from_transient_profile(
                    &profile,
                    proxy_jump,
                    &crate::core::keychain::OsCredentialStore,
                    verify,
                    &crate::ui::remote::ssh_connect::config_alias_resolver,
                );
                self.open_native_ssh_tab(Box::new(spec), None, window, cx);
            }
            Err(reason) => self
                .ssh_prompt
                .update(cx, |modal, cx| modal.push_connect_error(reason, cx)),
        }
    }

    pub(crate) fn toggle_tab_sidebar(&mut self, cx: &mut Context<Self>) {
        let next = match cx.global::<Config>().tab_bar_position {
            TabBarPosition::Top => TabBarPosition::Left,
            TabBarPosition::Left => TabBarPosition::Top,
        };
        self.update_config(cx, |cfg| cfg.tab_bar_position = next);
    }

    pub(crate) fn toggle_left_panel(&mut self, cx: &mut Context<Self>) {
        let (pos, collapsed) = match cx.global::<Config>().tab_bar_position {
            TabBarPosition::Top => (TabBarPosition::Left, false),
            TabBarPosition::Left => (TabBarPosition::Left, !self.sidebar_collapsed),
        };
        self.sidebar_collapsed = collapsed;
        self.update_config(cx, |cfg| {
            cfg.tab_bar_position = pos;
            cfg.sidebar_collapsed = collapsed;
        });
        cx.notify();
    }

    pub(crate) fn left_panel_open(&self, cx: &gpui::App) -> bool {
        matches!(cx.global::<Config>().tab_bar_position, TabBarPosition::Left)
            && !self.sidebar_collapsed
            && !self.tabs.is_empty()
    }

    // ── SettingsWindow launcher & command router ─────────────────────────

    /// Opens Settings in its own autonomous window, or brings it forward if
    /// already open.
    pub(super) fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.settings_window.clone() {
            if handle
                .window
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return;
            }
            self.settings_window = None;
        }
        self.remember_active_pane(window, cx);
        self.open_settings_window_then(window, cx, None);
    }

    fn open_settings_window_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: Option<SettingsWindowAction>,
    ) {
        let owner = window.window_handle();
        let app = cx.entity();
        cx.defer(move |cx| {
            let options = crate::ui::chrome::settings::window::window_options(cx);
            let mut view_weak = None;
            let opened = cx.open_window(options, |window, cx| {
                let host = build_settings_host(&app, Some(owner));
                let view = cx.new(|cx| {
                    let release = cx.observe_release_in(&app, window, |_, _, window, _| {
                        window.remove_window();
                    });
                    SettingsWindow::new(Some(host), Some(release), window, cx)
                });
                if let Some(then) = then {
                    view.update(cx, |this, cx| then(this, window, cx));
                }
                view_weak = Some(view.downgrade());
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
            });
            match (opened, view_weak) {
                (Ok(handle), Some(view)) => app.update(cx, |this, cx| {
                    this.settings_window = Some(SettingsWindowHandle {
                        window: handle.into(),
                        view,
                    });
                    cx.notify();
                }),
                (Err(e), _) => {
                    log::error!("failed to open the settings window: {e}");
                }
                _ => {}
            }
        });
    }

    pub(crate) fn forget_settings_window(&mut self, cx: &mut Context<Self>) {
        if self.settings_window.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn open_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_then(window, cx, move |settings, window, cx| {
            settings.navigate_settings(section, None, window, cx);
        });
    }

    pub(crate) fn open_settings_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) + 'static,
    ) {
        if let Some(handle) = self.settings_window.clone() {
            if handle.view.upgrade().is_some() {
                cx.defer(move |cx| {
                    let _ = handle.window.update(cx, |_, window, cx| {
                        window.activate_window();
                        let _ = handle.view.update(cx, |this, cx| then(this, window, cx));
                    });
                });
                return;
            }
            self.settings_window = None;
        }
        self.remember_active_pane(window, cx);
        self.open_settings_window_then(window, cx, Some(Box::new(then)));
    }

    pub(crate) fn open_ssh_profile_form(
        &mut self,
        profile: crate::core::ssh_profile::SshProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_then(window, cx, move |settings, window, cx| {
            settings.with_settings_edits_resolved(window, cx, move |settings, window, cx| {
                settings.navigate_settings(SettingsSection::Ssh, None, window, cx);
                settings.ssh_form_load(&profile, window, cx);
            });
        });
    }

    pub(crate) fn open_ssh_profile_in_settings(
        &mut self,
        id: uuid::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_then(window, cx, move |settings, window, cx| {
            settings.with_settings_edits_resolved(window, cx, move |settings, window, cx| {
                if let Some(profile) = cx
                    .global::<Config>()
                    .ssh_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .cloned()
                {
                    settings.navigate_settings(SettingsSection::Ssh, None, window, cx);
                    settings.ssh_form_load(&profile, window, cx);
                } else {
                    settings.navigate_settings(SettingsSection::Ssh, None, window, cx);
                }
            });
        });
    }

    pub(crate) fn open_ssh_profile_new_from_target(
        &mut self,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut profile = crate::core::ssh_profile::SshProfile::new(String::new());
        if let Some(qc) = crate::core::ssh_profile::parse_quick_connect(&target) {
            profile.port = qc.port_or_default();
            profile.host = qc.host;
            if let Some(user) = qc.user {
                profile.user = user;
            }
            if profile.name.is_empty() {
                profile.name = profile.host.clone();
            }
        }
        self.open_settings_then(window, cx, move |settings, window, cx| {
            settings.with_settings_edits_resolved(window, cx, move |settings, window, cx| {
                settings.navigate_settings(SettingsSection::Ssh, None, window, cx);
                settings.ssh_form_load(&profile, window, cx);
            });
        });
    }

    // ── Reactive Config & Locale synchronization ─────────────────────────

    pub(crate) fn refresh_locale_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.right_panel
            .update(cx, |rp, cx| rp.refresh_locale(window, cx));
        cx.notify();
    }

    pub(super) fn reload_from_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        apply_theme(Some(window), cx);
        let config = cx.global::<Config>().clone();
        if config.cursor_style != self.applied_terminal_config.cursor_style
            || config.scrollback_limit != self.applied_terminal_config.scrollback_limit
        {
            self.applied_terminal_config.cursor_style = config.cursor_style;
            self.applied_terminal_config.scrollback_limit = config.scrollback_limit;
            self.apply_terminal_config_to_panes(&config, cx);
        }
        let (font_size, line_height, font_family, font_fallbacks, font_features) = (
            config.font_size,
            config.line_height,
            config.font_family.clone(),
            config.font_fallbacks.clone(),
            config
                .font_features
                .as_ref()
                .map(crate::core::config::gpui_font_features),
        );
        self.sidebar_width.set(config.sidebar_width);
        self.right_panel.read(cx).set_width(config.right_panel_width);
        self.editor
            .read(cx)
            .editor
            .document_ratio
            .set(config.document_ratio);
        if font_size != self.font_size {
            self.font_size = font_size;
            let px_size = px(font_size);
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    leaf.update(cx, |v, cx| v.set_font_size(px_size, cx));
                }
            }
        }
        if line_height != self.applied_terminal_config.line_height {
            self.applied_terminal_config.line_height = line_height;
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    leaf.update(cx, |v, cx| v.set_line_height_mul(line_height, cx));
                }
            }
        }
        if font_family != self.applied_terminal_config.font_family {
            self.applied_terminal_config.font_family = font_family.clone();
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    let family = font_family.clone();
                    leaf.update(cx, |v, cx| v.set_font_family(family, cx));
                }
            }
        }
        if font_fallbacks != self.applied_terminal_config.font_fallbacks {
            self.applied_terminal_config.font_fallbacks = font_fallbacks;
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    leaf.update(cx, |v, cx| {
                        v.reread_fallback_chain(cx);
                        cx.notify();
                    });
                }
            }
        }
        if font_features != self.applied_terminal_config.font_features {
            self.applied_terminal_config.font_features = font_features.clone();
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    let features = font_features.clone();
                    leaf.update(cx, |v, cx| v.set_font_features(features, cx));
                }
            }
        }
        if config.font_family_bold != self.applied_terminal_config.font_family_bold {
            self.applied_terminal_config.font_family_bold = config.font_family_bold.clone();
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    let bold = config.font_family_bold.clone();
                    leaf.update(cx, |v, cx| v.set_font_family_bold(bold, cx));
                }
            }
        }
        if config.font_family_italic != self.applied_terminal_config.font_family_italic {
            self.applied_terminal_config.font_family_italic = config.font_family_italic.clone();
            for tab in &self.tabs {
                for leaf in tab.pane.terminals() {
                    let italic = config.font_family_italic.clone();
                    leaf.update(cx, |v, cx| v.set_font_family_italic(italic, cx));
                }
            }
        }
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                leaf.update(cx, |v, cx| {
                    v.set_report_mouse(config.mouse_reporting, cx);
                    if !config.cursor_blink {
                        v.ensure_cursor_visible(cx);
                    }
                    v.set_prompt_editor(config.prompt_editor, cx);
                    v.set_helix_mode(config.helix_mode, cx);
                });
            }
        }
        let helix_mode = config.helix_mode;
        let git_gutter = config.editor_git_gutter;
        self.editor.update(cx, |ed, cx| {
            ed.sync_editor_helix_mode(helix_mode);
            ed.sync_editor_git_gutter(git_gutter, cx);
        });
        cx.notify();
    }
}

pub(crate) fn build_settings_host(
    app: &gpui::Entity<AntermApp>,
    owner_window: Option<gpui::AnyWindowHandle>,
) -> crate::ui::chrome::settings::window::SettingsHost {
    let weak_forget = app.downgrade();
    let weak_focus = app.downgrade();
    let weak_palette = app.downgrade();
    let weak_connect = app.downgrade();
    let weak_restart = app.downgrade();
    let weak_shells = app.downgrade();
    let weak_live = app.downgrade();
    crate::ui::chrome::settings::window::SettingsHost {
        owner_window,
        on_forget: Box::new(move |cx| {
            if let Some(app) = weak_forget.upgrade() {
                app.update(cx, |this, cx| this.forget_settings_window(cx));
            }
        }),
        on_focus_owner: Box::new(move |window, cx| {
            if let Some(app) = weak_focus.upgrade() {
                app.update(cx, |this, cx| this.focus_active(window, cx));
            }
        }),
        on_open_palette: std::rc::Rc::new(move |window, cx| {
            if let Some(app) = weak_palette.upgrade() {
                app.update(cx, |this, cx| {
                    if this.search.is_none() {
                        this.toggle_search(window, cx);
                    }
                });
            }
        }),
        on_connect_ssh: Box::new(move |profile_id, window, cx| {
            if let Some(app) = weak_connect.upgrade() {
                app.update(cx, |this, cx| this.connect_ssh_profile(profile_id, window, cx));
            }
        }),
        on_restart_daemon: Box::new(move |window, cx| {
            if let Some(app) = weak_restart.upgrade() {
                app.update(cx, |this, cx| this.restart_daemon(window, cx));
            }
        }),
        detected_shells: Box::new(move |cx| {
            weak_shells
                .upgrade()
                .map(|app| {
                    app.read(cx)
                        .shells
                        .shells
                        .iter()
                        .filter(|shell| !shell.user_authored)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        }),
        live_ssh_profiles: Box::new(move |cx| {
            use crate::daemon::protocol::SshPhase;
            let mut live = std::collections::HashSet::new();
            let Some(app) = weak_live.upgrade() else {
                return live;
            };
            let app = app.read(cx);
            for tab in &app.tabs {
                for leaf in tab.pane.terminals() {
                    let v = leaf.read(cx);
                    if !matches!(v.ssh_phase(), Some(SshPhase::Connected)) || v.terminal.exited {
                        continue;
                    }
                    if let Some(id) = v
                        .ssh_spec()
                        .and_then(|s| s.profile_id.as_deref().and_then(|id| uuid::Uuid::parse_str(id).ok()))
                    {
                        live.insert(id);
                    }
                }
            }
            live
        }),
    }
}
