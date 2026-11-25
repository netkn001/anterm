use gpui::{
    App, Axis, Bounds, Context, Entity, Pixels, PromptLevel, Window, div, point, prelude::*, px,
    size,
};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};

use crate::core::config::{Config, NewTabPosition, TabBarPosition};
use crate::core::session::SessionTab;
use crate::daemon::protocol::{RemoteKind, ShellSpec};
use crate::terminal::view::TerminalView;
use crate::ui::chrome::settings::SettingsSection;
use crate::ui::workspace::pane::{CloseOutcome, Dir, Pane, PaneSlot};
use anterm_i18n::{L10nKey, t, t_fmt};

use super::session_spawn::{
    agent_resume_command, build_terminal_view, kill_pane_off_thread, new_terminal,
    new_terminal_in_bounds, new_terminal_native, new_terminal_native_in_bounds, one_slot_move,
    pane_to_session, redial_native_ssh, remember_leaf_in, replace_leaf_in, session_to_pane,
    start_pane_spawn, step_in_order, tab_to_session,
};
use super::{
    AntermApp, Asleep, CloseReason, CloseTarget, ForkPlacement, FullscreenHint, MAX_CLOSED_TABS,
    OpenedFrom, RESIZE_STEP, Rename, Renaming, SpawnAs, SpawnWhere, Tab, TabAgentSession,
    close_prompt, close_reason, inherited_start_dir, rename_outcome, strip_band,
    window_controls_drawn,
};

impl AntermApp {
    pub(crate) fn sync_window_title(&self, window: &mut Window, cx: &App) {
        let title = self
            .tabs
            .get(self.active)
            .and_then(|tab| self.full_tab_label(tab, Some(window), cx))
            .unwrap_or_else(|| "anterm".to_string());
        if *self.window_title.borrow() == title {
            return;
        }
        window.set_window_title(&title);
        *self.window_title.borrow_mut() = title;
    }

    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut App) {
        self.sync_window_title(window, cx);
        // Only where the page is drawn: with settings in a window of its own,
        // its handle focused in the workspace window is focus on nothing
        // there, and every shortcut in the workspace goes dead.
        let Some(tab) = self.tabs.get(self.active) else {
            window.focus(&self.home_focus, cx);
            return;
        };
        if let Some(overlay) = self.editor.read(cx).active_diff_overlay() {
            let handle = overlay.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
            return;
        }
        if let Some(leaf) = tab.focus_target() {
            let handle = leaf.focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// Sample which pane holds focus right now and record it against the
    /// active tab.
    ///
    /// A sample only ever writes the truth, but it can only write it when
    /// there is one to read: with focus one handle off the panes it finds no
    /// leaf and leaves the field alone. That is why it is no longer the only
    /// writer (see [`AntermApp::remember_focused_leaf`]) — it stays because the
    /// callers below want the answer settled at a named moment, before a pane
    /// is detached or a tab is torn down and the layout stops being able to
    /// answer at all.
    pub(crate) fn remember_active_pane(&mut self, window: &Window, cx: &App) {
        let active = self.active;
        if let Some(tab) = self.tabs.get_mut(active) {
            if let Some(leaf) = tab.pane.focused_leaf(window, cx) {
                tab.last_focused = Some(leaf.entity_id());
            }
        }
    }

    /// Record `leaf` as the pane its tab comes back to, as focus arrives in it.
    ///
    /// Sampling at switch time asks which leaf holds focus *at that instant*,
    /// and by then focus is routinely somewhere else: the switcher's own
    /// search input, a palette that just closed, the tab strip, a pane
    /// restored and never clicked. The sample then wrote nothing and the tab
    /// kept a stale pane — or the `None` it was born with — and came back to
    /// its first leaf instead of the one the reader was working in (#843).
    ///
    /// Focus-in is the one moment that knows the answer without having to
    /// guess when to look, so it is the primary writer now. The tab is found
    /// by the leaf rather than assumed to be the active one: a pane dragged
    /// into another tab is focused after the move, and it is the tab holding
    /// it now that has to remember it.
    pub(crate) fn remember_focused_leaf(&mut self, leaf: gpui::EntityId) {
        remember_leaf_in(&mut self.tabs, leaf);
    }

    /// Toggle fullscreen, and say how to leave it on the way in.
    ///
    /// Only on the way in, and only from the action: entering is an instant in
    /// which the window buttons disappear, and a window that starts fullscreen
    /// because the setting says so is not a surprise anybody needs explaining.
    /// The chord comes from the keymap rather than from a string, because it is
    /// `F11` on Windows and Linux, `Cmd+Enter` on macOS, and either of them may
    /// have been rebound.
    ///
    /// The hint carries an id of its own, which is what keeps a held-down
    /// `F11` to one notice rather than a column of identical ones: pushing
    /// under an id already on screen replaces that one. Leaving through the
    /// action takes it back too, so a quick in-and-out does not leave the way
    /// out on screen after it has been taken. Leaving some other way — a
    /// window manager with a chord of its own — just lets it time out, which
    /// is a second or two of a stale notice and not worth watching every
    /// frame for.
    pub(super) fn toggle_fullscreen(&self, window: &mut Window, cx: &mut App) {
        let entering = !window.is_fullscreen();
        window.toggle_fullscreen();
        window.remove_notification::<FullscreenHint>(cx);
        // Nothing disappeared where there were no buttons to begin with, so
        // there is nothing to explain.
        if !entering || !window_controls_drawn(false) {
            return;
        }
        let hint = match crate::ui::workspace::home::key_hint("ToggleFullscreen", cx) {
            Some(chord) => t_fmt(L10nKey::AppFullscreenEntered, &[("key", &chord)]),
            // Rebound to nothing at all: still worth saying the buttons are gone,
            // just without naming a key that would not work.
            None => t(L10nKey::AppFullscreenEnteredNoKey).to_string(),
        };
        window.push_notification(
            gpui_kit::component::notification::Notification::new()
                .id::<FullscreenHint>()
                .message(hint),
            cx,
        );
    }

    pub(super) fn focus_leaf(&self, leaf: &PaneSlot, window: &mut Window, cx: &mut App) {
        let handle = leaf.focus_handle(cx);
        window.focus(&handle, cx);
    }

    #[allow(dead_code)]
    pub(super) fn land_pane(
        &mut self,
        slot_id: gpui::EntityId,
        pending: &Entity<crate::ui::workspace::pending_pane::PendingPane>,
        parts: Result<crate::terminal::view::ShellParts, String>,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parts = match parts {
            Ok(parts) => parts,
            Err(reason) => {
                let retry = pending.update(cx, |p, cx| {
                    p.fail(reason, cx);
                    p.next_auto_retry()
                });
                if let Some(delay) = retry {
                    let pending = pending.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        let _ = this.update_in(cx, |app, window, cx| {
                            let still_there = app.tabs.iter().any(|tab| {
                                tab.pane.leaves().iter().any(|l| l.entity_id() == slot_id)
                            });
                            // Try Again got there first, or the tab is gone.
                            if !still_there || !pending.read(cx).is_failed() {
                                return;
                            }
                            pending.update(cx, |p, cx| p.retrying(cx));
                            start_pane_spawn(pending.clone(), window, cx);
                        });
                    })
                    .detach();
                }
                return;
            }
        };
        let still_there = self
            .tabs
            .iter()
            .any(|tab| tab.pane.leaves().iter().any(|l| l.entity_id() == slot_id));
        if !still_there {
            log::info!(
                "pane {} arrived after its slot closed; killing it",
                parts.pane_id
            );
            kill_pane_off_thread(crate::terminal::PaneRoute::Local, parts.pane_id, cx);
            return;
        }
        let was_focused = pending.read(cx).focus_handle.contains_focused(window, cx);
        let restored = parts.restored;
        let view = build_terminal_view(parts, font_size, window, cx);
        let resume = (!restored)
            .then(|| {
                let spawn = &pending.read(cx).spawn;
                agent_resume_command(
                    &spawn.agent,
                    spawn.agent_session_id.as_deref(),
                    spawn.agent_launch_argv.as_deref(),
                    view.read(cx),
                    cx,
                )
                .or_else(|| spawn.run_on_land.clone())
            })
            .flatten();
        if let Some(cmd) = resume {
            view.read(cx).run_command_line(&cmd);
        }
        let slot = PaneSlot::Ready(view.clone());
        replace_leaf_in(&mut self.tabs, slot_id, slot.clone());
        if was_focused {
            self.focus_leaf(&slot, window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn refresh_git_status_all(&mut self, cx: &mut Context<Self>) {
        for leaf in self.tabs.iter().flat_map(|tab| tab.pane.terminals()) {
            leaf.update(cx, |view, cx| view.refresh_git_status_now(cx));
        }
    }

    pub(super) fn new_tab_insert_at(&self, cx: &App) -> usize {
        match cx.global::<Config>().new_tab_position {
            NewTabPosition::AfterCurrent => (self.active + 1).min(self.tabs.len()),
            NewTabPosition::End => self.tabs.len(),
        }
    }

    /// ⌘T. From an SSH pane it dials the same host again, the way ⌘D does:
    /// a local shell opened from one would land in a directory the pane never
    /// showed, and in Ungrouped rather than under the host the user was on.
    /// The new session starts in the far directory the pane was in, as a
    /// local ⌘T starts in the local one.
    pub(crate) fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))
            .map(|view| {
                let view = view.read(cx);
                let remote = view
                    .remote_context()
                    .filter(|r| matches!(r.kind, RemoteKind::Ssh | RemoteKind::NativeSsh));
                (view.ssh_spec(), remote, view.cwd())
            });
        match source {
            Some((Some(spec), _remote, cwd)) => {
                let start_dir = inherited_start_dir(OpenedFrom::SameSshHost, cwd, true);
                let spec = crate::ui::remote::ssh_connect::resolve_persisted_ssh_spec(spec, cx);
                self.open_native_ssh_tab(spec, start_dir, window, cx);
            }
            // A shell that ssh'd onward from a local prompt: the far side is
            // only reachable by typing the same command again, from the
            // directory it was typed in.
            Some((None, Some(remote), cwd)) if remote.kind == RemoteKind::Ssh => {
                let line = crate::git::data::shell_quote(&remote.argv);
                if let Some(slot) = self.new_tab_slot(cwd, None, window, cx) {
                    crate::ui::workspace::agent_launch::run_when_ready(&slot, line, cx);
                }
            }
            _ => self.new_tab_with_shell(None, window, cx),
        }
    }

    pub(crate) fn new_tab_at(
        &mut self,
        cwd: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab_with_cwd(Some(cwd), None, window, cx);
    }

    /// Opens a new local shell in `cwd`, then types a command only after the
    /// pane exists. LaunchServices uses this for a script/executable selected
    /// in Finder and for `x-man-page:` requests.
    ///
    /// A tab that did not open takes the command with it. `new_tab_with_cwd`
    /// returns early when the spawn fails or the workspace cannot host a local
    /// shell, and writing anyway would type the command into whatever pane was
    /// focused before — a shell the user is mid-line in, or an agent.
    pub(crate) fn new_tab_running(
        &mut self,
        cwd: std::path::PathBuf,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.tabs.len();
        self.new_tab_with_cwd(Some(cwd), None, window, cx);
        if self.tabs.len() == before {
            log::warn!("no tab opened for requested command; not writing it to another pane");
            return;
        }
        if let Some(terminal) = self.focused_leaf(window, cx) {
            terminal.read(cx).run_command_line(&command);
        }
    }

    /// Uses the terminal that a newly-created window already opened. This keeps
    /// a cold LaunchServices request to one tab rather than creating the
    /// window's default shell and then a second shell for the requested item.
    pub(crate) fn run_in_active_terminal(&self, command: &str, window: &Window, cx: &App) {
        if let Some(terminal) = self.focused_leaf(window, cx) {
            terminal.read(cx).run_command_line(command);
        }
    }

    pub(crate) fn new_tab_with_shell(
        &mut self,
        shell: Option<ShellSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.tabs.get(self.active).and_then(|t| {
            t.pane
                .focused_or_first(window, cx)
                .and_then(|leaf| leaf.read(cx).spawnable_cwd())
        });
        self.new_tab_with_cwd(cwd, shell, window, cx);
    }

    /// A shell taken out of the new-tab menu, wherever the ⌥ key said to put
    /// it.
    pub(crate) fn open_shell(
        &mut self,
        shell: Option<ShellSpec>,
        at: SpawnWhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match at {
            SpawnWhere::NewTab => self.new_tab_with_shell(shell, window, cx),
            SpawnWhere::Split => {
                self.split_into(Axis::Horizontal, Some(SpawnAs::Shell(shell)), window, cx)
            }
        }
    }

    fn new_tab_with_cwd(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        shell: Option<ShellSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab_slot(cwd, shell, window, cx);
    }

    /// [`Self::new_tab_with_cwd`], answering with the pane it opened — `None`
    /// when nothing opened, so a caller about to type into the new pane has
    /// nothing to type into rather than whatever was focused before.
    pub(crate) fn new_tab_slot(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        shell: Option<ShellSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneSlot> {
        let bounds = self.pane_area.get().map(|b| b.size);
        let tab = match new_terminal_in_bounds(self.font_size, cwd, None, shell, bounds, window, cx)
        {
            Ok(view) => view,
            Err(e) => {
                log::error!("new tab spawn failed: {e}");
                let text = t_fmt(L10nKey::AppOpenTerminalFailed, &[("error", &e.to_string())]);
                // A retry from the home screen fails the same way; keep the
                // reason on screen rather than only in a toast that leaves.
                self.startup_error = Some(gpui::SharedString::from(text.clone()));
                window.push_notification(crate::ui::remote::host_ops::failure(text, &e), cx);
                cx.notify();
                return None;
            }
        };
        // Something opened, so whatever the last failure was is stale.
        self.startup_error = None;
        self.remember_active_pane(window, cx);
        self.maximized = None;
        let insert_at = self.new_tab_insert_at(cx);
        let new_tab = Tab::new(Pane::leaf(tab.clone()));
        self.tabs.insert(insert_at, new_tab);
        self.active = insert_at;
        self.focus_active(window, cx);
        self.save_session(cx);
        cx.notify();
        Some(tab)
    }

    /// A tab dialling `spec`, whose shell starts in `remote_start_dir` on the
    /// far host. A connection opened from a saved host or quick connect
    /// passes `None`: the pane in front belongs to some other machine (usually
    /// this one), and its directory means nothing over there.
    pub(crate) fn open_native_ssh_tab(
        &mut self,
        spec: Box<crate::daemon::protocol::NativeSshSpec>,
        remote_start_dir: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.pane_area.get().map(|b| b.size);
        let view = match new_terminal_native_in_bounds(
            self.font_size,
            remote_start_dir,
            spec,
            bounds,
            window,
            cx,
        ) {
            Ok(view) => view,
            Err(e) => {
                log::error!("native SSH spawn failed: {e}");
                window.push_notification(
                    crate::ui::remote::host_ops::failure(
                        t_fmt(
                            L10nKey::AppSshConnectionFailed,
                            &[("error", &e.to_string())],
                        ),
                        &e,
                    ),
                    cx,
                );
                return;
            }
        };
        self.remember_active_pane(window, cx);
        self.maximized = None;
        let insert_at = self.new_tab_insert_at(cx);
        self.tabs
            .insert(insert_at, Tab::new(Pane::leaf(PaneSlot::Ready(view))));
        self.active = insert_at;
        self.focus_active(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn respawn_native_ssh_in_place(
        &mut self,
        dead: &Entity<TerminalView>,
        spec: Box<crate::daemon::protocol::NativeSshSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The far shell's last report, not a local path: a native SSH pane's
        // cwd only ever comes from OSC 7 sent across the link.
        let remote_start_dir = dead.read(cx).cwd();
        let fresh = match new_terminal_native(self.font_size, remote_start_dir, spec, window, cx) {
            Ok(view) => view,
            Err(e) => {
                log::error!("native SSH respawn failed: {e}");
                window.push_notification(
                    crate::ui::remote::host_ops::failure(
                        t_fmt(L10nKey::AppSshReconnectFailed, &[("error", &e.to_string())]),
                        &e,
                    ),
                    cx,
                );
                return;
            }
        };
        replace_leaf_in(
            &mut self.tabs,
            dead.entity_id(),
            PaneSlot::Ready(fresh.clone()),
        );
        self.maximized = None;
        self.focus_leaf(&PaneSlot::Ready(fresh), window, cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Split the focused pane along `axis`, placing the new pane before
    /// (left / up) when `before` is true, or after (right / down) when false.
    pub(crate) fn split_in(
        &mut self,
        axis: Axis,
        before: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_slot_in(axis, before, None, window, cx);
    }

    /// `spawn` of `None` is what ⌘D has always done: copy the pane being split
    /// — same shell, or the same host dialled again. A `Some` names the new
    /// pane outright, which is how a row taken out of the new-tab menu with ⌥
    /// held lands beside the current pane instead of in a tab of its own.
    pub(crate) fn split_into(
        &mut self,
        axis: Axis,
        spawn: Option<SpawnAs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_slot_in(axis, false, spawn, window, cx);
    }

    /// [`Self::split_into`], answering with the pane it placed, or `None`
    /// when nothing was placed.
    pub(crate) fn split_slot(
        &mut self,
        axis: Axis,
        spawn: Option<SpawnAs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneSlot> {
        self.split_slot_in(axis, false, spawn, window, cx)
    }

    fn split_slot_in(
        &mut self,
        axis: Axis,
        before: bool,
        spawn: Option<SpawnAs>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneSlot> {
        let target = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))?;
        let (spawn, opened) = match spawn {
            Some(spawn) => {
                let opened = match spawn {
                    SpawnAs::Ssh(_) => OpenedFrom::FreshSshHost,
                    SpawnAs::Shell(_) => OpenedFrom::LocalShell,
                };
                (spawn, opened)
            }
            // A stored spec is resolved against the saved host before it is
            // dialled; one handed in by a caller was just built from that host
            // and needs no second pass.
            None => match target.read(cx).ssh_spec() {
                Some(spec) => (
                    SpawnAs::Ssh(crate::ui::remote::ssh_connect::resolve_persisted_ssh_spec(spec, cx)),
                    OpenedFrom::SameSshHost,
                ),
                None => (
                    SpawnAs::Shell(target.read(cx).shell_spec()),
                    OpenedFrom::LocalShell,
                ),
            },
        };
        let cwd = {
            let target = target.read(cx);
            inherited_start_dir(opened, target.cwd(), target.remote_context().is_some())
        };
        let split_bounds = self.pane_area.get().and_then(|area| {
            let tab = self.tabs.get(self.active)?;
            let (_, rect) = tab
                .pane
                .leaf_rects()
                .into_iter()
                .find(|(slot, _)| slot.entity_id() == target.entity_id())?;
            let w = area.size.width.as_f32() * rect.w;
            let h = area.size.height.as_f32() * rect.h;
            const SPLIT_DIVIDER_PX: f32 = 5.;
            Some(match axis {
                Axis::Horizontal => size(px(((w - SPLIT_DIVIDER_PX) * 0.5).max(0.)), px(h)),
                Axis::Vertical => size(px(w), px(((h - SPLIT_DIVIDER_PX) * 0.5).max(0.))),
            })
        });
        let new = match spawn {
            SpawnAs::Ssh(spec) => {
                match new_terminal_native_in_bounds(
                    self.font_size,
                    cwd,
                    spec,
                    split_bounds,
                    window,
                    cx,
                ) {
                    Ok(view) => PaneSlot::Ready(view),
                    Err(e) => {
                        log::error!("native SSH split spawn failed: {e}");
                        window.push_notification(
                            crate::ui::remote::host_ops::failure(
                                t_fmt(
                                    L10nKey::AppSshConnectionFailed,
                                    &[("error", &e.to_string())],
                                ),
                                &e,
                            ),
                            cx,
                        );
                        return None;
                    }
                }
            }
            SpawnAs::Shell(shell) => {
                match new_terminal_in_bounds(
                    self.font_size,
                    cwd,
                    None,
                    shell,
                    split_bounds,
                    window,
                    cx,
                ) {
                    Ok(view) => view,
                    Err(e) => {
                        log::error!("split spawn failed: {e}");
                        window.push_notification(
                            crate::ui::remote::host_ops::failure(
                                t_fmt(L10nKey::AppSplitPaneFailed, &[("error", &e.to_string())]),
                                &e,
                            ),
                            cx,
                        );
                        return None;
                    }
                }
            }
        };
        let tab = self.tabs.get_mut(self.active)?;
        if !tab
            .pane
            .split_leaf(target.entity_id(), axis, before, new.clone())
        {
            return None;
        }
        self.maximized = None;
        self.focus_leaf(&new, window, cx);
        self.save_session(cx);
        cx.notify();
        Some(new)
    }

    pub(super) fn close_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_pane_inner(false, window, cx);
    }

    /// `confirmed` is set only by the answer to this pane's own close question,
    /// so it travels with the close rather than being read back off shared
    /// state a second, unrelated close could have overwritten.
    fn close_pane_inner(&mut self, confirmed: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Closing the last pane closes the tab, and with it the files only
        // this tab's editor shows.
        if !confirmed
            && self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.pane.leaves().len() <= 1)
        {
            let unsaved = self.editor_unsaved_in_tab(self.active, cx);
            if self.editor_guard_unsaved(
                unsaved,
                crate::ui::editor::code_editor::AfterUnsaved::ClosePane,
                window,
                cx,
            ) {
                return;
            }
        }
        if !confirmed && let Some(reason) = self.focused_pane_close_reason(window, cx) {
            self.ask_before_closing(CloseTarget::Pane, reason, window, cx);
            return;
        }
        self.maximized = None;
        let focused = self.tabs.get(self.active).and_then(|tab| {
            tab.pane
                .leaves()
                .into_iter()
                .find(|l| l.contains_focused(window, cx))
                .and_then(|l| l.terminal().cloned())
        });
        let outcome = match self.tabs.get_mut(self.active) {
            Some(tab) => tab.pane.close_focused(window, cx),
            None => return,
        };
        match outcome {
            // The last pane takes its tab with it. The question was already
            // asked about this very pane, so carry the answer across rather
            // than letting the tab re-derive the same reason and ask again.
            CloseOutcome::RemoveSelf => {
                self.close_tab_inner(self.active, confirmed, window, cx);
            }
            CloseOutcome::NotFound => {
                let single = self
                    .tabs
                    .get(self.active)
                    .is_some_and(|tab| tab.pane.leaves().len() <= 1);
                if single {
                    self.close_tab_inner(self.active, confirmed, window, cx);
                }
            }
            CloseOutcome::Collapsed => {
                if let Some(leaf) = &focused {
                    kill_pane_off_thread(leaf.read(cx).pane_route(), leaf.read(cx).pane_id, cx);
                }
                self.focus_active(window, cx);
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    /// The pane close a question about unsaved files was holding up.
    pub(crate) fn close_pane_after_unsaved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_pane_inner(false, window, cx);
    }

    pub(super) fn on_child_exited(
        &mut self,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = view.entity_id();
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.pane.leaves().iter().any(|l| l.entity_id() == id))
        else {
            return;
        };
        if view.read(cx).ssh_disconnected() {
            cx.notify();
            return;
        }
        match self.tabs[index].pane.close_leaf(view.entity_id()) {
            // Not the asking close: the pane is already gone, so there is no
            // tab left to keep open if the answer were Cancel. Unsaved editor
            // files it had are handed to the tab in front instead (see
            // `editor_sync`).
            CloseOutcome::RemoveSelf => self.close_tab_inner(index, true, window, cx),
            CloseOutcome::NotFound => {}
            CloseOutcome::Collapsed => {
                kill_pane_off_thread(view.read(cx).pane_route(), view.read(cx).pane_id, cx);
                if index == self.active {
                    self.maximized = None;
                    self.focus_active(window, cx);
                }
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn cycle_pane(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaves = match self.tabs.get(self.active) {
            Some(tab) => tab.pane.leaves(),
            None => return,
        };
        if leaves.len() < 2 {
            return;
        }
        self.maximized = None;
        let current = leaves
            .iter()
            .position(|l| l.contains_focused(window, cx))
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % leaves.len()
        } else {
            (current + leaves.len() - 1) % leaves.len()
        };
        let leaf = leaves[next].clone();
        self.focus_leaf(&leaf, window, cx);
        cx.notify();
    }

    pub(super) fn focus_pane_dir(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let Some(from) = tab.pane.focused_leaf(window, cx) else {
            return;
        };
        let back = tab.focus_origin(from.entity_id(), dir);
        let Some(target) = tab.pane.neighbor_in_dir(dir, back, window, cx) else {
            return;
        };
        let live: Vec<gpui::EntityId> = tab.pane.leaves().iter().map(|l| l.entity_id()).collect();
        let (from, to) = (from.entity_id(), target.entity_id());
        let active = self.active;
        if let Some(tab) = self.tabs.get_mut(active) {
            tab.remember_focus_origin(from, to, dir, &live);
        }
        self.maximized = None;
        self.focus_leaf(&target, window, cx);
        cx.notify();
    }

    pub(super) fn resize_pane(&mut self, dir: Dir, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.pane.resize_focused_pane(dir, RESIZE_STEP, window, cx));
        if changed {
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(super) fn swap_pane(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (from, len) = match self.tabs.get(self.active) {
            Some(tab) => (tab.pane.focused_index(window, cx), tab.pane.leaves().len()),
            None => return,
        };
        if len < 2 {
            return;
        }
        let from = from.unwrap_or(0);
        let to = if forward {
            (from + 1) % len
        } else {
            (from + len - 1) % len
        };
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.pane.swap_leaf_indices(from, to) {
                self.maximized = None;
                self.save_session(cx);
                cx.notify();
            }
        }
    }

    /// The patch of the layout a pane being dragged would land on, lit up.
    ///
    /// Also records that landing as the one a drop would take, so the drop and
    /// the highlight can never disagree: a zone the tree refuses to carry out
    /// is neither drawn nor remembered, and releasing over it does nothing.
    pub(super) fn pane_landing(&self, window: &Window, cx: &App) -> Option<gpui::AnyElement> {
        use crate::ui::workspace::pane_drag;

        let from = pane_drag::lifted(&self.pane_drag)?;
        let area = self.pane_area.get()?;
        let tab = self.tabs.get(self.active)?;
        let leaves = tab.pane.leaves();
        let slot = leaves.iter().find(|l| l.entity_id() == from)?;
        let bounds = pane_drag::leaf_bounds(&tab.pane, area);
        let zone = pane_drag::zone_at(area, &bounds, window.mouse_position())?;
        // The zone comes back naming its target by position, which only means
        // anything against this frame's leaves. Drawn against the panes here,
        // and remembered as the panes so the drop that reads it back a frame
        // later is looking for the same ones.
        let here = zone.map(|i| leaves.get(i).cloned())?;
        let pinned = zone.map(|i| leaves.get(i).map(|l| l.entity_id()))?;
        let rect = pane_drag::landing(&tab.pane, slot, here, area)?;
        pane_drag::set_landing(&self.pane_drag, pinned);

        let accent = cx.theme().drag_border;
        Some(
            div()
                .absolute()
                .left(rect.origin.x - area.origin.x)
                .top(rect.origin.y - area.origin.y)
                .w(rect.size.width)
                .h(rect.size.height)
                .rounded_md()
                .border_2()
                .border_color(accent)
                .bg(accent.opacity(0.15))
                .into_any_element(),
        )
    }

    /// The patch of the layout a tab held over it would be grafted into, lit
    /// up — and, while that is on offer, the strip held still underneath it.
    ///
    /// A tab is dragged with the same grip that reorders it, so the two
    /// readings share one gesture: over the strip or the sidebar it is a
    /// reorder, out over the panes it is a merge. Suspending the reorder is
    /// what keeps the drop from being both.
    pub(super) fn tab_landing(&self, window: &Window, cx: &App) -> Option<gpui::AnyElement> {
        self.tab_merge.set(None);
        let dragged = crate::ui::workspace::reorder::dragged_tab(&self.reorder);
        let offer = dragged.and_then(|id| self.tab_landing_rect(id, window));
        crate::ui::workspace::reorder::suspend(&self.reorder, offer.is_some());
        let (zone, rect) = offer?;
        let area = self.pane_area.get()?;
        self.tab_merge.set(Some((dragged?, zone)));

        let accent = cx.theme().drag_border;
        Some(
            div()
                .absolute()
                .left(rect.origin.x - area.origin.x)
                .top(rect.origin.y - area.origin.y)
                .w(rect.size.width)
                .h(rect.size.height)
                .rounded_md()
                .border_2()
                .border_color(accent)
                .bg(accent.opacity(0.15))
                .into_any_element(),
        )
    }

    /// Where the tab `id` would land in the tab on screen, if it can land there
    /// at all — it cannot land in itself, and there is nothing to land in while
    /// a pane is zoomed over the layout.
    fn tab_landing_rect(
        &self,
        id: anterm_core::machine::TabId,
        window: &Window,
    ) -> Option<(
        crate::ui::workspace::pane_drag::DropZone<gpui::EntityId>,
        Bounds<Pixels>,
    )> {
        use crate::ui::workspace::pane_drag;

        if self.maximized.is_some() {
            return None;
        }
        let area = self.pane_area.get()?;
        let host = self.tabs.get(self.active)?;
        if host.tree_id.get() == id {
            return None;
        }
        let sub = &self.tabs.iter().find(|t| t.tree_id.get() == id)?.pane;
        let leaves = host.pane.leaves();
        let bounds = pane_drag::leaf_bounds(&host.pane, area);
        let zone = pane_drag::tab_zone_at(area, &bounds, window.mouse_position())?;
        // Named by pane rather than by position for the drop a frame later,
        // the same way a pane drag's landing is.
        let here = zone.map(|i| leaves.get(i).cloned())?;
        let pinned = zone.map(|i| leaves.get(i).map(|l| l.entity_id()))?;
        let rect = pane_drag::graft_landing(&host.pane, sub, here, area)?;
        Some((pinned, rect))
    }

    /// Merges a dragged tab into the tab on screen, where the last painted
    /// frame said it would go.
    ///
    /// The panes come across as they were arranged, and the tab they came from
    /// goes away with them — nothing is killed and nothing is respawned, so a
    /// shell mid-command carries on through the move. What the source tab held
    /// besides panes is the tab's own: its name goes, and its overlays come
    /// along only where the tab receiving them has none of its own to lose.
    pub(super) fn merge_tab(
        &mut self,
        source: anterm_core::machine::TabId,
        zone: crate::ui::workspace::pane_drag::DropZone<gpui::EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_hover.set(None);
        let Some(from) = self.tabs.iter().position(|t| t.tree_id.get() == source) else {
            return;
        };
        let Some(host) = self.tabs.get(self.active) else {
            return;
        };
        if host.tree_id.get() == source {
            return;
        }
        let leaves = host.pane.leaves();
        let Some(zone) = zone.map(|id| leaves.iter().find(|l| l.entity_id() == id).cloned()) else {
            return;
        };

        let mut moved = self.tabs.remove(from);
        if from < self.active {
            self.active -= 1;
        }
        // Lifted out rather than cloned: these are the live terminals, and a
        // graft that finds nowhere to put them has to be able to hand them
        // back intact.
        let sub = std::mem::replace(&mut moved.pane, crate::ui::workspace::pane::Pane::Empty);
        let first = sub.first_leaf();
        let grafted = match self.tabs.get_mut(self.active) {
            Some(host) => crate::ui::workspace::pane_drag::graft(&mut host.pane, sub, zone),
            None => Err(sub),
        };
        if let Err(sub) = grafted {
            moved.pane = sub;
            self.tabs.insert(from, moved);
            if from <= self.active {
                self.active += 1;
            }
            return;
        }
        let merged_id = moved.tree_id.get();
        let host_id = self.tabs[self.active].tree_id.get();
        self.editor
            .update(cx, |ed, cx| ed.merge_tab_documents(host_id, merged_id, cx));
        if self
            .renaming
            .as_ref()
            .is_some_and(|r| !self.tabs.iter().any(|t| t.tree_id.get() == r.tab))
        {
            self.renaming = None;
        }
        self.maximized = None;
        if let Some(leaf) = first {
            self.focus_leaf(&leaf, window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// The caret between two tabs where a pane carried up to the strip — or
    /// out to the sidebar — would become a tab of its own.
    ///
    /// The reverse of grafting a tab in, and read the same way: offered only
    /// when the drop would change something, which for a detach means the pane
    /// has somewhere to leave. The last pane in a tab is already a tab, so
    /// nothing lights up for it.
    pub(super) fn detach_caret(&self, window: &Window, cx: &App) -> Option<gpui::AnyElement> {
        self.pane_detach.set(None);
        let pane = crate::ui::workspace::pane_drag::lifted(&self.pane_drag)?;
        let tab = self.tabs.get(self.active)?;
        if tab.pane.leaves().len() < 2 {
            return None;
        }
        if !tab.pane.leaves().iter().any(|l| l.entity_id() == pane) {
            return None;
        }
        // The pane's own landing is read later in the frame and answers nothing
        // while the pointer is up here, so the two are never both on offer;
        // the drop takes this one first regardless.
        let (at, caret) = self.detach_slot(window, cx)?;
        self.pane_detach.set(Some((pane, at)));

        let accent = cx.theme().drag_border;
        Some(
            div()
                .absolute()
                .left(caret.origin.x)
                .top(caret.origin.y)
                .w(caret.size.width)
                .h(caret.size.height)
                .rounded_full()
                .bg(accent)
                .into_any_element(),
        )
    }

    /// Which gap between tabs the pointer is in, as the tab a newcomer would
    /// be inserted before and the caret marking it — on the strip, or on the
    /// sidebar, whichever the pointer is over.
    ///
    /// `viewport` and the pointer are both in the window's outer coordinates.
    fn detach_slot(&self, window: &Window, cx: &App) -> Option<(usize, Bounds<Pixels>)> {
        /// How far above its first row the sidebar's band starts, so the gap
        /// over that row is inside it.
        const BAND_REACH: f32 = 6.;

        let pointer = window.mouse_position();
        // The two surfaces never share a window: the sidebar *is* the tab bar
        // when it is up, and the strip is what stands in for it when it is not.
        let vertical = matches!(cx.global::<Config>().tab_bar_position, TabBarPosition::Left)
            && !self.tabs.is_empty();
        let viewport = window.viewport_size();
        let pad = gpui_kit::component::window_paddings(window);
        // The band each surface claims, read off the tabs it drew rather than
        // measured as an element of its own: the chips and the rows are the
        // only part of either surface a drop has anything to say about, and a
        // band derived from them cannot disagree with the gaps measured inside
        // it. Stretched past the outermost tab so the empty space beyond —
        // where the strip keeps its New Tab button, where the sidebar keeps
        // nothing at all — still reads as "after everything".
        let band = |axis: Axis, drawn: &[(usize, Bounds<Pixels>)]| {
            let top = drawn.iter().map(|(_, b)| b.origin.y).reduce(Pixels::min)?;
            let left = drawn.iter().map(|(_, b)| b.origin.x).reduce(Pixels::min)?;
            let right = drawn
                .iter()
                .map(|(_, b)| b.origin.x + b.size.width)
                .reduce(Pixels::max)?;
            Some(match axis {
                Axis::Horizontal => strip_band(viewport, pad),
                Axis::Vertical => Bounds {
                    origin: point(left, top - px(BAND_REACH)),
                    size: size(
                        right - left,
                        (viewport.height - top + px(BAND_REACH)).max(px(0.)),
                    ),
                },
            })
        };

        // Sorted by where they were drawn rather than by tab: the sidebar
        // groups its rows, and a strip that has scrolled shows a window of
        // chips. What the pointer is between is a matter of the screen.
        let measured = |slots: &[Bounds<Pixels>]| -> Vec<(usize, Bounds<Pixels>)> {
            slots
                .iter()
                .enumerate()
                .filter(|(_, b)| b.size.width > px(0.) && b.size.height > px(0.))
                .map(|(i, b)| (i, *b))
                .collect()
        };
        let surfaces = [
            (!vertical).then(|| (Axis::Horizontal, measured(&self.strip_slots.borrow()))),
            self.sidebar_open(cx)
                .then(|| (Axis::Vertical, measured(&self.sidebar_slots.borrow()))),
        ];
        let (axis, mut drawn) = surfaces
            .into_iter()
            .flatten()
            .find(|(axis, drawn)| band(*axis, drawn).is_some_and(|b| b.contains(&pointer)))?;
        let lead = |b: &Bounds<Pixels>| match axis {
            Axis::Horizontal => b.origin.x,
            Axis::Vertical => b.origin.y,
        };
        drawn.sort_by(|(_, a), (_, b)| lead(a).as_f32().total_cmp(&lead(b).as_f32()));

        let row: Vec<Bounds<Pixels>> = drawn.iter().map(|(_, b)| *b).collect();
        let (gap, caret) = crate::ui::workspace::pane_drag::insertion(&row, axis, pointer)?;
        // The gap counts tabs on screen; what an insert needs is a place in the
        // list. Past the last tab drawn is the end of the list, which is not
        // the same thing as the last tab drawn plus one: a strip too narrow to
        // show every chip has tabs on either side of the ones it drew.
        let at = drawn.get(gap).map(|(i, _)| *i).unwrap_or(self.tabs.len());
        Some((at, caret))
    }

    /// Takes a dragged pane out of its tab and gives it one of its own, where
    /// the last painted frame's caret said.
    ///
    /// Nothing is spawned and nothing is killed: the pane that leaves is the
    /// same pane, still running whatever it was running.
    pub(super) fn detach_pane(
        &mut self,
        pane: gpui::EntityId,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_hover.set(None);
        // Which pane the tab being left comes back to, settled while the one
        // that is leaving is still in it to be ruled out.
        self.remember_active_pane(window, cx);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(slot) = tab
            .pane
            .leaves()
            .into_iter()
            .find(|l| l.entity_id() == pane)
        else {
            return;
        };
        // Refused for the last pane in a tab, which is what makes a drop that
        // would change nothing do nothing.
        let Some(slot) = tab.pane.take_leaf(&slot) else {
            return;
        };
        let fresh = Tab::new(crate::ui::workspace::pane::Pane::leaf(slot));
        let at = at.min(self.tabs.len());
        self.tabs.insert(at, fresh);
        self.maximized = None;
        self.active = at;
        self.focus_active(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Puts a dragged pane down where the last painted frame said it would go.
    ///
    /// Both ends of the drop are named by pane rather than by position, so a
    /// pane that closed between the frame that offered the landing and this one
    /// leaves the drop with nothing to land against, and it is refused.
    pub(super) fn drop_pane(
        &mut self,
        from: gpui::EntityId,
        zone: crate::ui::workspace::pane_drag::DropZone<gpui::EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_hover.set(None);
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let leaves = tab.pane.leaves();
        let here = |id| leaves.iter().find(|l| l.entity_id() == id).cloned();
        let (Some(moved), Some(zone)) = (here(from), zone.map(here)) else {
            return;
        };
        if !crate::ui::workspace::pane_drag::apply(&mut tab.pane, &moved, zone) {
            return;
        }
        self.maximized = None;
        self.focus_leaf(&moved, window, cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Activates the tab carrying `id`. A workspace this window just switched
    /// to hydrates its tabs asynchronously, so when the tab is not here yet the
    /// request is parked and claimed on the frame it arrives.
    pub(crate) fn activate_tree_tab(
        &mut self,
        id: anterm_core::machine::TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.tabs.iter().position(|t| t.tree_id.get() == id) {
            Some(index) => self.activate(index, window, cx),
            None => self.pending_tab = Some(id),
        }
    }

    pub(super) fn claim_pending_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(want) = self.pending_tab else {
            return;
        };
        let Some(index) = self.tabs.iter().position(|t| t.tree_id.get() == want) else {
            return;
        };
        self.pending_tab = None;
        self.activate(index, window, cx);
    }

    /// Declare to the pane registry which terminals are actually visible on
    /// screen this frame: only the active tab's unoccluded panes.
    ///
    /// Excludes background tabs, non-maximized siblings when a split is
    /// maximized, and panes fully covered by in-window settings or a fill-mode
    /// diff/code overlay. Entity ids only — reading the entities here would put
    /// them into the window's tracked set.
    pub(super) fn declare_displayed_panes(&self, cx: &App) {
        let active = self.active;
        let active_tab_maximized = self.tabs.get(active).and_then(|tab| {
            let max = self.maximized.as_ref()?;
            let max_id = max.entity_id();
            tab.pane
                .leaves()
                .iter()
                .any(|l| l.entity_id() == max_id)
                .then_some(max_id)
        });
        let active_tab_overlay_occludes = self.editor.read(cx).active_overlay_occludes(cx);
        crate::terminal::view::declare_displayed(
            cx,
            self.tabs.iter().enumerate().flat_map(|(i, tab)| {
                tab.pane.leaves().into_iter().filter_map(move |slot| {
                    let id = slot.terminal()?.entity_id();
                    let visible = i == active
                        && !active_tab_overlay_occludes
                        && active_tab_maximized.is_none_or(|max_id| max_id == id);
                    Some((id, visible))
                })
            }),
        );
    }

    /// Stamps whichever tab is active right now. Called once per frame rather
    /// than from the ten places that assign `self.active` — it is idempotent,
    /// so the stamp only advances on the first frame after a switch.
    pub(crate) fn touch_active_tab(&self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let top = self.tab_use_seq.get();
        if top != 0 && tab.last_used.get() == top {
            return;
        }
        let next = top + 1;
        self.tab_use_seq.set(next);
        tab.last_used.set(next);
    }

    pub(super) fn tab_switch(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_tab(forward, window, cx);
    }

    /// The neighbouring tab in the order the strip shows them, wrapping at
    /// the ends — no switcher, no MRU (#867). "Shows" matters with the
    /// sidebar grouping tabs: the next row is not always the next index.
    pub(super) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.visual_tab_order(cx);
        if let Some(next) = step_in_order(&order, self.active, forward) {
            self.activate(next, window, cx);
        }
    }

    /// Reorders the active tab one slot — the keyboard form of the drag that
    /// the strip and the sidebar both do. Left is back along the order and
    /// right is forward (on a left tab bar that reads as up and down), and
    /// both wrap: past the last tab the first one follows, so a tab can walk
    /// the whole list without letting go of the key. On a left tab bar the
    /// list is the tab's own sidebar group: the grouping keys off the tab,
    /// not its position, so a step out of the group would move nothing.
    pub(super) fn move_tab(&mut self, right: bool, cx: &mut Context<Self>) {
        let run = self.tab_run(self.active, cx);
        if let Some(permutation) = one_slot_move(&run, self.tabs.len(), self.active, right) {
            self.apply_tab_order(&permutation, cx);
        }
    }

    pub(crate) fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        // A tab is woken by being looked at, the way a browser reloads a
        // discarded tab when it is selected: a tab on screen is always a
        // running one, so there is no third state to draw. A wake that fails
        // leaves the tab asleep and the window where it was.
        if self.tabs.get(index).is_some_and(Tab::is_asleep) && !self.wake_tab(index, window, cx) {
            return;
        }
        if index < self.tabs.len() && index != self.active {
            self.remember_active_pane(window, cx);
            // Zoom rides with its tab (#599): stash the outgoing tab's zoom
            // and bring the incoming tab's back. The clears elsewhere (drag,
            // split, close) still stand — those genuinely reshape what was
            // zoomed; merely looking at another tab does not.
            if let Some(outgoing) = self.tabs.get_mut(self.active) {
                outgoing.zoomed = self.maximized.take();
            }
            self.active = index;
            self.maximized = self.tabs[index].zoomed.take().filter(|leaf| {
                // The zoomed pane may have exited while its tab was away —
                // a stale entity must not come back as the zoom.
                self.tabs[index]
                    .pane
                    .leaves()
                    .iter()
                    .any(|l| l.entity_id() == leaf.entity_id())
            });
            self.sync_editor_context(Some(window), cx);
            self.maybe_refresh_diff_overlay(cx);
            self.sidebar_reveal
                .set(Some(self.tabs[index].tree_id.get()));
            if self.code_panel_visible(cx) {
                self.file_tree_refresh_roots(window, cx);
                let handle = self
                    .right_panel
                    .read(cx)
                    .file_tree
                    .read(cx)
                    .focus_handle
                    .clone();
                handle.focus(window, cx);
            } else {
                self.focus_active(window, cx);
            }
            self.save_session(cx);
            cx.notify();
        }
    }

    /// Whether tab `index` can be put to sleep right now (#762).
    ///
    /// Not while it is connecting: a pane still on its way has no id the
    /// machine knows, so it would be lost rather than stopped. Not with a
    /// native SSH pane in a remote window either — that pane runs on this
    /// computer, beyond the reach of the machine asked to stop the tab. Not on
    /// a machine whose server cannot do it. And not the one tab left awake:
    /// the window has to have something on screen, and looking at a tab is
    /// what wakes it.
    pub(crate) fn can_hibernate_tab(&self, index: usize, cx: &App) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };
        if tab.is_asleep() || !crate::ui::remote::tree_sync::can_hibernate(cx) {
            return false;
        }
        let leaves = tab.pane.leaves();
        let all_hibernatable = !leaves.is_empty()
            && leaves
                .iter()
                .all(|leaf| matches!(leaf, PaneSlot::Ready(_)));
        all_hibernatable && self.awake_tab_besides(index).is_some()
    }

    /// The tab to show instead of `index` when that one goes to sleep: the
    /// awake tab used most recently, the nearest one on a tie.
    pub(super) fn awake_tab_besides(&self, index: usize) -> Option<usize> {
        (0..self.tabs.len())
            .filter(|&i| i != index && !self.tabs[i].is_asleep())
            .max_by_key(|&i| {
                (
                    self.tabs[i].last_used.get(),
                    std::cmp::Reverse(i.abs_diff(index)),
                )
            })
    }

    /// Put tab `index` to sleep: stop its panes, keep its place (#762).
    ///
    /// The window lets go of the panes first and the machine stops them after,
    /// from the sync this ends with. That order is what keeps a stopped pane
    /// from reading as one that died: by the time its shell exits there is no
    /// view left here to take the exit as a reason to close the pane, or the
    /// tab with it. The machine, for its part, marks the tab before it stops
    /// anything, so a restore reading the tree in between never takes the
    /// panes for dead ones and spawns them straight back.
    pub(crate) fn hibernate_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_hibernate_tab(index, cx) {
            return;
        }
        if index == self.active {
            let Some(next) = self.awake_tab_besides(index) else {
                return;
            };
            self.activate(next, window, cx);
        }
        self.put_to_sleep(index, window, cx);
    }

    /// The part of [`Self::hibernate_tab`] that does not ask whether it may:
    /// the tab lets go of its panes and keeps what a wake needs instead.
    pub(super) fn put_to_sleep(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index).filter(|t| !t.is_asleep()) else {
            return;
        };
        let (mut view, home) = tab.label_view(Some(window), cx);
        view.live = false;
        view.status = None;
        let layout = pane_to_session(&tab.pane, cx);
        let tab = &mut self.tabs[index];
        let panes = std::mem::replace(&mut tab.pane, Pane::Empty);
        tab.zoomed = None;
        tab.last_focused = None;
        tab.focus_origin.clear();
        tab.asleep = Some(Asleep { layout, view, home });
        drop(panes);
        self.save_session(cx);
        cx.notify();
    }

    /// Bring a sleeping tab back, through the restore every pane takes after a
    /// reboot: each pane is spawned in its old one's place, opens on the screen
    /// that one left behind, and resumes its agent session when it had one.
    /// Returns whether the tab is awake now; a tab none of whose panes could
    /// be started stays asleep, and says so.
    pub(crate) fn wake_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(asleep) = self.tabs.get_mut(index).and_then(|t| t.asleep.take()) else {
            return true;
        };
        let layout = redial_native_ssh(&asleep.layout);
        let pane = session_to_pane(&layout, None, self.font_size, window, cx);
        let tab = &mut self.tabs[index];
        let Some(pane) = pane else {
            tab.asleep = Some(asleep);
            window.push_notification(
                gpui_kit::component::notification::Notification::error(t(L10nKey::TabWakeFailed)),
                cx,
            );
            return false;
        };
        tab.pane = pane;
        tab.last_focused = None;
        self.save_session(cx);
        cx.notify();
        true
    }

    /// Wake the tab on screen if it is asleep — for the paths that land on a
    /// tab without going through [`Self::activate`]: a neighbour taking over
    /// from a closed tab, a window restored onto a sleeping one.
    pub(crate) fn wake_active_if_asleep(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.get(self.active).is_some_and(Tab::is_asleep) {
            self.wake_tab(self.active, window, cx);
        }
    }

    /// Whether tab `index` has a pane zoomed over hidden siblings — what the
    /// chrome marks so the state is readable without toggling it (#752).
    ///
    /// Zoom rides with its tab (#599): the active tab's lives in
    /// `self.maximized`, every other tab's is parked in `Tab::zoomed`. Either
    /// can name a pane that exited while nobody was looking, which is why the
    /// answer is asked of the layout rather than of the handle alone.
    pub(crate) fn tab_is_zoomed(&self, index: usize) -> bool {
        let Some(tab) = self.tabs.get(index) else {
            return false;
        };
        let zoom = match index == self.active {
            true => self.maximized.as_ref(),
            false => tab.zoomed.as_ref(),
        };
        zoom.is_some_and(|zoom| {
            tab.pane
                .zoom_hides_siblings(|slot| slot.entity_id() == zoom.entity_id())
        })
    }

    /// Zoom the pane being worked in over its siblings, or put the layout back.
    ///
    /// Both directions name the pane outright rather than leaving it to
    /// whatever holds focus at the instant the toggle lands (#869). Un-zooming
    /// hands the tab the pane that was zoomed as its answer before focusing it:
    /// that pane was the only one on screen to work in, and the tab's memory is
    /// written by focus-in alone, which nothing promises has caught up. Zooming
    /// with focus off the panes — a palette just closed, the tab strip — takes
    /// the pane the tab remembers, the one switching back to it would focus,
    /// and not whichever leaf the layout happens to list first.
    pub(super) fn toggle_maximize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(leaf) = self.maximized.take() {
            remember_leaf_in(&mut self.tabs, leaf.entity_id());
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if tab.pane.leaves().len() < 2 {
            return;
        }
        let leaf = tab
            .pane
            .focused_leaf(window, cx)
            .or_else(|| tab.focus_target())
            .and_then(|slot| slot.terminal().cloned());
        if let Some(leaf) = leaf {
            let handle = leaf.read(cx).focus_handle.clone();
            self.maximized = Some(leaf);
            window.focus(&handle, cx);
            cx.notify();
        }
    }

    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_inner(index, false, window, cx);
    }

    /// See [`Self::close_pane_inner`] for what `confirmed` carries.
    pub(super) fn close_tab_inner(
        &mut self,
        index: usize,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        if !confirmed {
            let id = self.tabs[index].tree_id.get();
            let unsaved = self.editor_unsaved_in_tab(index, cx);
            if self.editor_guard_unsaved(
                unsaved,
                crate::ui::editor::code_editor::AfterUnsaved::CloseTab(id),
                window,
                cx,
            ) {
                return;
            }
        }
        if !confirmed && let Some(reason) = self.tab_close_reason(index, cx) {
            let id = self.tabs[index].tree_id.get();
            self.ask_before_closing(CloseTarget::Tab(id), reason, window, cx);
            return;
        }
        self.maximized = None;
        let closing = self.tabs[index].tree_id.get();
        self.editor.update(cx, |ed, cx| {
            ed.editor_close_tab_files(closing, cx);
            ed.remove_tab_state(closing, cx);
        });
        let worktree_cwd = self.tab_host_cwd(index, window, cx);
        let snapshot = tab_to_session(&self.tabs[index], cx);
        let leaves: Vec<(u64, crate::terminal::PaneRoute)> = self.tabs[index]
            .pane
            .terminals()
            .iter()
            .map(|leaf| {
                let view = leaf.read(cx);
                (view.pane_id, view.pane_route())
            })
            .collect();
        crate::ui::remote::tree_sync::intend_close(cx, closing);
        let remembered = crate::ui::remote::tree_sync::remembers_closed_tab(cx, closing);
        if remembered {
            let retiring = crate::ui::remote::tree_sync::Retiring {
                tab: closing,
                panes: leaves.iter().map(|(id, _)| *id).collect(),
                route: crate::terminal::PaneRoute::Local,
            };
            crate::ui::remote::tree_sync::retire_tab(cx, retiring);
        } else {
            self.remember_closed_here(snapshot.clone());
            for (pane, route) in leaves {
                kill_pane_off_thread(route, pane, cx);
            }
        }
        self.tabs.remove(index);
        // Only losing the renaming tab itself ends the rename — closing an
        // unrelated tab must not throw the half-typed name away (#598).
        if self
            .renaming
            .as_ref()
            .is_some_and(|r| !self.tabs.iter().any(|t| t.tree_id.get() == r.tab))
        {
            self.renaming = None;
        }
        if self.tabs.is_empty() {
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
        self.wake_active_if_asleep(window, cx);
        self.focus_active(window, cx);
        self.save_session(cx);
        if remembered
            && let Some(unsent) =
                crate::ui::remote::tree_sync::take_unsent_retirement(cx, closing)
        {
            // The sync never got the close to the machine — the link went, or
            // the window was told to re-pull just now. Close it the way a
            // machine without the list would: kept here, ended from here.
            self.remember_closed_here(snapshot);
            for pane in unsent.panes {
                kill_pane_off_thread(unsent.route.clone(), pane, cx);
            }
        }
        cx.notify();
        self.offer_worktree_cleanup(worktree_cwd, cx);
    }

    /// Keep a closed tab in this window's own list, for a machine that keeps
    /// none — see [`Self::reopen_closed_tab`].
    pub(super) fn remember_closed_here(&mut self, snapshot: SessionTab) {
        self.closed.push(snapshot);
        if self.closed.len() > MAX_CLOSED_TABS {
            self.closed.remove(0);
        }
    }

    fn offer_worktree_cleanup(
        &mut self,
        cwd: Option<(crate::ui::remote::host_ops::SharedHost, std::path::PathBuf)>,
        cx: &mut Context<Self>,
    ) {
        let Some((host, cwd)) = cwd else { return };
        let id = host.id();
        let open_cwds: Vec<std::path::PathBuf> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .filter_map(|leaf| {
                let view = leaf.read(cx);
                (view.host_id() == id).then(|| view.host_cwd())?
            })
            .collect();
        let remove_host = host.clone();
        crate::ui::remote::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                crate::core::worktree::managed(h, &cwd)
                    .filter(|wt| !crate::core::worktree::occupied(h, &wt.path, &open_cwds))
            },
            move |_this, found, cx| {
                let Some(wt) = found else { return };
                let path = wt.path.display().to_string();
                let name = wt
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let detail = if wt.dirty {
                    t_fmt(
                        L10nKey::AppWorktreeRemoveDetailDirty,
                        &[("path", &path), ("name", &name)],
                    )
                } else {
                    t_fmt(L10nKey::AppWorktreeRemoveDetailClean, &[("path", &path)])
                };
                let title = t_fmt(L10nKey::AppWorktreeRemoveTitle, &[("branch", &wt.branch)]);
                let level = if wt.dirty {
                    PromptLevel::Warning
                } else {
                    PromptLevel::Info
                };
                let remove_label = if wt.dirty {
                    t(L10nKey::AppWorktreeDiscardAndRemove)
                } else {
                    t(L10nKey::AppWorktreeRemove)
                };
                cx.spawn(async move |this, cx| {
                    let Ok(answer) = this.update_in(cx, |_, window, cx| {
                        window.prompt(
                            level,
                            &title,
                            Some(&detail),
                            &crate::ui::confirm_answers(remove_label, t(L10nKey::AppWorktreeKeep)),
                            cx,
                        )
                    }) else {
                        return;
                    };
                    if !matches!(answer.await, Ok(0)) {
                        return;
                    }
                    let force = wt.dirty;
                    let branch = wt.branch.clone();
                    let _ = this.update_in(cx, |_, window, cx| {
                        crate::ui::remote::host_ops::HostOps::run_in(
                            remove_host,
                            window,
                            cx,
                            move |h| crate::core::worktree::remove(h, &wt, force),
                            move |_this, result, window, cx| match result {
                                Ok(removed) => window.push_notification(
                                    t_fmt(
                                        if removed.branch_kept {
                                            L10nKey::AppWorktreeRemovedBranchKept
                                        } else {
                                            L10nKey::AppWorktreeRemoved
                                        },
                                        &[("branch", &branch)],
                                    ),
                                    cx,
                                ),
                                Err(e) => window.push_notification(
                                    crate::ui::remote::host_ops::failure(
                                        t_fmt(
                                            L10nKey::AppWorktreeRemoveFailed,
                                            &[("error", &e.to_string())],
                                        ),
                                        &e,
                                    ),
                                    cx,
                                ),
                            },
                        );
                    });
                })
                .detach();
            },
        );
    }

    pub(crate) fn close_other_tabs(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_other_tabs_inner(index, false, window, cx);
    }

    /// Which of the tabs at `indices` a bulk close takes: all but the ones
    /// whose connection asked to be warned about, or that hold unsaved editor
    /// files — see [`Self::close_other_tabs_inner`].
    fn bulk_closable(&self, indices: impl Iterator<Item = usize>, cx: &App) -> Vec<usize> {
        indices
            .filter(|&i| !self.tab_has_warn_ssh(i, cx) && self.editor_unsaved_in_tab(i, cx).is_empty())
            .collect()
    }

    /// Under [`ConfirmClose::Always`](crate::core::config::ConfirmClose::Always)
    /// a bulk close is asked about once, for all the tabs it takes; one dialog
    /// per tab would be a question nobody can answer. Answers whether the
    /// question went up, in which case the close waits for it.
    fn ask_before_bulk_close(
        &mut self,
        target: CloseTarget,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let always =
            cx.global::<Config>().confirm_close == crate::core::config::ConfirmClose::Always;
        if !always || count == 0 {
            return false;
        }
        self.ask_before_closing(target, CloseReason::Tabs(count), window, cx);
        true
    }

    fn close_other_tabs_inner(
        &mut self,
        index: usize,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        if !confirmed {
            let count = self
                .bulk_closable((0..self.tabs.len()).filter(|&i| i != index), cx)
                .len();
            let keep = CloseTarget::OtherTabs(self.tabs[index].tree_id.get());
            if self.ask_before_bulk_close(keep, count, window, cx) {
                return;
            }
        }
        // A bulk close skips the tabs whose profile asked to be warned about,
        // and closes the rest outright — one dialog per tab is not a question
        // anyone can answer, and only the first would ever get asked. It
        // deliberately does *not* skip merely busy tabs: on a working window
        // that is most of them, and a menu item that quietly closes nothing is
        // worse than one that closes what it says.
        for i in (0..self.tabs.len()).rev() {
            // Unsaved editor files are skipped the same way: a bulk close
            // cannot ask about them one tab at a time, and must not lose them.
            if i == index
                || self.tab_has_warn_ssh(i, cx)
                || !self.editor_unsaved_in_tab(i, cx).is_empty()
            {
                continue;
            }
            self.close_tab_inner(i, true, window, cx);
        }
    }

    pub(crate) fn close_tabs_right_of(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tabs_right_of_inner(index, false, window, cx);
    }

    fn close_tabs_right_of_inner(
        &mut self,
        index: usize,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !confirmed && let Some(tab) = self.tabs.get(index) {
            let count = self.bulk_closable((index + 1)..self.tabs.len(), cx).len();
            let from = CloseTarget::TabsRightOf(tab.tree_id.get());
            if self.ask_before_bulk_close(from, count, window, cx) {
                return;
            }
        }
        // Same bargain as `close_other_tabs`.
        for i in ((index + 1)..self.tabs.len()).rev() {
            if self.tab_has_warn_ssh(i, cx) || !self.editor_unsaved_in_tab(i, cx).is_empty() {
                continue;
            }
            self.close_tab_inner(i, true, window, cx);
        }
    }

    pub(crate) fn mark_tab_unread(&mut self, index: usize, cx: &mut Context<Self>) {
        use crate::core::cli_agent::AgentStatus;
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let refocus = (index == self.active).then(|| tab.focus_target()).flatten();
        for leaf in tab.pane.terminals() {
            let refocus_incoming =
                refocus.as_ref().map(|s| s.entity_id()) == Some(leaf.entity_id());
            leaf.update(cx, |view, cx| {
                if view.agent_session().map(|s| s.status) == Some(AgentStatus::Done) {
                    view.mark_agent_result_unread(refocus_incoming, cx);
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    /// A tab's working directory, spelled for the clipboard.
    ///
    /// A cwd on this machine is re-spelled with this OS's separators, because
    /// what goes on the clipboard is meant to be pasted into a shell and a
    /// mixed-separator path is not one Windows will take. A remote pane's cwd
    /// keeps the spelling its own machine uses — it is already native over
    /// there. Both "Copy working directory" entry points (the app-menu action
    /// and the tab context menu) come through here so the two cannot drift.
    pub(crate) fn tab_cwd_text(&self, index: usize, window: &Window, cx: &App) -> Option<String> {
        let leaf = self.tabs.get(index)?.pane.focused_or_first(window, cx)?;
        let view = leaf.read(cx);
        let cwd = view.cwd()?;
        Some(match view.local_cwd().is_some() {
            true => crate::ui::files::path_display::native_separators(&cwd)
                .display()
                .to_string(),
            false => cwd.display().to_string(),
        })
    }

    pub(crate) fn copy_active_cwd(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(text) = self.tab_cwd_text(self.active, window, cx) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn tab_agent_session(
        &self,
        index: usize,
        window: &Window,
        cx: &App,
    ) -> Option<(Entity<TerminalView>, TabAgentSession)> {
        let leaf = self.tabs.get(index)?.pane.focused_or_first(window, cx)?;
        let view = leaf.read(cx);
        let agent = view.agent()?;
        let session = TabAgentSession {
            fork_label: agent.fork_label(),
            session_id: view.agent_session().and_then(|s| s.session_id),
            remote: view.remote_context().is_some(),
        };
        Some((leaf, session))
    }

    pub(crate) fn copy_agent_session_id(
        &mut self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self
            .tab_agent_session(index, window, cx)
            .and_then(|(_, s)| s.session_id)
        {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(id));
        }
    }

    pub(crate) fn fork_agent_session(
        &mut self,
        index: usize,
        source: Entity<TerminalView>,
        placement: ForkPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cmd) = self.agent_fork_command(&source, window, cx) else {
            return;
        };

        if matches!(placement, ForkPlacement::Split { .. }) {
            self.activate(index, window, cx);
        }

        // The directory the source runs in, on whichever host it runs on: an
        // agent keys its history by directory, and a fork started anywhere
        // else finds nothing to branch.
        let (cwd, shell) = {
            let view = source.read(cx);
            (view.spawnable_cwd(), view.shell_spec())
        };
        let new = match new_terminal(self.font_size, cwd, None, shell, window, cx) {
            Ok(view) => view,
            Err(e) => {
                log::error!("fork spawn failed: {e}");
                window.push_notification(
                    crate::ui::remote::host_ops::failure(
                        t_fmt(L10nKey::AppOpenTerminalFailed, &[("error", &e.to_string())]),
                        &e,
                    ),
                    cx,
                );
                return;
            }
        };
        // A remote workspace's pane is still dialling here; the fork runs
        // the moment it lands.
        crate::ui::workspace::agent_launch::run_when_ready(&new, cmd, cx);

        match placement {
            ForkPlacement::NewTab => {
                self.remember_active_pane(window, cx);
                self.maximized = None;
                let insert_at = self.new_tab_insert_at(cx);
                let tab = Tab::new(Pane::leaf(new));
                self.tabs.insert(insert_at, tab);
                self.active = insert_at;
                self.focus_active(window, cx);
            }
            ForkPlacement::Split { axis, before } => {
                let placed = self.tabs.get_mut(index).is_some_and(|tab| {
                    tab.pane
                        .split_leaf(source.entity_id(), axis, before, new.clone())
                });
                if !placed {
                    return;
                }
                self.maximized = None;
                self.focus_leaf(&new, window, cx);
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn fork_active_pane_session(
        &mut self,
        placement: ForkPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))
        else {
            return;
        };
        self.fork_agent_session(self.active, source, placement, window, cx);
    }

    pub(crate) fn fork_focused_pane_session(
        &mut self,
        axis: Axis,
        before: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fork_active_pane_session(ForkPlacement::Split { axis, before }, window, cx);
    }

    fn agent_fork_command(
        &self,
        source: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        use crate::core::cli_agent::AgentStatus;
        let view = source.read(cx);
        let (agent, session, remote) = (view.agent(), view.agent_session(), view.remote_context());
        let Some(agent) = agent else {
            window.push_notification(t(L10nKey::AppPaneNoCodingAgent), cx);
            return None;
        };
        let name = agent.display_name();
        if agent.fork_label().is_none() {
            window.push_notification(t_fmt(L10nKey::AppForkNoCommand, &[("name", &name)]), cx);
            return None;
        }
        if remote.is_some() {
            window.push_notification(t_fmt(L10nKey::AppForkLocalOnly, &[("name", &name)]), cx);
            return None;
        }
        let session = session.unwrap_or_default();
        let Some(id) = session.session_id.as_deref() else {
            window.push_notification(t_fmt(L10nKey::AppForkNoSessionId, &[("name", &name)]), cx);
            return None;
        };
        let Some(cmd) = agent.fork_command(id, session.launch_argv.as_deref()) else {
            window.push_notification(
                t_fmt(L10nKey::AppForkSessionIdNotToken, &[("name", &name)]),
                cx,
            );
            return None;
        };
        if session.status == AgentStatus::Working {
            window.push_notification(t_fmt(L10nKey::AppForkMidTurn, &[("name", &name)]), cx);
        }
        Some(cmd)
    }

    pub(crate) fn check_for_updates_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::core::update::spawn_check_forced(cx);
        self.open_settings_section(SettingsSection::About, window, cx);
    }

    fn tab_host_cwd(
        &self,
        index: usize,
        window: &Window,
        cx: &App,
    ) -> Option<(crate::ui::remote::host_ops::SharedHost, std::path::PathBuf)> {
        let leaf = self.tabs.get(index)?.pane.focused_or_first(window, cx)?;
        let view = leaf.read(cx);
        Some((view.host(cx)?, view.host_cwd()?))
    }

    pub(crate) fn apply_tab_order(&mut self, order: &[usize], cx: &mut Context<Self>) {
        if order.len() != self.tabs.len() || order.iter().enumerate().all(|(i, &o)| i == o) {
            return;
        }
        // The rename box rides out a reorder: it tracks its tab by tree id,
        // so the drift that once forced it closed here is gone (#598).
        let was_active = self.active;
        let mut slots: Vec<Option<Tab>> = std::mem::take(&mut self.tabs)
            .into_iter()
            .map(Some)
            .collect();
        self.tabs = order.iter().filter_map(|&i| slots[i].take()).collect();
        self.active = order.iter().position(|&i| i == was_active).unwrap_or(0);
        self.save_session(cx);
        cx.notify();
    }

    /// Opens a rename box on the current name, selected and focused, so the
    /// first thing typed replaces it.
    pub(crate) fn rename_box(
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let input = crate::ui::chrome::prefill::filled_box(current, window, cx);
        input.update(cx, |state, cx| state.focus(window, cx));
        input
    }

    pub(crate) fn start_rename(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.get(index).is_none() {
            return;
        }
        let current = self.tab_label(&self.tabs[index], index, Some(&*window), cx);
        let prefill = current.to_string();
        let input = Self::rename_box(current, window, cx);
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(window, cx),
                _ => {}
            },
        )];
        self.renaming = Some(Renaming {
            tab: self.tabs[index].tree_id.get(),
            input,
            prefill,
            _subs: subs,
        });
        cx.notify();
    }

    pub(super) fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let value = renaming.input.read(cx).value().to_string();
        self.apply_tab_rename(renaming.tab, &value, &renaming.prefill, window, cx);
    }

    pub(super) fn apply_tab_rename(
        &mut self,
        tab_id: anterm_core::machine::TabId,
        value: &str,
        prefill: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match rename_outcome(value, prefill) {
            Rename::Unchanged => {
                self.focus_active(window, cx);
                cx.notify();
                return;
            }
            outcome => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.tree_id.get() == tab_id) {
                    tab.name = match outcome {
                        Rename::Named(name) => Some(name),
                        _ => None,
                    };
                }
            }
        }
        self.save_session(cx);
        crate::ui::workspace::windows::refresh_menu(cx);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn tab_ssh_dot(&self, tab: &Tab, cx: &App) -> Option<u32> {
        use crate::daemon::protocol::SshPhase;
        let leaf = tab.pane.first_leaf()?;
        let v = leaf.terminal()?.read(cx);
        if let Some(phase) = v.ssh_phase() {
            let rgb = if v.ssh_disconnected() {
                0xEF4444
            } else {
                match phase {
                    SshPhase::Connecting | SshPhase::Authenticating => 0xF59E0B,
                    SshPhase::Connected => 0x22C55E,
                    SshPhase::Failed { .. } => 0xEF4444,
                }
            };
            Some(rgb)
        } else if v
            .remote_context()
            .is_some_and(|r| r.kind != crate::daemon::protocol::RemoteKind::Wsl)
        {
            Some(0x9CA3AF)
        } else {
            None
        }
    }

    fn leaf_is_warn_ssh(&self, leaf: &Entity<TerminalView>, cx: &App) -> bool {
        use crate::daemon::protocol::SshPhase;
        let v = leaf.read(cx);
        let connected = matches!(v.ssh_phase(), Some(SshPhase::Connected)) && !v.terminal.exited;
        if !connected {
            return false;
        }
        let cfg = cx.global::<Config>();
        let per_profile = v
            .ssh_spec()
            .and_then(|s| s.profile_id.clone())
            .and_then(|id| uuid::Uuid::parse_str(&id).ok())
            .and_then(|id| cfg.ssh_profiles.iter().find(|p| p.id == id))
            .and_then(|p| p.warn_on_close);
        per_profile.unwrap_or(cfg.ssh_warn_on_close)
    }

    /// The app asks every other question of this class through the platform's
    /// own dialog. This one used to be a bespoke in-app card with no scrim, no
    /// Escape and no click-outside — the two buttons were the only way out.
    fn ask_before_closing(
        &mut self,
        target: CloseTarget,
        reason: CloseReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_prompt_open {
            return;
        }
        self.close_prompt_open = true;
        // ⌘W closes a pane, but when it is the only one in its tab the tab goes
        // with it — the question has to name what actually disappears.
        let ends_the_tab = matches!(target, CloseTarget::Tab(_))
            || self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.pane.leaves().len() <= 1);
        let (title, body) = close_prompt(ends_the_tab, &reason);
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&body),
            &crate::ui::confirm_answers(
                t(anterm_i18n::L10nKey::Close),
                t(anterm_i18n::L10nKey::Keep),
            ),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let close = matches!(answer.await, Ok(0));
            let _ = this.update_in(cx, |this, window, cx| {
                this.close_prompt_open = false;
                // Cancelled, or the window went away with the question open:
                // either way the work carries on.
                if !close {
                    cx.notify();
                    return;
                }
                match target {
                    // The tab may have moved, or gone, while the question was
                    // up; find it by id and let it be if it is already closed.
                    CloseTarget::Tab(id) => {
                        if let Some(i) = this.tabs.iter().position(|t| t.tree_id.get() == id) {
                            this.close_tab_inner(i, true, window, cx);
                        }
                    }
                    CloseTarget::Pane => this.close_pane_inner(true, window, cx),
                    CloseTarget::OtherTabs(id) => {
                        if let Some(i) = this.tabs.iter().position(|t| t.tree_id.get() == id) {
                            this.close_other_tabs_inner(i, true, window, cx);
                        }
                    }
                    CloseTarget::TabsRightOf(id) => {
                        if let Some(i) = this.tabs.iter().position(|t| t.tree_id.get() == id) {
                            this.close_tabs_right_of_inner(i, true, window, cx);
                        }
                    }
                }
            });
        })
        .detach();
    }

    /// The first reason this pane should not simply vanish.
    fn leaf_close_reason(&self, leaf: &Entity<TerminalView>, cx: &App) -> Option<CloseReason> {
        self.leaf_close_reason_as(cx.global::<Config>().confirm_close, leaf, cx)
    }

    fn leaf_close_reason_as(
        &self,
        mode: crate::core::config::ConfirmClose,
        leaf: &Entity<TerminalView>,
        cx: &App,
    ) -> Option<CloseReason> {
        close_reason(mode, self.leaf_is_warn_ssh(leaf, cx), leaf.read(cx).busy())
    }

    /// Whether closing this tab would drop a connection the user asked to be
    /// warned about. Narrower than [`Self::tab_close_reason`] on purpose — see
    /// the bulk closes, which skip these and only these.
    fn tab_has_warn_ssh(&self, index: usize, cx: &App) -> bool {
        self.tabs.get(index).is_some_and(|tab| {
            tab.pane
                .terminals()
                .iter()
                .any(|l| self.leaf_is_warn_ssh(l, cx))
        })
    }

    /// The first reason any pane of the tab gives. Under `Always` a busy pane
    /// or a warned connection still names itself — the idle question is only
    /// what is left when no pane has anything more specific to say.
    fn tab_close_reason(&self, index: usize, cx: &App) -> Option<CloseReason> {
        use crate::core::config::ConfirmClose;
        let mode = cx.global::<Config>().confirm_close;
        let asked_as = match mode {
            ConfirmClose::Always => ConfirmClose::WhenBusy,
            other => other,
        };
        let specific = self
            .tabs
            .get(index)?
            .pane
            .terminals()
            .iter()
            .find_map(|l| self.leaf_close_reason_as(asked_as, l, cx));
        specific.or((mode == ConfirmClose::Always).then_some(CloseReason::Idle))
    }

    fn focused_pane_close_reason(&self, window: &Window, cx: &App) -> Option<CloseReason> {
        let leaf = self
            .tabs
            .get(self.active)?
            .pane
            .focused_or_first(window, cx)?;
        self.leaf_close_reason(&leaf, cx)
    }
}
