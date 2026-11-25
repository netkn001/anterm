//! Window-level GPUI action bindings for [`AntermApp`].

use gpui::{Axis, Context, Div, Stateful, prelude::*};

use crate::core::actions::*;
use crate::ui::workspace::pane::Dir;
use crate::ui::files::scm::ScmIntent;
use crate::ui::chrome::settings::SettingsSection;

use super::{
    AntermApp, DISCORD_URL, DOCS_URL, FONT_SIZE_STEP, ForkPlacement, ISSUES_URL, SpawnWhere,
};

impl AntermApp {
    /// Attach all window-level GPUI action listeners to the root container.
    pub(super) fn with_app_actions(
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root
                .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
                .on_action(cx.listener(|this, _: &NewWindow, _window, cx| {
                    this.new_window(cx);
                }))
                .on_action(
                    cx.listener(|this, _: &CloseWindow, window, cx| this.close_window(window, cx)),
                )
                .on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    if !this
                        .editor
                        .update(cx, |ed, cx| ed.editor_close_active_if_focused(window, cx))
                    {
                        this.close_pane(window, cx)
                    }
                }))
                .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                    this.split_in(Axis::Horizontal, false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SplitLeft, window, cx| {
                    this.split_in(Axis::Horizontal, true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SplitDown, window, cx| {
                    this.split_in(Axis::Vertical, false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SplitUp, window, cx| {
                    this.split_in(Axis::Vertical, true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &FocusNextPane, window, cx| {
                    this.cycle_pane(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &FocusPrevPane, window, cx| {
                    this.cycle_pane(false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &FocusPaneLeft, window, cx| {
                    this.focus_pane_dir(Dir::Left, window, cx)
                }))
                .on_action(cx.listener(|this, _: &FocusPaneRight, window, cx| {
                    this.focus_pane_dir(Dir::Right, window, cx)
                }))
                .on_action(cx.listener(|this, _: &FocusPaneUp, window, cx| {
                    this.focus_pane_dir(Dir::Up, window, cx)
                }))
                .on_action(cx.listener(|this, _: &FocusPaneDown, window, cx| {
                    this.focus_pane_dir(Dir::Down, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ResizePaneLeft, window, cx| {
                    this.resize_pane(Dir::Left, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ResizePaneRight, window, cx| {
                    this.resize_pane(Dir::Right, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ResizePaneUp, window, cx| {
                    this.resize_pane(Dir::Up, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ResizePaneDown, window, cx| {
                    this.resize_pane(Dir::Down, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SwapPaneNext, window, cx| {
                    this.swap_pane(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SwapPanePrev, window, cx| {
                    this.swap_pane(false, window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &NextTab, window, cx| this.tab_switch(true, window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &PrevTab, window, cx| this.tab_switch(false, window, cx)),
                )
                .on_action(cx.listener(|this, _: &SelectNextTab, window, cx| {
                    this.cycle_tab(true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SelectPrevTab, window, cx| {
                    this.cycle_tab(false, window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &MoveTabLeft, _window, cx| this.move_tab(false, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &MoveTabRight, _window, cx| this.move_tab(true, cx)),
                )
                .on_action(cx.listener(|this, _: &ActivateTab1, window, cx| {
                    this.activate_visual(0, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab2, window, cx| {
                    this.activate_visual(1, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab3, window, cx| {
                    this.activate_visual(2, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab4, window, cx| {
                    this.activate_visual(3, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab5, window, cx| {
                    this.activate_visual(4, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab6, window, cx| {
                    this.activate_visual(5, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab7, window, cx| {
                    this.activate_visual(6, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab8, window, cx| {
                    this.activate_visual(7, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ActivateTab9, window, cx| {
                    this.activate_visual(8, window, cx)
                }))
                .on_action(cx.listener(|this, _: &IncreaseFontSize, _window, cx| {
                    this.change_font_size(FONT_SIZE_STEP, cx)
                }))
                .on_action(cx.listener(|this, _: &DecreaseFontSize, _window, cx| {
                    this.change_font_size(-FONT_SIZE_STEP, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &ResetFontSize, _window, cx| this.reset_font_size(cx)),
                )
                .on_action(
                    cx.listener(|this, _: &TogglePalette, window, cx| {
                        this.toggle_search(window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &QuickOpenFile, window, cx| {
                    this.quick_open_file(window, cx)
                }))
                .on_action(cx.listener(|this, _: &ReopenClosedTab, window, cx| {
                    this.reopen_closed_tab(window, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleMaximizePane, window, cx| {
                    this.toggle_maximize(window, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleFullscreen, window, cx| {
                    this.toggle_fullscreen(window, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleTabSidebar, _window, cx| {
                    this.toggle_tab_sidebar(cx)
                }))
                .on_action(
                    cx.listener(|this, _: &ToggleLeftPanel, _window, cx| {
                        this.toggle_left_panel(cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &ToggleRightPanel, _window, cx| {
                    this.toggle_right_panel(cx)
                }))
                .on_action(cx.listener(|this, _: &ShowRightPanelInfo, _window, cx| {
                    this.set_right_panel_tab(crate::core::config::RightPanelTab::Info, cx)
                }))
                .on_action(cx.listener(|this, _: &ShowRightPanelChanges, _window, cx| {
                    this.set_right_panel_tab(crate::core::config::RightPanelTab::Scm, cx)
                }))
                .on_action(cx.listener(|this, _: &ShowRightPanelFiles, _window, cx| {
                    this.set_right_panel_tab(crate::core::config::RightPanelTab::Files, cx)
                }))
                .on_action(cx.listener(|this, _: &ShowRightPanelSearch, _window, cx| {
                    this.set_right_panel_tab(crate::core::config::RightPanelTab::Search, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &ScmToggleGraph, _window, cx| this.scm_toggle_graph(cx)),
                )
                .on_action(cx.listener(|this, _: &ToggleDiffViewMode, _window, cx| {
                    this.toggle_diff_view_mode(cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleDocumentFill, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    this.editor
                        .update(cx, |ed, cx| ed.toggle_document_fill(cx))
                }))
                .on_action(cx.listener(|this, _: &DocumentWidthThird, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    this.editor.update(cx, |ed, cx| {
                        ed.set_document_ratio(crate::core::config::DOCUMENT_RATIO_THIRD, cx)
                    })
                }))
                .on_action(cx.listener(|this, _: &DocumentWidthHalf, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    this.editor.update(cx, |ed, cx| {
                        ed.set_document_ratio(crate::core::config::DOCUMENT_RATIO_HALF, cx)
                    })
                }))
                .on_action(
                    cx.listener(|this, _: &DocumentWidthTwoThirds, window, cx| {
                        this.sync_editor_context(Some(window), cx);
                        this.editor.update(cx, |ed, cx| {
                            ed.set_document_ratio(
                                crate::core::config::DOCUMENT_RATIO_TWO_THIRDS,
                                cx,
                            )
                        })
                    }),
                )
                .on_action(cx.listener(|this, _: &ToggleDocumentPreview, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    this.editor
                        .update(cx, |ed, cx| ed.toggle_document_preview(cx))
                }))
                .on_action(cx.listener(|this, _: &ToggleDocumentWrap, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    this.editor
                        .update(cx, |ed, cx| ed.toggle_document_wrap(window, cx))
                }))
                .on_action(cx.listener(|this, _: &ScmCommit, window, cx| {
                    this.run_scm_action(ScmIntent::Commit, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmCommitAmend, window, cx| {
                    this.run_scm_action(ScmIntent::CommitAmend, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmStageAll, window, cx| {
                    this.run_scm_action(ScmIntent::StageAll, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmUnstageAll, window, cx| {
                    this.run_scm_action(ScmIntent::UnstageAll, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmDiscardAll, window, cx| {
                    this.run_scm_action(ScmIntent::DiscardAll, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmRefresh, window, cx| {
                    this.run_scm_action(ScmIntent::Refresh, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmSync, window, cx| {
                    this.run_scm_action(ScmIntent::Sync, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmPush, window, cx| {
                    this.run_scm_action(ScmIntent::Push, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmPull, window, cx| {
                    this.run_scm_action(ScmIntent::Pull, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmFetch, window, cx| {
                    this.run_scm_action(ScmIntent::Fetch, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmCheckoutBranch, window, cx| {
                    this.run_scm_action(ScmIntent::CheckoutBranch, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ScmCreateBranch, window, cx| {
                    this.run_scm_action(ScmIntent::CreateBranch, window, cx)
                }))
                .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                    this.toggle_settings(window, cx)
                }))
                .on_action(cx.listener(|this, _: &RestartDaemon, window, cx| {
                    this.restart_window_daemon(window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &ToggleSftp, window, cx| this.toggle_sftp(window, cx)),
                )
                .on_action(cx.listener(|this, _: &ShowSshForwards, window, cx| {
                    this.show_ssh_forwards(window, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleCodePanel, window, cx| {
                    this.toggle_code_panel(window, cx)
                }))
                .on_action(cx.listener(|this, _: &EditorSave, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_panel_has_focus(window, cx) {
                            return false;
                        }
                        ed.editor_save_active(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorSaveAs, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_panel_has_focus(window, cx) {
                            return false;
                        }
                        ed.editor_save_as_active(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorGoToLine, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_panel_has_focus(window, cx) {
                            return false;
                        }
                        ed.editor_go_to_line(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorGoToDefinition, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_has_focus(window, cx) {
                            return false;
                        }
                        ed.lsp_go_to_definition(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorQuickFix, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_has_focus(window, cx) {
                            return false;
                        }
                        ed.lsp_code_actions(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorRenameSymbol, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_has_focus(window, cx) {
                            return false;
                        }
                        ed.lsp_rename_start(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorFormatDocument, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_has_focus(window, cx) {
                            return false;
                        }
                        ed.lsp_format_document(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorFindReferences, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    let handled = this.editor.update(cx, |ed, cx| {
                        if !ed.editor_has_focus(window, cx) {
                            return false;
                        }
                        ed.lsp_find_references(window, cx);
                        true
                    });
                    if !handled {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorNewFile, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    this.editor
                        .update(cx, |ed, cx| ed.editor_new_file(window, cx))
                }))
                .on_action(cx.listener(|this, _: &EditorNextChange, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    if !this
                        .editor
                        .update(cx, |ed, cx| ed.editor_gutter_step(true, window, cx))
                    {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorPrevChange, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    if !this
                        .editor
                        .update(cx, |ed, cx| ed.editor_gutter_step(false, window, cx))
                    {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &EditorRevertChange, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    if !this
                        .editor
                        .update(cx, |ed, cx| ed.editor_gutter_revert_at_cursor(window, cx))
                    {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &ToggleEditorGitGutter, _, cx| {
                    this.editor
                        .update(cx, |ed, cx| ed.toggle_editor_git_gutter(cx))
                }))
                .on_action(cx.listener(|this, _: &EditorPeekChange, window, cx| {
                    this.sync_editor_context(Some(window), cx);
                    if !this
                        .editor
                        .update(cx, |ed, cx| ed.editor_gutter_peek_at_cursor(window, cx))
                    {
                        cx.propagate();
                    }
                }))
                .on_action(cx.listener(|this, _: &ToggleEditorProblems, _, cx| {
                    if !this.code_panel_visible(cx) {
                        cx.propagate();
                        return;
                    }
                    this.editor
                        .update(cx, |ed, cx| ed.toggle_editor_problems(cx))
                }))
                .on_action(
                    cx.listener(|this, _: &Quit, window, cx| this.quit_stop_sessions(window, cx)),
                )
                .on_action(cx.listener(|this, _: &OpenSshProfiles, window, cx| {
                    this.open_settings_section(SettingsSection::Ssh, window, cx)
                }))
                .on_action(cx.listener(|this, _: &RestartSshSession, window, cx| {
                    this.restart_ssh_session(window, cx)
                }))
                .on_action(cx.listener(|this, _: &RenameTab, window, cx| {
                    this.open_rename_tab_prompt(window, cx)
                }))
                .on_action(cx.listener(|this, _: &EditTabTags, window, cx| {
                    this.open_tag_popover(this.active, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SwitchTab, window, cx| {
                    this.toggle_switch_tab(window, cx)
                }))
                .on_action(cx.listener(|this, _: &SwitchTag, window, cx| {
                    this.toggle_switch_tag(window, cx)
                }))
                .on_action(cx.listener(|this, _: &CloseOtherTabs, window, cx| {
                    this.close_other_tabs(this.active, window, cx)
                }))
                .on_action(cx.listener(|this, _: &CloseTabsToTheRight, window, cx| {
                    this.close_tabs_right_of(this.active, window, cx)
                }))
                .on_action(cx.listener(|this, _: &CopyWorkingDirectory, window, cx| {
                    this.copy_active_cwd(window, cx)
                }))
                .on_action(cx.listener(|this, _: &MarkTabUnread, _window, cx| {
                    this.mark_tab_unread(this.active, cx)
                }))
                .on_action(cx.listener(|this, _: &HibernateTab, window, cx| {
                    this.hibernate_tab(this.active, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ForkAgentSession, window, cx| {
                    this.fork_active_pane_session(ForkPlacement::NewTab, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ForkAgentSessionRight, window, cx| {
                    this.fork_focused_pane_session(Axis::Horizontal, false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ForkAgentSessionLeft, window, cx| {
                    this.fork_focused_pane_session(Axis::Horizontal, true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ForkAgentSessionDown, window, cx| {
                    this.fork_focused_pane_session(Axis::Vertical, false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &ForkAgentSessionUp, window, cx| {
                    this.fork_focused_pane_session(Axis::Vertical, true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &CopyAgentSessionId, window, cx| {
                    this.copy_agent_session_id(this.active, window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &NewAgentTab, window, cx| this.new_agent_tab(window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &FocusNextAgentNeedingAttention, window, cx| {
                        this.focus_next_agent_needing_attention(window, cx)
                    }),
                )
                .on_action(cx.listener(|this, action: &LaunchAgent, window, cx| {
                    this.launch_agent(action.agent, SpawnWhere::NewTab, window, cx)
                }))
                .on_action(cx.listener(|this, _: &SaveAgentLaunchArgs, window, cx| {
                    this.save_agent_launch_args(window, cx)
                }))
                .on_action(cx.listener(|this, _: &ShowKeyboardShortcuts, window, cx| {
                    this.open_settings_section(SettingsSection::Keybindings, window, cx)
                }))
                .on_action(cx.listener(|this, _: &About, window, cx| {
                    this.open_settings_section(SettingsSection::About, window, cx)
                }))
                .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| {
                    this.check_for_updates_now(window, cx)
                }))
                .on_action(cx.listener(|_, _: &HideApp, _window, cx| cx.hide()))
                .on_action(cx.listener(|_, _: &HideOthers, _window, cx| cx.hide_other_apps()))
                .on_action(cx.listener(|_, _: &ShowAll, _window, cx| cx.unhide_other_apps()))
                .on_action(
                    cx.listener(|_, _: &MinimizeWindow, window, _cx| window.minimize_window()),
                )
                .on_action(cx.listener(|_, _: &ZoomWindow, window, _cx| window.zoom_window()))
                .on_action(
                    cx.listener(|_, _: &OpenDocumentation, _window, cx| cx.open_url(DOCS_URL)),
                )
                .on_action(cx.listener(|_, _: &OpenDiscord, _window, cx| cx.open_url(DISCORD_URL)))
                .on_action(cx.listener(|_, _: &ReportIssue, _window, cx| cx.open_url(ISSUES_URL)))
    }
}
