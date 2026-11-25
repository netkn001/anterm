//! Cross-panel workspace intent & context seam.
//!
//! Detached feature views (such as `ForwardsView`, `SshPromptModal`,
//! `SftpPanelView`, `PanelSearchView`, `FileTreeView`, `ScmPanelView`, and
//! `CodeEditorWorkspace`) emit [`WorkspaceIntent`] events instead of depending
//! on `AntermApp` directly. `AntermApp` subscribes to those views and executes
//! each intent through [`AntermApp::handle_intent`].
//!
//! Likewise, [`ActiveWorkspaceContext`] captures the narrow read-only snapshot
//! of the currently active tab that feature panels need (tab identity, host,
//! working directory) without indexing into `AntermApp::tabs`.

use gpui::{App, Context, Window};

use crate::ui::workspace::app::AntermApp;

pub(crate) use anterm_ui_kit::intent::{ActiveWorkspaceContext, WorkspaceIntent};

impl AntermApp {
    /// Produces the read-only [`ActiveWorkspaceContext`] snapshot for the
    /// currently active tab.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn active_workspace_context(&self, cx: &App) -> ActiveWorkspaceContext {
        let tab = self.tabs.get(self.active);
        let tab_id = tab.map(|t| t.tree_id.get());
        let host_id = self.spawn_host(cx);
        let cwd = tab.and_then(|t| {
            t.pane
                .terminals()
                .into_iter()
                .find_map(|leaf| leaf.read(cx).cwd())
        });
        ActiveWorkspaceContext {
            tab_id,
            host_id,
            cwd,
        }
    }

    /// Central dispatcher for [`WorkspaceIntent`] events emitted by feature
    /// views.
    pub(crate) fn handle_intent(
        &mut self,
        intent: WorkspaceIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match intent {
            WorkspaceIntent::OpenFile { path, line, column } => {
                self.open_linked_file(&path, line, column, false, window, cx);
            }
            WorkspaceIntent::RevealDirectory(path) => {
                self.open_linked_file(&path, None, None, true, window, cx);
            }
            WorkspaceIntent::FocusActiveTerminal => {
                self.focus_active(window, cx);
            }
            WorkspaceIntent::OpenSshProfile(profile_id) => {
                self.open_ssh_profile_in_settings(profile_id, window, cx);
            }
            WorkspaceIntent::RestartActiveSshSession => {
                self.restart_ssh_session(window, cx);
            }
            WorkspaceIntent::ShowRightPanel(tab) => {
                self.set_right_panel_tab(tab, cx);
            }
            WorkspaceIntent::HideRightPanel => {
                if self.right_panel.read(cx).visible {
                    self.toggle_right_panel(cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::RightPanelTab;
    use crate::ui::workspace::app::test_window::harness_with_tabs;
    use gpui::TestAppContext;

    #[gpui::test]
    fn handle_intent_toggles_right_panel_and_exposes_active_context(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let ctx = app.read_with(&vcx, |app, cx| app.active_workspace_context(cx));
        assert!(ctx.tab_id.is_some());
        assert!(ctx.host_id.is_local());

        app.update_in(&mut vcx, |app, window, cx| {
            app.handle_intent(WorkspaceIntent::ShowRightPanel(RightPanelTab::Files), window, cx);
        });
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, cx| app.right_panel.read(cx).visible));
        assert_eq!(
            app.read_with(&vcx, |app, cx| app.right_panel.read(cx).tab),
            RightPanelTab::Files
        );

        app.update_in(&mut vcx, |app, window, cx| {
            app.handle_intent(WorkspaceIntent::HideRightPanel, window, cx);
        });
        vcx.run_until_parked();
        assert!(!app.read_with(&vcx, |app, cx| app.right_panel.read(cx).visible));

        // Hiding an already-hidden panel is idempotent.
        app.update_in(&mut vcx, |app, window, cx| {
            app.handle_intent(WorkspaceIntent::HideRightPanel, window, cx);
        });
        vcx.run_until_parked();
        assert!(!app.read_with(&vcx, |app, cx| app.right_panel.read(cx).visible));
    }
}
