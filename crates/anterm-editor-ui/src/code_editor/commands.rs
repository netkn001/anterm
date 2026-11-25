//! Editor panel visibility, focus, command dispatch, and inline prompt bars.

use std::path::PathBuf;

use anterm_editor::text::LineEnding;
use anterm_i18n::{L10nKey, t, t_fmt};
use gpui::{Context, Focusable as _, PromptLevel, Window, prelude::*};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState, Position, RopeExt as _};

use super::{
    BarKind, CURSOR_SCROLL_ATTEMPTS, EditorBar, TabCode, parse_line_target, place_cursor,
    text_actions,
};
use crate::document_column::OverlayTop;
use crate::{CodeEditorEvent, CodeEditorWorkspace};
use anterm_ui_kit::host_ops::HostOps;

impl CodeEditorWorkspace {
    pub fn toggle_code_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab_doc_mut() else {
            return;
        };
        let buried = tab.overlay_top == OverlayTop::Diff
            && tab.diff_overlay.is_some()
            && tab.code.as_ref().is_some_and(|c| c.visible);
        tab.overlay_top = OverlayTop::Code;
        if buried {
            self.focus_editor(window, cx);
            cx.notify();
            return;
        }
        let code = tab.code.get_or_insert_with(|| Box::new(TabCode::new()));
        if code.visible {
            code.visible = false;
            self.editor.had_focus = None;
            self.editor.bar = None;
            self.file_tree_cancel_edit(cx);
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        code.visible = true;
        let has_buffer = self.active_buffer().is_some();
        if has_buffer {
            self.focus_editor(window, cx);
        }
        cx.emit(CodeEditorEvent::CodePanelShown { empty: !has_buffer });
        cx.notify();
    }

    pub(super) fn raise_code_overlay(&mut self) {
        if let Some(tab) = self.active_tab_doc_mut() {
            tab.overlay_top = OverlayTop::Code;
        }
    }

    pub(super) fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.active_buffer() {
            if f.preview {
                self.editor.empty_focus.focus(window, cx);
            } else {
                f.input.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }

    /// Brings the file at `pos` in the active tab's strip to the front.
    pub fn editor_activate(
        &mut self,
        pos: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(code) = self.tab_code_mut() else {
            return;
        };
        if pos >= code.files.len() {
            return;
        }
        code.active = pos;
        self.editor.bar = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// The status bar's Preview / Edit button, and `ToggleDocumentPreview`.
    /// Acts on the active tab's open file, and only when that file is
    /// Markdown and the code panel is on screen — anything else has no
    /// rendered form to switch to. The new state is remembered for the next
    /// Markdown file opened.
    pub fn toggle_document_preview(&mut self, cx: &mut Context<Self>) {
        if !self.code_panel_visible() {
            return;
        }
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        if f.language() != "markdown" {
            return;
        }
        f.preview = !f.preview;
        let preview = f.preview;
        self.update_config(cx, |cfg| cfg.editor_markdown_preview = preview);
        cx.notify();
    }

    /// The status bar's Wrap button, and `ToggleDocumentWrap`. Same reach as
    /// [`Self::toggle_document_preview`], for any file; the new state becomes
    /// what the next file opens with.
    pub fn toggle_document_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.code_panel_visible() {
            return;
        }
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        f.wrap = !f.wrap;
        let wrap = f.wrap;
        f.input.clone().update(cx, |st, cx| {
            st.set_soft_wrap(wrap, window, cx);
        });
        self.update_config(cx, |cfg| cfg.editor_soft_wrap = wrap);
        cx.notify();
    }

    /// The status bar's line-ending button: flips the file between LF and
    /// CRLF. The buffer itself always holds `\n`; this only changes what a
    /// save writes, so it marks the file unsaved without touching its text.
    pub(super) fn toggle_line_ending(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        f.format.line_ending = match f.format.line_ending {
            LineEnding::Lf => LineEnding::CrLf,
            LineEnding::CrLf => LineEnding::Lf,
        };
        self.editor_note_edit(id, cx);
    }

    /// Run one of gpui-component's editing actions on the active buffer, as
    /// if it had been pressed there. Returns false with no buffer to run it
    /// on. The palette closes before its row runs, so this focuses the
    /// buffer first rather than relying on where the focus happens to be.
    pub fn editor_dispatch(
        &mut self,
        action: &dyn gpui::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.code_panel_visible() {
            return false;
        }
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return false;
        };
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        handle.dispatch_action(action, window, cx);
        true
    }

    /// The window's listeners for the editor's text commands, when the user
    /// has bound one: each runs on the editor only while it has the focus.
    pub fn with_editor_text_commands(
        root: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        use anterm_ui_kit::actions::{
            EditorJoinLines, EditorRemoveSurroundingBrackets, EditorTransformLowercase,
            EditorTransformTitleCase, EditorTransformUppercase, EditorTrimTrailingWhitespace,
        };
        use text_actions::{
            CopyLineDown, CopyLineUp, DeleteLine, JoinLines, MoveLineDown, MoveLineUp,
            MoveToMatchingBracket, RemoveSurroundingBrackets, SelectLine, ToggleLineComment,
            TransformToLowercase, TransformToTitleCase, TransformToUppercase,
            TrimTrailingWhitespace,
        };
        root.on_action(
            cx.listener(|this, _: &EditorTransformUppercase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_transform_uppercase(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &TransformToUppercase, window, cx| {
                this.editor_transform_uppercase(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTransformLowercase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_transform_lowercase(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &TransformToLowercase, window, cx| {
                this.editor_transform_lowercase(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTransformTitleCase, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_transform_title_case(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &TransformToTitleCase, window, cx| {
                this.editor_transform_title_case(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &EditorTrimTrailingWhitespace, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_trim_trailing_whitespace(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &TrimTrailingWhitespace, window, cx| {
                this.editor_trim_trailing_whitespace(window, cx);
            }),
        )
        .on_action(cx.listener(|this, _: &EditorJoinLines, window, cx| {
            if !this.editor_has_focus(window, cx) {
                cx.propagate();
                return;
            }
            this.editor_join_lines(window, cx);
        }))
        .on_action(cx.listener(|this, _: &JoinLines, window, cx| {
            this.editor_join_lines(window, cx);
        }))
        .on_action(
            cx.listener(|this, _: &EditorRemoveSurroundingBrackets, window, cx| {
                if !this.editor_has_focus(window, cx) {
                    cx.propagate();
                    return;
                }
                this.editor_remove_surrounding_brackets(window, cx);
            }),
        )
        .on_action(
            cx.listener(|this, _: &RemoveSurroundingBrackets, window, cx| {
                this.editor_remove_surrounding_brackets(window, cx);
            }),
        )
        .on_action(cx.listener(|this, _: &ToggleLineComment, window, cx| {
            this.editor_toggle_line_comment(window, cx);
        }))
        .on_action(cx.listener(|this, _: &DeleteLine, window, cx| {
            this.editor_delete_line(window, cx);
        }))
        .on_action(cx.listener(|this, _: &SelectLine, _window, cx| {
            this.editor_select_line(cx);
        }))
        .on_action(cx.listener(|this, _: &MoveToMatchingBracket, _window, cx| {
            this.editor_move_to_matching_bracket(cx);
        }))
        .on_action(cx.listener(|this, _: &CopyLineDown, window, cx| {
            this.editor_duplicate_line(window, cx);
        }))
        .on_action(cx.listener(|this, _: &CopyLineUp, window, cx| {
            this.editor_duplicate_line(window, cx);
        }))
        .on_action(cx.listener(|this, _: &MoveLineUp, window, cx| {
            this.editor_move_line(false, window, cx);
        }))
        .on_action(cx.listener(|this, _: &MoveLineDown, window, cx| {
            this.editor_move_line(true, window, cx);
        }))
    }

    /// Whether an editor exists to run [`Self::editor_dispatch`] on.
    pub fn editor_can_dispatch(&self) -> bool {
        self.code_panel_visible() && self.active_buffer().is_some()
    }

    pub fn editor_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.code_panel_visible()
            && self.active_buffer().is_some_and(|f| {
                f.input
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
    }

    /// Whether anything in the editor panel — the text, its search bar, the
    /// go-to-line prompt — has the focus. Wider than
    /// [`Self::editor_has_focus`], for keys that belong to the panel as a
    /// whole: ⌘S from the find box should still save.
    pub fn editor_panel_has_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.editor_has_focus(window, cx)
            || (self.code_panel_visible() && self.editor.empty_focus.is_focused(window))
            || self.editor.bar.as_ref().is_some_and(|b| {
                b.input
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
    }

    pub(super) fn editor_open_bar(
        &mut self,
        kind: BarKind,
        initial: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            BarKind::GoToLine => {
                let total = self
                    .active_buffer()
                    .map(|f| f.input.read(cx).text().lines_len())
                    .unwrap_or(1);
                t_fmt(
                    L10nKey::EditorGoToLinePlaceholder,
                    &[("total", &total.to_string())],
                )
            }
            BarKind::SaveAs { .. } => t(L10nKey::EditorSaveAsPlaceholder).to_string(),
            BarKind::Rename { .. } => t_fmt(L10nKey::LspRenamePlaceholder, &[("name", &initial)]),
            BarKind::Command => ":w, :q, :wq, :q!, :vs, :120".to_string(),
            BarKind::HelixSearch => "Search…".to_string(),
            BarKind::HelixSplitSelect => "Split selection on pattern…".to_string(),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut CodeEditorWorkspace, _input, ev, window, cx| {
                match ev {
                    InputEvent::PressEnter { .. } => this.editor_submit_bar(window, cx),
                    // Clicking away from a go-to-line box is how you say never
                    // mind. A Save As is left up: it may be half-typed.
                    InputEvent::Blur => {
                        if matches!(
                            this.editor.bar.as_ref().map(|b| &b.kind),
                            Some(
                                BarKind::GoToLine
                                    | BarKind::Rename { .. }
                                    | BarKind::Command
                                    | BarKind::HelixSearch
                                    | BarKind::HelixSplitSelect
                            )
                        ) {
                            this.editor.bar = None;
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            },
        );
        input.update(cx, |st, cx| st.focus(window, cx));
        self.editor.bar = Some(EditorBar {
            kind,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    pub fn editor_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.code_panel_visible() || self.active_buffer().is_none() {
            return;
        }
        if let Some(f) = self.tab_code().and_then(TabCode::active_id)
            && let Some(f) = self.buffer_mut(f)
        {
            // There is no line to land on in the rendered view.
            f.preview = false;
        }
        self.editor_open_bar(BarKind::GoToLine, String::new(), window, cx);
    }

    pub(super) fn editor_close_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(BarKind::SaveAs { id, .. }) = self.editor.bar.as_ref().map(|b| &b.kind) {
            let id = *id;
            self.editor_saves_failed(id);
        }
        self.editor.bar = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn editor_submit_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.editor.bar.as_ref() else {
            return;
        };
        let text = bar.input.read(cx).value().to_string();
        match bar.kind {
            BarKind::GoToLine => {
                let Some((line, column)) = parse_line_target(&text) else {
                    return;
                };
                self.editor_close_bar(window, cx);
                let Some(f) = self.active_buffer() else {
                    return;
                };
                let input = f.input.clone();
                let lines = input.read(cx).text().lines_len().max(1) as u32;
                let position = Position {
                    line: line.min(lines) - 1,
                    character: column - 1,
                };
                place_cursor(input, position, CURSOR_SCROLL_ATTEMPTS, window, cx);
            }
            BarKind::Rename { id, offset } => {
                let name = text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                self.editor_close_bar(window, cx);
                self.lsp_rename(id, offset, name, window, cx);
            }
            BarKind::SaveAs { id, then_close } => {
                let path = PathBuf::from(text.trim());
                if path.as_os_str().is_empty() {
                    return;
                }
                let Some(host) = self.buffer(id).map(|f| f.host.clone()) else {
                    return;
                };
                if !host.is_absolute(&path) {
                    return;
                }
                self.editor.bar = None;
                cx.notify();
                let check = path.clone();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |h| h.exists(&check),
                    move |app, exists: bool, window, cx| {
                        if !exists {
                            app.editor_save_to(id, path, then_close, window, cx);
                            return;
                        }
                        let answer = window.prompt(
                            PromptLevel::Warning,
                            &t_fmt(
                                L10nKey::EditorReplaceExisting,
                                &[("path", &path.display().to_string())],
                            ),
                            None,
                            &anterm_ui_kit::confirm_answers(
                                t(L10nKey::EditorReplace),
                                t(L10nKey::Cancel),
                            ),
                            cx,
                        );
                        cx.spawn_in(window, async move |app, cx| {
                            if !matches!(answer.await, Ok(0)) {
                                return;
                            }
                            let _ = app.update_in(cx, |app, window, cx| {
                                app.editor_save_to(id, path, then_close, window, cx);
                            });
                        })
                        .detach();
                    },
                );
            }
            BarKind::Command => {
                self.editor_close_bar(window, cx);
                self.editor_exec_command_bar(&text, window, cx);
            }
            BarKind::HelixSearch => {
                self.editor_close_bar(window, cx);
                if text.is_empty() {
                    return;
                }
                let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
                    return;
                };
                let Some(f) = self.buffer_mut(id) else {
                    return;
                };
                let buf_text = f.input.read(cx).text().to_string();
                f.helix.search_query = text.clone();
                if let Some(m) =
                    anterm_editor::helix::find_next_match(&buf_text, &text, f.helix.head, true)
                {
                    f.helix.select_span(&buf_text, m.start, m.end);
                    self.editor_sync_helix_selection(id, window, cx);
                }
            }
            BarKind::HelixSplitSelect => {
                self.editor_close_bar(window, cx);
                if text.is_empty() {
                    return;
                }
                let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
                    return;
                };
                let Some(f) = self.buffer_mut(id) else {
                    return;
                };
                let buf_text = f.input.read(cx).text().to_string();
                let range = f.helix.effective_range(&buf_text);
                let splits = anterm_editor::helix::split_selection_on_pattern(&buf_text, range, &text);
                if let Some(last) = splits.last().cloned() {
                    f.helix.extra_selections = splits[..splits.len() - 1]
                        .iter()
                        .map(|r| (r.start, r.end))
                        .collect();
                    f.helix.anchor = last.start;
                    f.helix.head = last.end;
                    self.editor_sync_helix_selection(id, window, cx);
                }
            }
        }
    }

    fn editor_resolve_command_path(&self, path_str: &str, cx: &Context<Self>) -> PathBuf {
        let p = PathBuf::from(path_str);
        let Some(host) = self
            .active_buffer()
            .map(|f| f.host.clone())
            .or_else(|| self.active_host(cx))
        else {
            return p;
        };
        if host.is_absolute(&p) {
            p
        } else if let Some(parent) = self
            .active_buffer()
            .and_then(|f| f.path.parent().filter(|_| f.untitled.is_none()))
        {
            host.join(parent, path_str)
        } else if let Some(root) = self.active_tree_roots(cx).first() {
            host.join(root, path_str)
        } else {
            p
        }
    }

    fn editor_exec_command_bar(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) {
        use anterm_editor::helix::CommandBarAction;
        match anterm_editor::helix::parse_command_bar(raw) {
            CommandBarAction::Empty => {}
            CommandBarAction::GoToLine(line) => {
                let Some(f) = self.active_buffer() else {
                    return;
                };
                let input = f.input.clone();
                let lines = input.read(cx).text().lines_len().max(1) as u32;
                let position = Position {
                    line: (line.max(1) as u32).min(lines) - 1,
                    character: 0,
                };
                place_cursor(input, position, CURSOR_SCROLL_ATTEMPTS, window, cx);
                if let Some(id) = self.tab_code().and_then(TabCode::active_id)
                    && let Some(f) = self.buffer_mut(id)
                {
                    let text = f.input.read(cx).text().to_string();
                    let off = anterm_editor::helix::line_number_to_offset(&text, line);
                    f.helix.move_cursor(&text, off);
                }
            }
            CommandBarAction::Write(None) => {
                self.editor_save_active(window, cx);
            }
            CommandBarAction::Write(Some(path_str)) => {
                let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
                    return;
                };
                let resolved = self.editor_resolve_command_path(&path_str, cx);
                self.editor_save_to(id, resolved, false, window, cx);
            }
            CommandBarAction::Quit { force } => {
                let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
                    self.toggle_code_panel(window, cx);
                    return;
                };
                if force {
                    if let Some(tab) = self.active_tab_id() { self.editor_remove_from_tab(tab, id, cx); }
                    if self.buffer_refs(id) == 0 {
                        self.editor_drop_buffer(id, cx);
                    }
                } else {
                    self.editor_close_buffer(id, window, cx);
                }
            }
            CommandBarAction::WriteQuit { force, path } => {
                let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
                    return;
                };
                if let Some(path_str) = path {
                    let resolved = self.editor_resolve_command_path(&path_str, cx);
                    self.editor_save_to(id, resolved, true, window, cx);
                } else {
                    self.editor_save_file(id, true, force, window, cx);
                }
            }
            CommandBarAction::WriteAll => {
                for id in self.editor_unsaved() {
                    self.editor_save_file(id, false, false, window, cx);
                }
            }
            CommandBarAction::QuitAll { force } => {
                let Some(ids) = self.tab_code().map(TabCode::all_files) else {
                    return;
                };
                if force {
                    for id in ids {
                        self.editor_drop_buffer(id, cx);
                    }
                    if self.tab_code().is_some_and(|c| c.visible) {
                        self.toggle_code_panel(window, cx);
                    }
                } else {
                    self.editor_close_buffers(ids, window, cx);
                }
            }
            CommandBarAction::WriteQuitAll => {
                let ids = self.editor_unsaved();
                for id in ids {
                    self.editor_save_file(id, true, true, window, cx);
                }
                if let Some(clean_ids) = self.tab_code().map(TabCode::all_files) {
                    for id in clean_ids {
                        if self.buffer(id).is_some_and(|b| !b.dirty) {
                            if let Some(tab) = self.active_tab_id() { self.editor_remove_from_tab(tab, id, cx); }
                        }
                    }
                }
            }
            CommandBarAction::Reload => {
                if let Some(id) = self.tab_code().and_then(TabCode::active_id) {
                    self.editor_reload_from_disk(id, window, cx);
                }
            }
            CommandBarAction::Edit(path_str) => {
                let resolved = self.editor_resolve_command_path(&path_str, cx);
                self.open_file_in_editor(&resolved, window, cx);
            }
            CommandBarAction::VSplit(path_opt) => {
                self.editor_split(window, cx);
                if let Some(path_str) = path_opt {
                    let resolved = self.editor_resolve_command_path(&path_str, cx);
                    self.open_file_in_editor(&resolved, window, cx);
                }
            }
            CommandBarAction::Unknown(cmd) => {
                window.push_notification(format!("Unknown command: :{cmd}"), cx);
            }
        }
    }

}
