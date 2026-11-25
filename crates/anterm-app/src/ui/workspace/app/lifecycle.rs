use gpui::{AnyWindowHandle, App, Context, PromptLevel, Window, prelude::*};
use gpui_kit::component::WindowExt as _;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::core::config::Config;
use crate::core::session::{Session, SessionTab};
use crate::core::shells::ShellInventory;
use crate::core::window_state::{WindowGeometry as _, WindowState};
use crate::daemon::spawn::DaemonMismatch;
use crate::ui::chrome::theme::{apply_theme, set_menus};
use crate::ui::remote::host_registry::HostId;
use crate::ui::workspace::pane::Pane;
use anterm_i18n::{L10nKey, t, t_fmt, t_plural};

use super::session_spawn::{alive_panes, new_terminal, session_to_pane, tabs_from_session};
use super::{
    AntermApp, HOME_CURSOR_BLINK_MS, REOPEN_SETTLE_POLL, REOPEN_SETTLE_POLLS, Tab,
    window_bounds_to_remember,
};

impl AntermApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_at(None, window, cx)
    }

    pub fn new_at(
        mut initial_cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let restore = cx.global::<Config>().restore_session;
        let on_machine = crate::ui::remote::machine_mirror::machine_holds_tabs(cx);
        let hydrate = on_machine || restore;
        let session = hydrate.then(Session::default);
        // A window that is about to pull its layout cannot also open the folder
        // the launch asked for as its first terminal — the pull would find a
        // window that already has a tab, decline to adopt into it, and push
        // that one tab back as the whole tree. So the folder travels with the
        // hydration and becomes a tab once the layout is up.
        let open_after_hydrate = hydrate.then(|| initial_cwd.take()).flatten();
        let app = Self::with_session_at(session, initial_cwd, window, cx);
        if hydrate {
            match open_after_hydrate {
                Some(cwd) => {
                    crate::ui::remote::tree_sync::hydrate_window_then_open(cx, cwd);
                }
                None => crate::ui::remote::tree_sync::hydrate_window_from_tree(cx),
            }
        } else {
            crate::ui::remote::tree_sync::mark_window_informed(cx);
            app.save_session(cx);
        }
        Self::prompt_daemon_version_mismatch(window, cx);
        Self::prompt_deferred_adoption(window, cx);
        app
    }

    pub(crate) fn spawn_host(&self, _cx: &App) -> HostId {
        HostId::LOCAL
    }

    pub(crate) fn default_shell_label(&self, _cx: &App) -> String {
        self.shells.default_name.clone()
    }

    pub(crate) fn refresh_shells(&mut self, cx: &mut Context<Self>) {
        let Some(host) = crate::ui::remote::host_registry::HostRegistry::get(cx, HostId::LOCAL)
        else {
            return;
        };
        crate::ui::remote::host_ops::HostOps::run(
            host,
            cx,
            |h| h.shells(),
            |this, res, cx| {
                if let Ok(inv) = res {
                    this.shells = inv;
                    cx.notify();
                }
            },
        );
    }

    /// Put a version-mismatched local server to the user, with no third way out.
    fn prompt_daemon_version_mismatch(window: &mut Window, cx: &mut Context<Self>) {
        let Some(mismatch) = crate::daemon::spawn::take_mismatched_daemon() else {
            return;
        };
        let ours = crate::daemon::protocol::PROTOCOL_VERSION;
        let detail = match &mismatch {
            DaemonMismatch::Protocol(Some(v)) => t_fmt(
                L10nKey::AppRestartServerMismatchDetail,
                &[
                    ("build", &v.build.to_string()),
                    ("protocol", &v.protocol.to_string()),
                    ("ours", &ours.to_string()),
                ],
            ),
            DaemonMismatch::Protocol(None) => t(L10nKey::AppRestartServerOldDetail).to_string(),
            DaemonMismatch::Dialect(refusal) => t_fmt(
                if refusal.peer < refusal.ours {
                    L10nKey::AppRestartServerDialectDetail
                } else {
                    L10nKey::AppRestartServerDialectNewerDetail
                },
                &[
                    ("build", &refusal.peer_build),
                    ("dialect", &refusal.peer.to_string()),
                    ("ours", &refusal.ours.to_string()),
                ],
            ),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            t(L10nKey::AppRestartServerTitle),
            Some(&detail),
            &[t(L10nKey::CmdQuitAnterm), t(L10nKey::RestartServer)],
            cx,
        );
        cx.spawn(async move |this, cx| match answer.await {
            Ok(1) => {
                let _ = this.update_in(cx, |this, _window, cx| this.restart_daemon_confirmed(cx));
            }
            Ok(_) => {
                let _ = cx.update(|cx| cx.quit());
            }
            Err(_) => crate::daemon::spawn::note_daemon_mismatch(mismatch),
        })
        .detach();
    }

    fn prompt_deferred_adoption(window: &mut Window, cx: &mut Context<Self>) {
        let Some(build) = crate::daemon::spawn::take_deferred_adoption() else {
            return;
        };
        let detail = t_fmt(
            L10nKey::AppRestartServerAfterUpdateDetail,
            &[("build", &build), ("ours", env!("CARGO_PKG_VERSION"))],
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            t(L10nKey::AppRestartServerTitle),
            Some(&detail),
            &crate::ui::confirm_answers(
                t(L10nKey::AppRestart),
                t(anterm_i18n::L10nKey::Cancel),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |this, _window, cx| this.restart_daemon_confirmed(cx));
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn with_session(
        session: Option<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_session_at(session, None, window, cx)
    }

    fn with_session_at(
        session: Option<Session>,
        initial_cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (font_size, applied_terminal_config, sidebar_width, sidebar_collapsed) = {
            let cfg = cx.global::<Config>();
            (
                cfg.font_size,
                super::TerminalConfigSnapshot::from_config(cfg),
                cfg.sidebar_width,
                cfg.sidebar_collapsed,
            )
        };
        let editor = cx.new(|cx| crate::ui::editor::CodeEditorWorkspace::new(window, cx));
        cx.subscribe_in(
            &editor,
            window,
            |this, _, ev: &crate::ui::editor::CodeEditorEvent, window, cx| {
                this.handle_editor_event(ev, window, cx);
            },
        )
        .detach();
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        let right_panel =
            cx.new(|cx| crate::ui::files::right_panel::RightPanelView::new(window, cx));
        cx.subscribe_in(
            &right_panel,
            window,
            |this, _, ev: &crate::ui::files::right_panel::RightPanelEvent, window, cx| {
                this.handle_right_panel_event(ev, window, cx);
            },
        )
        .detach();
        cx.observe(&right_panel, |_, _, cx| cx.notify()).detach();
        let ssh_prompt = cx.new(crate::ui::remote::ssh_prompt::SshPromptModal::new);
        cx.subscribe_in(
            &ssh_prompt,
            window,
            |this, modal, ev: &crate::ui::remote::ssh_prompt::SshPromptEvent, window, cx| {
                use crate::ui::remote::ssh_prompt::SshPromptEvent;
                match ev {
                    SshPromptEvent::Answered {
                        pane_id,
                        request_id,
                        response,
                    } => {
                        if let Some(view) = this
                            .tabs
                            .iter()
                            .flat_map(|t| t.pane.terminals())
                            .find(|l| l.read(cx).pane_id == *pane_id)
                        {
                            view.read(cx).terminal.respond_auth(*request_id, response.clone());
                        }
                    }
                    SshPromptEvent::Dismissed { previous_pane_id } => {
                        let same_pane = previous_pane_id.and_then(|pid| {
                            this.tabs
                                .iter()
                                .flat_map(|t| t.pane.terminals())
                                .find(|l| {
                                    l.read(cx).pane_id == pid
                                        && l.read(cx).terminal.has_pending_auth()
                                })
                        });
                        let waiting = same_pane.or_else(|| {
                            this.tabs
                                .iter()
                                .flat_map(|t| t.pane.terminals())
                                .find(|l| l.read(cx).terminal.has_pending_auth())
                        });
                        if let Some(view) = waiting {
                            let batch = drain_ssh_prompt_batch(&view, !modal.read(cx).has_active_prompt(), cx);
                            modal.update(cx, |m, cx| m.push_prompt_batch(batch, window, cx));
                        }
                        if !modal.read(cx).has_active_prompt() {
                            this.handle_intent(
                                crate::ui::workspace::intent::WorkspaceIntent::FocusActiveTerminal,
                                window,
                                cx,
                            );
                        }
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        cx.observe(&ssh_prompt, |_, _, cx| cx.notify()).detach();
        let config_watch = cx.observe_global_in::<Config>(window, |this, window, cx| {
            this.reload_from_config(window, cx)
        });
        cx.default_global::<crate::git::status::GitStatusCache>();
        let git_status_watch =
            cx.observe_global::<crate::git::status::GitStatusCache>(|this, cx| {
                this.maybe_refresh_diff_overlay(cx);
                let scm = this.right_panel.read(cx).scm.clone();
                scm.update(cx, |scm, cx| scm.refresh_from_tab_status(cx));
                cx.notify();
            });
        let this = cx.weak_entity();
        let keystroke_watch = cx.intercept_keystrokes(move |_ev, _window, cx| {
            let _ = this.update(cx, |this, cx| this.dismiss_mod_hint(cx));
        });
        let activation_watch = cx.observe_window_activation(window, |this, window, cx| {
            this.dismiss_mod_hint(cx);
            this.set_link_modifier(false, cx);
            if window.is_window_active() {
                if cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
                    crate::ui::workspace::windows::WindowRegistry::note_focused(
                        cx,
                        window.window_handle(),
                    );
                }
                this.refresh_git_status_all(cx);
            }
        });
        let this = cx.weak_entity();
        let appearance_watch = window.observe_window_appearance(move |window, cx| {
            crate::ui::chrome::theme::note_system_appearance(window, cx);
            if !cx.global::<Config>().theme_follow_system {
                return;
            }
            apply_theme(Some(window), cx);
            let _ = this.update(cx, |_this, cx| cx.notify());
        });
        apply_theme(Some(window), cx);
        set_menus(cx);
        let mut startup_error: Option<gpui::SharedString> = None;
        let (tabs, active) = match session {
            None => match new_terminal(font_size, initial_cwd, None, None, window, cx) {
                Ok(first) => (vec![Tab::new(Pane::leaf(first))], 0),
                Err(e) => {
                    log::error!("first terminal failed to start: {e}");
                    startup_error = Some(gpui::SharedString::from(t_fmt(
                        L10nKey::AppOpenTerminalFailed,
                        &[("error", &e.to_string())],
                    )));
                    (Vec::new(), 0)
                }
            },
            some => {
                let (tabs, active, dropped) = tabs_from_session(some, font_size, window, cx);
                if dropped > 0 {
                    startup_error = Some(gpui::SharedString::from(t_plural(
                        L10nKey::AppTabsNotRestored,
                        dropped,
                        &[],
                    )));
                }
                (tabs, active)
            }
        };
        if !cx.has_global::<crate::terminal::view::ChromeMeta>() {
            cx.set_global(crate::terminal::view::ChromeMeta(0));
        }
        let app_entity = cx.entity();
        let weak_self = cx.weak_entity();
        let sidebar_host = cx.new(|host_cx| {
            host_cx.observe(&app_entity, |_, _, cx| cx.notify()).detach();
            host_cx
                .observe_global::<crate::terminal::view::ChromeMeta>(|_, cx| cx.notify())
                .detach();
            host_cx
                .observe_global::<Config>(|_, cx| cx.notify())
                .detach();
            host_cx
                .observe_global::<crate::git::status::GitStatusCache>(|_, cx| cx.notify())
                .detach();
            host_cx
                .observe_global::<gpui_kit::component::Theme>(|_, cx| cx.notify())
                .detach();
            super::TabSidebarHost {
                app: weak_self.clone(),
            }
        });
        let initial_show_chips = !(matches!(
            cx.global::<Config>().tab_bar_position,
            crate::core::config::TabBarPosition::Left
        ) && !tabs.is_empty());
        let strip_host = cx.new(|host_cx| {
            host_cx.observe(&app_entity, |_, _, cx| cx.notify()).detach();
            host_cx
                .observe_global::<crate::terminal::view::ChromeMeta>(|_, cx| cx.notify())
                .detach();
            host_cx
                .observe_global::<Config>(|_, cx| cx.notify())
                .detach();
            host_cx
                .observe_global::<crate::git::status::GitStatusCache>(|_, cx| cx.notify())
                .detach();
            host_cx
                .observe_global::<gpui_kit::component::Theme>(|_, cx| cx.notify())
                .detach();
            super::TabStripHost {
                app: weak_self,
                show_chips: Rc::new(Cell::new(initial_show_chips)),
            }
        });
        let mut app = Self {
            tabs,
            active,
            tab_use_seq: std::cell::Cell::new(0),
            pending_tab: None,
            font_size,
            applied_terminal_config,
            _subscriptions: vec![
                config_watch,
                keystroke_watch,
                activation_watch,
                git_status_watch,
                appearance_watch,
            ],
            search: None,
            theme_preview_restore: None,
            closed: Vec::new(),
            renaming: None,
            maximized: None,
            mod_hints: Default::default(),
            home_focus: cx.focus_handle(),
            home_cursor_on: true,
            working_dot_on: true,
            shells: ShellInventory::default(),
            right_panel,
            editor,
            sidebar_width: Rc::new(Cell::new(sidebar_width)),
            sidebar_dragging: Rc::new(Cell::new(false)),
            sidebar_chrome_hover: Rc::new(Cell::new(false)),
            strip_chrome_hover: Rc::new(Cell::new(false)),
            sidebar_collapsed,
            sidebar_scroll: gpui_kit::component::VirtualListScrollHandle::new(),
            sidebar_reveal: Rc::new(Cell::new(None)),
            reorder: Rc::new(RefCell::new(None)),
            pane_hover: Rc::new(Cell::new(None)),
            pane_drag: Rc::new(RefCell::new(None)),
            tab_merge: Cell::new(None),
            pane_detach: Cell::new(None),
            strip_slots: Rc::new(RefCell::new(Vec::new())),
            sidebar_slots: Rc::new(RefCell::new(Vec::new())),
            sidebar_tags: Default::default(),
            active_tag_filter: None,
            pane_area: Rc::new(Cell::new(None)),
            settings_window: None,
            ssh_prompt,
            close_prompt_open: false,
            window_bounds: window_bounds_to_remember(window),
            tag_rename: None,
            window_title: std::cell::RefCell::new(String::new()),
            startup_error,
            sidebar_host,
            strip_host,
        };
        if !cfg!(test) && crate::ui::workspace::windows::WindowRegistry::count(cx) == 0 {
            crate::ui::chrome::tray::init(cx);
        }
        app.refresh_shells(cx);
        let app_id = cx.entity_id();
        let editor_id = app.editor.entity_id();
        cx.on_release(move |_, cx| {
            crate::ui::editor::lsp::LspStore::sync_window(editor_id, Vec::new(), cx)
        })
        .detach();
        cx.on_release(move |_, cx| {
            if cx.has_global::<crate::git::data::ScmData>() {
                cx.global_mut::<crate::git::data::ScmData>()
                    .release_window(app_id.as_u64());
            }
        })
        .detach();
        cx.on_app_quit(|app, cx| {
            app.save_session(cx);
            WindowState::from_bounds(app.window_bounds).save();
            async move {}
        })
        .detach();

        cx.observe_window_bounds(window, |this, window, _cx| {
            this.window_bounds = window_bounds_to_remember(window);
        })
        .detach();

        cx.on_focus_lost(window, |this, window, cx| this.focus_active(window, cx))
            .detach();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(HOME_CURSOR_BLINK_MS))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.tabs.is_empty() {
                            this.home_cursor_on = !this.home_cursor_on;
                            cx.notify();
                        }
                        let working = this.tabs.iter().any(|tab| {
                            tab.agent_status(cx)
                                == Some(crate::core::cli_agent::AgentStatus::Working)
                        });
                        if working || !this.working_dot_on {
                            this.working_dot_on = !this.working_dot_on || !working;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let weak_app = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(app) = weak_app.upgrade() else {
                return true;
            };
            let asked = app.update(cx, |app, cx| {
                let unsaved = app.editor_unsaved(cx);
                app.editor_guard_unsaved(
                    unsaved,
                    crate::ui::editor::code_editor::AfterUnsaved::CloseWindow,
                    window,
                    cx,
                )
            });
            if asked {
                return false;
            }
            let handle = window.window_handle();
            app.update(cx, |app, cx| {
                app.prepare_window_close(handle, cx);
                app.lsp_window_closed(cx);
            });
            true
        });

        app.focus_active(window, cx);
        app
    }

    pub(crate) fn save_session(&self, cx: &mut App) {
        WindowState::from_bounds(self.window_bounds).save();
        crate::ui::remote::tree_sync::sync_window(self, cx);
    }

    pub(crate) fn new_window(&self, cx: &mut App) {
        cx.defer(|cx| crate::ui::workspace::windows::open(cx));
    }

    fn prepare_window_close(&self, handle: AnyWindowHandle, cx: &mut App) {
        let last_window = crate::ui::workspace::windows::WindowRegistry::count(cx) <= 1;
        self.save_session(cx);
        crate::ui::workspace::windows::WindowRegistry::unregister(cx, handle);
        crate::ui::workspace::windows::refresh_menu(cx);
        if last_window {
            crate::ui::remote::tree_sync::forget(cx);
            let retire_to_tray =
                cx.global::<Config>().show_tray_icon && crate::ui::chrome::tray::icon_is_up();
            if !retire_to_tray {
                cx.spawn(async move |cx| {
                    let _ = cx.update(|cx| cx.quit());
                })
                .detach();
            }
        }
    }

    pub(super) fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let unsaved = self.editor_unsaved(cx);
        if self.editor_guard_unsaved(
            unsaved,
            crate::ui::editor::code_editor::AfterUnsaved::CloseWindow,
            window,
            cx,
        ) {
            return;
        }
        self.prepare_window_close(window.window_handle(), cx);
        self.lsp_window_closed(cx);
        window.remove_window();
    }

    /// The window close a question about unsaved files was holding up.
    pub(crate) fn close_window_after_unsaved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_window(window, cx);
    }

    pub(crate) fn adopt_session(
        &mut self,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_shells(cx);
        let font_size = self.font_size;
        let (tabs, active, dropped) = tabs_from_session(Some(session), font_size, window, cx);
        if dropped > 0 {
            window.push_notification(
                gpui_kit::component::notification::Notification::warning(t_plural(
                    L10nKey::AppTabsNotRestored,
                    dropped,
                    &[],
                )),
                cx,
            );
        }
        self.tabs = tabs;
        self.active = active;
        self.maximized = None;
        self.wake_active_if_asleep(window, cx);
        self.save_session(cx);
        crate::ui::workspace::windows::refresh_menu(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// ⌘⇧T. Asks the machine first, where it keeps closed tabs.
    pub(super) fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = crate::ui::remote::tree_sync::closed_tabs_link(cx) else {
            self.reopen_closed_here(window, cx);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..REOPEN_SETTLE_POLLS {
                let settled = this
                    .update(cx, |_app, cx| {
                        crate::ui::remote::tree_sync::tree_ops_settled(cx)
                    })
                    .unwrap_or(true);
                if settled {
                    break;
                }
                cx.background_executor().timer(REOPEN_SETTLE_POLL).await;
            }
            let answer = cx
                .background_executor()
                .spawn(async move {
                    client.call(anterm_core::daemon::control::ControlRequest::TabReopen {
                        tab: None,
                    })
                })
                .await;
            let _ = this.update_in(cx, |app, window, cx| {
                use anterm_core::daemon::control::ReplyOk;
                match answer {
                    Ok(ReplyOk::ReopenedTab(Some(reopened))) => {
                        crate::ui::remote::machine_mirror::MachineMirrors::note_reopened(
                            cx,
                            reopened.tab.id,
                        );
                        let st = crate::ui::remote::tree_sync::session_tab_from_reopened(&reopened);
                        if let Err(st) = app.put_back_closed_tab(st, window, cx) {
                            app.remember_closed_here(st);
                        }
                    }
                    Ok(ReplyOk::ReopenedTab(None)) => app.reopen_closed_here(window, cx),
                    Ok(other) => {
                        log::warn!("the machine answered TabReopen with {other:?}");
                        app.reopen_closed_here(window, cx);
                    }
                    Err(e) => {
                        log::warn!("could not ask the machine for a closed tab ({e})");
                        app.reopen_closed_here(window, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn reopen_closed_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.closed.pop() else {
            return;
        };
        if let Err(st) = self.put_back_closed_tab(st, window, cx) {
            self.closed.push(st);
        }
    }

    fn put_back_closed_tab(
        &mut self,
        st: SessionTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), SessionTab> {
        let alive = alive_panes();
        let Some(pane) = session_to_pane(&st.pane, alive.as_ref(), self.font_size, window, cx)
        else {
            window.push_notification(
                gpui_kit::component::notification::Notification::error(t(
                    L10nKey::AppReopenTabFailed,
                )),
                cx,
            );
            return Err(st);
        };
        self.remember_active_pane(window, cx);
        self.maximized = None;
        let insert_at = self.new_tab_insert_at(cx);
        self.tabs.insert(
            insert_at,
            Tab {
                pane,
                name: st.name,
                last_focused: None,
                zoomed: None,
                tags: std::cell::RefCell::new(st.tags),
                tree_id: std::cell::Cell::new(
                    st.tree_id
                        .unwrap_or_else(anterm_core::machine::TabId::new),
                ),
                last_used: std::cell::Cell::new(0),
                focus_origin: Default::default(),
                asleep: None,
            },
        );
        self.active = insert_at;
        self.focus_active(window, cx);
        self.save_session(cx);
        cx.notify();
        Ok(())
    }

    pub(crate) fn owns_leaf(&self, leaf_id: u64) -> bool {
        self.tabs.iter().any(|t| {
            t.pane
                .leaves()
                .iter()
                .any(|l| l.entity_id().as_u64() == leaf_id)
        })
    }

    pub(crate) fn agent_rows(&self, cx: &App) -> Vec<crate::ui::chrome::tray::AgentRow> {
        use crate::core::cli_agent::AgentStatus;
        let now_ms = crate::core::config::unix_now().saturating_mul(1000);
        let mut agents = Vec::new();
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                let view = leaf.read(cx);
                let Some(agent) = view.agent() else { continue };
                let session = view.agent_session();
                let status = session.as_ref().map_or(AgentStatus::Idle, |s| s.status);
                let since_ms = session.as_ref().and_then(|s| s.since_ms);
                let waited = session
                    .as_ref()
                    .filter(|s| s.status == AgentStatus::Waiting)
                    .and_then(|s| s.status_age_secs(now_ms))
                    .map(crate::ui::workspace::home::waiting_for);
                let dir = view
                    .cwd()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                let branch = view.git_status(cx).map(|g| g.branch);
                let detail = match (dir, branch) {
                    (Some(dir), Some(branch)) => format!("{dir} @ {branch}"),
                    (Some(dir), None) => dir,
                    (None, _) => String::new(),
                };
                agents.push(crate::ui::chrome::tray::AgentRow {
                    leaf_id: leaf.entity_id().as_u64(),
                    agent,
                    status,
                    detail,
                    since_ms,
                    unread: view.agent_result_unread(),
                    waited,
                });
            }
        }
        agents
    }

    pub(crate) fn focus_next_agent_needing_attention(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity_id();
        let mut rows = Vec::new();
        let mut seen_self = false;
        for (_, weak) in crate::ui::workspace::windows::WindowRegistry::open_windows(cx) {
            let Some(app) = weak.upgrade() else { continue };
            if app.entity_id() == me {
                seen_self = true;
                rows.extend(self.agent_rows(cx));
            } else {
                rows.extend(app.read(cx).agent_rows(cx));
            }
        }
        if !seen_self {
            rows.extend(self.agent_rows(cx));
        }
        let current = self
            .focused_leaf(window, cx)
            .map(|leaf| leaf.entity_id().as_u64());
        let Some(leaf_id) = crate::ui::chrome::tray::next_needing_attention(&rows, current) else {
            window.push_notification(t(L10nKey::AppNoAgentNeedsAttention), cx);
            return;
        };
        if self.owns_leaf(leaf_id) {
            self.handle_tray_action(
                crate::ui::chrome::tray::TrayAction::RevealPane { leaf_id },
                window,
                cx,
            );
        } else {
            cx.defer(move |cx| crate::ui::chrome::tray::reveal_pane(leaf_id, cx));
        }
    }

    pub(crate) fn handle_tray_action(
        &mut self,
        action: crate::ui::chrome::tray::TrayAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::chrome::tray::TrayAction;
        fn surface_window(window: &mut Window, cx: &mut App) {
            cx.activate(true);
            window.activate_window();
        }
        match action {
            TrayAction::ShowWindow => surface_window(window, cx),
            TrayAction::RevealPane { leaf_id } => {
                let tab_ix = self.tabs.iter().position(|t| {
                    t.pane
                        .leaves()
                        .iter()
                        .any(|l| l.entity_id().as_u64() == leaf_id)
                });
                if let Some(ix) = tab_ix {
                    self.activate(ix, window, cx);
                    if self
                        .maximized
                        .as_ref()
                        .is_some_and(|m| m.entity_id().as_u64() != leaf_id)
                    {
                        self.maximized = None;
                    }
                    if let Some(leaf) = self.tabs[ix]
                        .pane
                        .leaves()
                        .into_iter()
                        .find(|l| l.entity_id().as_u64() == leaf_id)
                    {
                        self.tabs[ix].last_focused = Some(leaf.entity_id());
                        self.focus_leaf(&leaf, window, cx);
                    }
                    cx.notify();
                }
                surface_window(window, cx);
            }
            TrayAction::SetNotifyMode(mode) => self.set_notify_mode(mode, cx),
            TrayAction::OpenSettings => {
                surface_window(window, cx);
                self.toggle_settings(window, cx);
            }
            TrayAction::CheckForUpdates => {
                surface_window(window, cx);
                self.check_for_updates_now(window, cx);
            }
            TrayAction::Quit => self.quit_stop_sessions(window, cx),
        }
    }

    pub(super) fn quit_stop_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.activate(true);
        window.activate_window();
        let unsaved = self.editor_unsaved(cx);
        if self.editor_guard_unsaved(
            unsaved,
            crate::ui::editor::code_editor::AfterUnsaved::Quit,
            window,
            cx,
        ) {
            return;
        }
        let me = cx.entity_id();
        let elsewhere = crate::ui::workspace::windows::WindowRegistry::open_windows(cx)
            .into_iter()
            .filter_map(|(handle, app)| Some((handle, app.upgrade()?)))
            .find(|(_, app)| app.entity_id() != me && !app.read(cx).editor_unsaved(cx).is_empty());
        if let Some((handle, app)) = elsewhere {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    app.update(cx, |app, cx| app.quit_stop_sessions(window, cx));
                });
            });
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            t(anterm_i18n::L10nKey::QuitStopServerTitle),
            Some(t(anterm_i18n::L10nKey::QuitStopServerBody)),
            &crate::ui::confirm_answers(
                t(anterm_i18n::L10nKey::QuitAndStop),
                t(anterm_i18n::L10nKey::Cancel),
            ),
            cx,
        );
        cx.spawn(async move |_this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            cx.background_spawn(async { crate::daemon::spawn::stop() })
                .await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    }

    /// The quit a question about unsaved files was holding up.
    pub(crate) fn quit_after_unsaved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quit_stop_sessions(window, cx);
    }

    pub(crate) fn restart_window_daemon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restart_daemon(window, cx);
    }

    pub(super) fn update_local_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::daemon::spawn::local_daemon_is_this_build() {
            window.push_notification(
                t_fmt(
                    L10nKey::AppLocalServerAlreadyCurrent,
                    &[("build", env!("CARGO_PKG_VERSION"))],
                ),
                cx,
            );
            return;
        }
        self.restart_daemon(window, cx);
    }

    pub(crate) fn restart_daemon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let in_place =
            crate::daemon::spawn::local_daemon_supports(crate::daemon::protocol::FEATURE_HANDOFF);
        let answer = window.prompt(
            PromptLevel::Warning,
            t(L10nKey::AppRestartServerTitle),
            Some(t(if in_place {
                L10nKey::AppRestartServerBodyInPlace
            } else {
                L10nKey::AppRestartServerBody
            })),
            &crate::ui::confirm_answers(
                t(L10nKey::AppRestart),
                t(anterm_i18n::L10nKey::Cancel),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = this.update_in(cx, |this, _window, cx| this.restart_daemon_confirmed(cx));
        })
        .detach();
    }

    fn restart_daemon_confirmed(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if this
                .update_in(cx, |this, _window, cx| {
                    this.save_session(cx);
                    this.maximized = None;
                    this.tabs.clear();
                    this.active = 0;
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            let restarted = cx
                .background_spawn(async move {
                    if crate::daemon::spawn::local_daemon_supports(
                        crate::daemon::protocol::FEATURE_HANDOFF,
                    ) {
                        crate::daemon::spawn::hand_off()
                    } else {
                        crate::daemon::spawn::restart()
                    }
                })
                .await;
            Self::settle_after_restart(this, restarted, cx).await;
        })
        .detach();
    }

    pub(super) async fn settle_after_restart(
        this: gpui::WeakEntity<Self>,
        restarted: anyhow::Result<()>,
        cx: &mut gpui::AsyncApp,
    ) {
        if this
            .update_in(cx, |this, window, cx| {
                if let Err(e) = &restarted {
                    log::error!("restart background service failed, resyncing from the tree: {e}");
                    let text = t_fmt(
                        L10nKey::AppRestartServerFailed,
                        &[("error", &e.to_string())],
                    );
                    this.startup_error = Some(gpui::SharedString::from(text.clone()));
                    window.push_notification(text, cx);
                }
            })
            .is_err()
        {
            return;
        }

        let _ = cx.update(crate::ui::remote::tree_sync::resync_after_local_daemon_change);

        let _ = this.update_in(cx, |this, window, cx| {
            this.focus_active(window, cx);
            cx.notify();
        });
    }
}

pub(super) fn drain_ssh_prompt_batch(
    view: &gpui::Entity<crate::terminal::view::TerminalView>,
    want_prompt: bool,
    cx: &App,
) -> crate::ui::remote::ssh_prompt::SshPromptBatch {
    use crate::daemon::protocol::AuthPromptKind;
    use crate::ui::remote::ssh_prompt::{PromptEndpoint, SshPromptBatch};

    let v = view.read(cx);
    let pane_id = v.pane_id;
    let term = &v.terminal;
    let mut banners = Vec::new();
    let mut next: Option<(u64, AuthPromptKind)> = None;
    loop {
        if want_prompt {
            match term.take_auth_prompt() {
                Some((_, AuthPromptKind::Banner { text })) => banners.push(text),
                Some(p) => {
                    next = Some(p);
                    break;
                }
                None => break,
            }
        } else {
            match term.take_auth_banner() {
                Some(text) => banners.push(text),
                None => break,
            }
        }
    }
    let endpoint = term.ssh_endpoint().map(|(host, port)| PromptEndpoint {
        user: term.ssh_user().unwrap_or_default(),
        host,
        port,
    });
    SshPromptBatch {
        pane_id,
        endpoint,
        auto_supplied: term.auto_supplied_password(),
        phase: term.ssh_phase(),
        banners,
        next,
    }
}
