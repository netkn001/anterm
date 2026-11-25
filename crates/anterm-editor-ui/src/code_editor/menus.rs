//! Context menus, buffer closing, tab cycling, and agent attachment helpers.

use std::path::Path;

use anterm_core::machine::TabId;
use anterm_i18n::{L10nKey, t, t_fmt};
use gpui::{Context, Focusable as _, PromptLevel, Window, px};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

use super::io::{is_program, open_in_browser, opens_in_browser, selected_lines};
use super::{AfterUnsaved, BarKind, BufferId, text_actions};
use crate::CodeEditorWorkspace;

impl CodeEditorWorkspace {
    pub(super) fn editor_cycle_buffer(&mut self, next: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = self.tab_code() else {
            return;
        };
        let len = code.files.len();
        if len <= 1 {
            return;
        }
        let target = if next {
            (code.active + 1) % len
        } else {
            (code.active + len - 1) % len
        };
        self.editor_activate(target, window, cx);
    }

    /// Closes the file at `pos` in the active tab's strip. Asks first only
    /// when this is the last tab showing a buffer with unsaved changes.
    pub fn editor_close_file(
        &mut self,
        pos: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab_id() else {
            return;
        };
        let Some(id) = self.tab_code().and_then(|c| c.files.get(pos).copied()) else {
            return;
        };
        let Some(f) = self.buffer(id) else {
            return;
        };
        if !f.dirty || self.buffer_refs(id) > 1 {
            self.editor_remove_from_tab(tab, id, cx);
            return;
        }
        let name = f.label();
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::EditorUnsavedChanges, &[("name", &name)]),
            None,
            // Cancel sits between Save and Discard on purpose. The platform
            // renders the first button as the default and lays the rest out
            // beside it, so Discard was landing directly next to the key that
            // Return presses. Apple separates them for exactly this reason.
            // Three answers, so the shared helper does not fit: Save keeps
            // index 0 (rightmost, Return), Cancel takes Escape, and Discard
            // sits on the far left where nothing lands by reflex.
            &[
                gpui::PromptButton::ok(t(L10nKey::Save)),
                gpui::PromptButton::cancel(t(L10nKey::Cancel)),
                gpui::PromptButton::ok(t(L10nKey::EditorDiscard)),
            ],
            cx,
        );
        cx.spawn_in(window, async move |app, cx| {
            let Ok(choice) = answer.await else { return };
            let _ = app.update_in(cx, |app, window, cx| match choice {
                0 => app.editor_save_file(id, true, false, window, cx),
                2 => app.editor_drop_buffer(id, cx),
                _ => {}
            });
        })
        .detach();
    }

    /// Closes the file with this buffer in the front tab's strip, wherever
    /// it has moved to since a menu was built for it.
    pub(super) fn editor_close_buffer(&mut self, id: BufferId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pos) = self
            .tab_code()
            .and_then(|c| c.files.iter().position(|f| *f == id))
        {
            self.editor_close_file(pos, window, cx);
        }
    }

    /// Closes several files in the front tab's strip, asking once about the
    /// ones whose edits would be lost with them — not those another tab still
    /// shows.
    pub(super) fn editor_close_buffers(
        &mut self,
        ids: Vec<BufferId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab_id() else {
            return;
        };
        let unsaved: Vec<BufferId> = ids
            .iter()
            .copied()
            .filter(|id| self.buffer(*id).is_some_and(|f| f.dirty) && self.buffer_refs(*id) == 1)
            .collect();
        if self.editor_guard_unsaved(
            unsaved,
            AfterUnsaved::CloseFiles(tab, ids.clone()),
            window,
            cx,
        ) {
            return;
        }
        for id in ids {
            self.editor_remove_from_tab(tab, id, cx);
        }
    }

    /// Types `@path` into the running agent's prompt, with the selected
    /// lines after it when there are any.
    fn editor_attach_to_agent(&mut self, id: BufferId, cx: &mut Context<Self>) {
        let Some(f) = self.buffer(id).filter(|f| f.untitled.is_none()) else {
            return;
        };
        let state = f.input.read(cx);
        let suffix = match selected_lines(state.text(), state.selected_range()) {
            Some((a, b)) if a == b => format!("#L{a}"),
            Some((a, b)) => format!("#L{a}-{b}"),
            None => String::new(),
        };
        let path = f.path.clone();
        self.attach_path_to_agent(&path, &suffix, cx);
    }

    /// Right-click in the text. Edits dispatch to the editor itself, so each
    /// row shows the chord that does the same thing.
    pub(super) fn editor_body_menu(
        menu: PopupMenu,
        app: &gpui::WeakEntity<Self>,
        id: BufferId,
        cx: &gpui::App,
    ) -> PopupMenu {
        use gpui_kit::component::input as edit;
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let Some(f) = this.buffer(id) else {
            return menu;
        };
        let state = f.input.read(cx);
        let selected = !state.selected_range().is_empty();
        let mut menu = menu.min_w(px(220.)).action_context(state.focus_handle(cx));
        if f.untitled.is_none() {
            menu = menu
                .item(
                    PopupMenuItem::new(t(L10nKey::FileTreeContextAttachAgent)).on_click({
                        let app = app.clone();
                        move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| this.editor_attach_to_agent(id, cx));
                        }
                    }),
                )
                .separator();
        }
        let menu = menu
            .menu(t(L10nKey::AppMenuUndo), Box::new(edit::Undo))
            .menu(t(L10nKey::AppMenuRedo), Box::new(edit::Redo))
            .separator()
            .menu_with_disabled(t(L10nKey::AppMenuCut), Box::new(edit::Cut), !selected)
            .menu_with_disabled(t(L10nKey::AppMenuCopy), Box::new(edit::Copy), !selected)
            .menu_with_disabled(
                t(L10nKey::AppMenuPaste),
                Box::new(edit::Paste),
                cx.read_from_clipboard().is_none(),
            )
            .menu(t(L10nKey::AppMenuSelectAll), Box::new(edit::SelectAll))
            .separator()
            // The line commands act on every line a cursor or selection
            // touches. Their chords live on gpui-component's `CodeEditor` key
            // context and are re-added in `keymap::fixed_bindings`, so they
            // beat the app's own keys while the editor has focus.
            .menu(
                t(L10nKey::EditorToggleComment),
                Box::new(text_actions::ToggleLineComment),
            )
            .menu(t(L10nKey::EditorMoveLineUp), Box::new(text_actions::MoveLineUp))
            .menu(t(L10nKey::EditorMoveLineDown), Box::new(text_actions::MoveLineDown))
            .menu(
                t(L10nKey::EditorDuplicateLine),
                Box::new(text_actions::CopyLineDown),
            )
            .menu(t(L10nKey::EditorDeleteLine), Box::new(text_actions::DeleteLine))
            .separator()
            .menu(t(L10nKey::AppMenuFind), Box::new(edit::Search))
            .menu(
                t(L10nKey::EditorGoToLineAction),
                Box::new(anterm_ui_kit::actions::EditorGoToLine),
            )
            .menu_with_disabled(
                t(L10nKey::EditorGitRevertChange),
                Box::new(anterm_ui_kit::actions::EditorRevertChange),
                !this.editor_gutter_can_revert(id, cx),
            )
            .menu(
                t(L10nKey::EditorGoToMatchingBracket),
                Box::new(text_actions::MoveToMatchingBracket),
            );
        let menu = this.lsp_menu_items(menu, id, cx);
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// Right-click on a file in the header's strip.
    pub(super) fn editor_tab_menu(
        menu: PopupMenu,
        app: &gpui::WeakEntity<Self>,
        pos: usize,
        cx: &gpui::App,
    ) -> PopupMenu {
        let Some(this) = app.upgrade() else {
            return menu;
        };
        let this = this.read(cx);
        let Some(files) = this.tab_code().map(|c| c.files.clone()) else {
            return menu;
        };
        let Some(&id) = files.get(pos) else {
            return menu;
        };
        let others: Vec<BufferId> = files.iter().copied().filter(|f| *f != id).collect();
        let right = files[pos + 1..].to_vec();
        let close = |label: L10nKey, ids: Vec<BufferId>| {
            let app = app.clone();
            PopupMenuItem::new(t(label))
                .disabled(ids.is_empty())
                .on_click(move |_, window, cx| {
                    let ids = ids.clone();
                    let _ = app.update(cx, |this, cx| this.editor_close_buffers(ids, window, cx));
                })
        };
        let menu = menu
            .min_w(px(220.))
            .item(
                PopupMenuItem::new(t(L10nKey::TabContextCloseTab)).on_click({
                    let app = app.clone();
                    move |_, window, cx| {
                        let _ = app.update(cx, |this, cx| this.editor_close_buffer(id, window, cx));
                    }
                }),
            )
            .item(close(L10nKey::AppMenuCloseOtherTabs, others))
            .item(close(L10nKey::AppMenuCloseTabsRight, right));
        this.editor_file_menu_items(menu, id, app, cx)
    }

    /// What both menus offer for the file as a whole: open it outside anterm,
    /// show it in its folder, copy where it is. Nothing for a file that has
    /// never been saved, since it is not anywhere yet.
    fn editor_file_menu_items(
        &self,
        menu: PopupMenu,
        id: BufferId,
        app: &gpui::WeakEntity<Self>,
        cx: &gpui::App,
    ) -> PopupMenu {
        let Some(f) = self.buffer(id).filter(|f| f.untitled.is_none()) else {
            return menu;
        };
        let path = f.path.clone();
        // Only this machine's desktop can open or show a file, and only a
        // file that is on this machine — the same rule as the file tree.
        let local = f.host.id().is_local();
        let mut menu = menu.separator();
        let _ = cx;
        if local && !is_program(&path) {
            let browser = opens_in_browser(&path);
            let label = match browser {
                true => t(L10nKey::PanelOpenInBrowser),
                false => t(L10nKey::AppMenuOpenLinkWithDefaultApp),
            };
            let opener: fn(&Path) -> std::io::Result<()> = match browser {
                true => open_in_browser,
                false => anterm_ui_kit::platform::open::open_file_path,
            };
            menu = menu.item(PopupMenuItem::new(label).on_click({
                let app = app.clone();
                let path = path.clone();
                move |_, window, cx| {
                    let _ = app.update(cx, |_, cx| Self::open_with(&path, opener, window, cx));
                }
            }));
        }
        if local {
            menu = menu.item(
                PopupMenuItem::new(anterm_ui_kit::panel_tokens::reveal_label()).on_click({
                    let path = path.clone();
                    move |_, _window, cx| {
                        cx.reveal_path(&anterm_ui_kit::path_display::native_separators(&path));
                    }
                }),
            );
        }
        let copy = |label: L10nKey, text: String| {
            PopupMenuItem::new(t(label)).on_click(move |_, _window, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
            })
        };
        // A remote host's paths are already spelled its own way; only one on
        // this machine is re-spelled with its separators.
        let full = match local {
            true => anterm_ui_kit::path_display::native_separators(&path)
                .display()
                .to_string(),
            false => path.display().to_string(),
        };
        menu = menu.item(copy(L10nKey::FileTreeContextCopyPath, full));
        if let Some(rel) = self.path_under_tree_root(&path, cx) {
            let rel = match local {
                true => anterm_ui_kit::path_display::native_separators(&rel)
                    .display()
                    .to_string(),
                false => rel.display().to_string(),
            };
            menu = menu.item(copy(L10nKey::EditorCopyRelativePath, rel));
        }
        menu
    }

    pub fn editor_close_active_if_focused(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let empty = self
            .tab_code()
            .is_some_and(|c| c.visible && c.files.is_empty());
        // ⌘⇧E on an empty panel hands the focus to the file tree, so from
        // there ⌘W is the empty panel's too.
        let tree = empty && self.file_tree_contains_focus(window, cx);
        if !(tree || self.editor_panel_has_focus(window, cx)) {
            return false;
        }
        let Some(code) = self.tab_code_mut() else {
            return false;
        };
        if code.files.is_empty() {
            code.visible = false;
            self.editor.had_focus = None;
            self.editor.bar = None;
            self.file_tree_cancel_edit(cx);
            self.focus_active(window, cx);
            cx.notify();
            return true;
        }
        let active = code.active;
        self.editor_close_file(active, window, cx);
        true
    }

    /// Takes a buffer out of one tab's strip, and drops it once no tab shows
    /// it any more.
    pub(super) fn editor_remove_from_tab(&mut self, tab: TabId, id: BufferId, cx: &mut Context<Self>) {
        if let Some(code) = self
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.code.as_deref_mut())
        {
            code.close_in_focused(id);
        }
        if self.buffer_refs(id) == 0 {
            self.editor_drop_buffer(id, cx);
        }
        cx.notify();
    }

    /// Forgets a buffer everywhere, edits and all.
    pub(super) fn editor_drop_buffer(&mut self, id: BufferId, cx: &mut Context<Self>) {
        // Whatever was waiting on this buffer to be saved is not going to
        // see that happen now.
        self.editor_saves_failed(id);
        for code in self.tabs.values_mut().filter_map(|t| t.code.as_deref_mut()) {
            code.forget_everywhere(id);
        }
        self.editor.buffers.retain(|b| b.id() != id);
        self.editor.nav.forget_buffer(id);
        if matches!(
            self.editor.bar.as_ref().map(|b| &b.kind),
            Some(BarKind::SaveAs { id: bar_id, .. }) if *bar_id == id
        ) {
            self.editor.bar = None;
        }
        self.editor_rebuild_watcher(cx);
        cx.notify();
    }

}
