//! Helix modal key handling and editor selection synchronization.

use gpui::{Context, EntityInputHandler as _, Focusable as _, Window, px};
use gpui_kit::component::input::{Position, RopeExt as _};

use super::{BarKind, BufferId, TabCode};
use crate::{CodeEditorEvent, CodeEditorWorkspace};

impl CodeEditorWorkspace {
    /// Synchronize the buffer's `InputState` cursor and selection ranges with `f.helix`.
    pub fn editor_sync_helix_selection(
        &mut self,
        id: BufferId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.buffer(id) else {
            return;
        };
        let input = f.input.clone();
        let mode = f.helix.mode;
        let anchor = f.helix.anchor;
        let head = f.helix.head;
        let extras = f.helix.extra_selections.clone();
        input.update(cx, |state, cx| {
            let text = state.text().clone();
            let len = text.len();
            let a = anchor.min(len);
            let h = head.min(len);
            let pt = text.offset_to_point(h);
            let line_str = text.slice_line(pt.row).to_string();
            let col_bytes = pt.column.min(line_str.len());
            let character = line_str[..col_bytes].chars().count() as u32;
            state.set_cursor_position(Position::new(pt.row as u32, character), window, cx);

            let primary = a.min(h)..a.max(h);
            if mode == anterm_editor::helix::HelixMode::Insert
                && extras.is_empty()
                && primary.is_empty()
            {
                return;
            }
            state.set_selected_range(primary, cx);
            for (ea, eh) in extras {
                let r = ea.min(len).min(eh.min(len))..ea.min(len).max(eh.min(len));
                state.add_selection(r, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn editor_handle_helix_key(
        &mut self,
        ev: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use anterm_editor::helix::{
            HelixHostCommand, HelixMode, HelixOutcome, MinorMode, SurfaceKind, ViewAlign,
        };
        if !cx.global::<anterm_ui_kit::config::Config>().helix_mode {
            return;
        }
        // Let the Go-to-Line / Rename / SaveAs / Command / Search bar handle its own keys.
        if self.editor.bar.is_some() {
            return;
        }
        let Some(id) = self.tab_code().and_then(TabCode::active_id) else {
            return;
        };
        let Some(f) = self.buffer(id) else {
            return;
        };
        // Read-only Markdown Preview navigation when `f.preview` is active.
        if f.preview {
            let ks = &ev.keystroke;
            if ks.modifiers.platform {
                return;
            }
            let scroll = f.preview_scroll.clone();
            let key = ks.key.as_str();
            let minor = f.helix.minor;
            if key == "escape" {
                if let Some(f) = self.buffer_mut(id) {
                    f.helix.clear_pending();
                }
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
            if ks.modifiers.control {
                match key {
                    "d" => {
                        let off = scroll.offset();
                        scroll.set_offset(gpui::point(off.x, (off.y - px(240.)).min(px(0.))));
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "u" => {
                        let off = scroll.offset();
                        scroll.set_offset(gpui::point(off.x, (off.y + px(240.)).min(px(0.))));
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
                return;
            }
            let ch = ks.key_char.as_deref().unwrap_or(key);
            if minor == Some(MinorMode::Goto) {
                if let Some(f) = self.buffer_mut(id) {
                    f.helix.clear_pending();
                }
                match ch {
                    "g" => scroll.set_offset(gpui::point(px(0.), px(0.))),
                    "e" => {
                        let max = scroll.max_offset();
                        scroll.set_offset(gpui::point(px(0.), -max.y));
                    }
                    "n" => self.editor_cycle_buffer(true, window, cx),
                    "p" => self.editor_cycle_buffer(false, window, cx),
                    _ => {}
                }
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
            match ch {
                "j" | "down" => {
                    let off = scroll.offset();
                    scroll.set_offset(gpui::point(off.x, (off.y - px(48.)).min(px(0.))));
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
                "k" | "up" => {
                    let off = scroll.offset();
                    scroll.set_offset(gpui::point(off.x, (off.y + px(48.)).min(px(0.))));
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
                "g" => {
                    if let Some(f) = self.buffer_mut(id) {
                        f.helix.minor = Some(MinorMode::Goto);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
                "G" => {
                    let max = scroll.max_offset();
                    scroll.set_offset(gpui::point(px(0.), -max.y));
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
                "i" | "v" => {
                    let to_insert = ch == "i";
                    if let Some(f) = self.buffer_mut(id) {
                        f.preview = false;
                        f.helix.mode = if to_insert {
                            HelixMode::Insert
                        } else {
                            HelixMode::Normal
                        };
                    }
                    self.update_config(cx, |cfg| cfg.editor_markdown_preview = false);
                    self.focus_editor(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
                ":" => {
                    self.editor_open_bar(BarKind::Command, String::new(), window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }

        // Only intercept when the active buffer's Input has focus.
        if !f
            .input
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            return;
        }

        let clipboard_text = cx.read_from_clipboard().and_then(|item| item.text());
        let outcome = {
            let (text, cursor, selected) = {
                let state = f.input.read(cx);
                (
                    state.text().to_string(),
                    state.cursor(),
                    state.selected_range(),
                )
            };
            let Some(f) = self.buffer_mut(id) else {
                return;
            };
            // When in Insert mode (or if selection was changed by mouse/external command
            // while no Helix selection is active), sync `anchor`/`head` from `InputState`.
            if f.helix.mode == HelixMode::Insert
                || (f.helix.anchor == f.helix.head && selected.is_empty())
            {
                f.helix.sync_from_editor(cursor, selected, &text);
            }
            f.helix.handle_keystroke(
                anterm_ui_kit::helix_bridge::key_input(&ev.keystroke),
                &text,
                SurfaceKind::CodeEditor,
                clipboard_text.as_deref(),
            )
        };

        if matches!(outcome, HelixOutcome::Ignored) {
            return;
        }
        window.prevent_default();
        cx.stop_propagation();

        match outcome {
            HelixOutcome::Ignored => {}
            HelixOutcome::Handled => {
                self.editor_sync_helix_selection(id, window, cx);
            }
            HelixOutcome::Yank { clipboard } => {
                if let Some(clip) = clipboard {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(clip));
                }
                self.editor_sync_helix_selection(id, window, cx);
            }
            HelixOutcome::Edit {
                range,
                new_text,
                new_anchor,
                new_head,
                yank_to_clipboard,
            } => {
                if let Some(clip) = yank_to_clipboard {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(clip));
                }
                if let Some(f) = self.buffer(id) {
                    let input = f.input.clone();
                    let old_text = input.read(cx).text().to_string();
                    let s = range.start.min(old_text.len());
                    let e = range.end.min(old_text.len());
                    let start16 = old_text[..s].encode_utf16().count();
                    let end16 = start16 + old_text[s..e].encode_utf16().count();
                    input.update(cx, |state, cx| {
                        state.replace_text_in_range(Some(start16..end16), &new_text, window, cx);
                    });
                }
                if let Some(f) = self.buffer_mut(id) {
                    f.helix.anchor = new_anchor;
                    f.helix.head = new_head;
                }
                self.editor_sync_helix_selection(id, window, cx);
            }
            HelixOutcome::Host(cmd) => match cmd {
                HelixHostCommand::Undo => {
                    self.editor_dispatch(&gpui_kit::component::input::Undo, window, cx);
                    self.editor_resync_helix_from_input(id, cx);
                }
                HelixHostCommand::Redo => {
                    self.editor_dispatch(&gpui_kit::component::input::Redo, window, cx);
                    self.editor_resync_helix_from_input(id, cx);
                }
                HelixHostCommand::ToggleComment => {
                    self.editor_toggle_line_comment(window, cx);
                    self.editor_resync_helix_from_input(id, cx);
                }
                HelixHostCommand::AlignView(align) => {
                    if let Some(f) = self.buffer(id) {
                        let input = f.input.clone();
                        input.update(cx, |state, cx| {
                            let pos = state.cursor_position();
                            let lines = state.text().lines_len().max(1) as u32;
                            let target_line = match align {
                                ViewAlign::Center => pos.line,
                                ViewAlign::Top => (pos.line + 8).min(lines - 1),
                                ViewAlign::Bottom => pos.line.saturating_sub(8),
                            };
                            state.set_cursor_position(
                                Position::new(target_line, pos.character),
                                window,
                                cx,
                            );
                            state.set_cursor_position(pos, window, cx);
                        });
                    }
                    cx.notify();
                }
                HelixHostCommand::ScrollPages { half_pages } => {
                    if let Some(f) = self.buffer_mut(id) {
                        let text = f.input.read(cx).text().to_string();
                        let delta_lines = half_pages * 15;
                        let next =
                            anterm_editor::helix::move_vertical(&text, f.helix.head, delta_lines);
                        f.helix.move_cursor(&text, next);
                    }
                    self.editor_sync_helix_selection(id, window, cx);
                }
                HelixHostCommand::GoToDefinition => {
                    self.lsp_go_to_definition(window, cx);
                }
                HelixHostCommand::FindReferences => {
                    self.lsp_find_references(window, cx);
                }
                HelixHostCommand::CycleBuffer { next } => {
                    self.editor_cycle_buffer(next, window, cx);
                }
                HelixHostCommand::StepGitChange { next } => {
                    self.editor_gutter_step(next, window, cx);
                    self.editor_resync_helix_from_input(id, cx);
                }
                HelixHostCommand::JumpHistory { forward } => {
                    self.editor_navigate(forward, window, cx);
                }
                HelixHostCommand::QuickOpenFile => {
                    self.open_search(anterm_ui_kit::search::SearchTab::Files, "", window, cx);
                }
                HelixHostCommand::GlobalSearch => {
                    self.focus_global_search(cx);
                }
                HelixHostCommand::GoToSymbol => {
                    self.editor_go_to_symbol(window, cx);
                }
                HelixHostCommand::RenameSymbol => {
                    self.lsp_rename_start(window, cx);
                }
                HelixHostCommand::CodeActions => {
                    self.lsp_code_actions(window, cx);
                }
                HelixHostCommand::ToggleDiagnostics => {
                    self.toggle_editor_problems(cx);
                }
                HelixHostCommand::WindowFocusOther => {
                    let focus_left = self.tab_code().is_none_or(TabCode::focus_is_left);
                    if !self.editor_focus_group(!focus_left, window, cx) {
                        cx.emit(CodeEditorEvent::CyclePane);
                    }
                }
                HelixHostCommand::WindowSplitRight => {
                    self.editor_split(window, cx);
                }
                HelixHostCommand::WindowCloseActive => {
                    self.editor_close_buffer(id, window, cx);
                }
                HelixHostCommand::OpenCommandBar => {
                    self.editor_open_bar(BarKind::Command, String::new(), window, cx);
                }
                HelixHostCommand::OpenSearchBar => {
                    self.editor_open_bar(BarKind::HelixSearch, String::new(), window, cx);
                }
                HelixHostCommand::OpenSplitSelectionBar => {
                    self.editor_open_bar(BarKind::HelixSplitSelect, String::new(), window, cx);
                }
                HelixHostCommand::Submit | HelixHostCommand::DismissSurface => {}
            },
        }
    }

    pub(super) fn editor_resync_helix_from_input(&mut self, id: BufferId, cx: &mut Context<Self>) {
        let Some(f) = self.buffer_mut(id) else {
            return;
        };
        let (text, cursor, selected) = {
            let state = f.input.read(cx);
            (
                state.text().to_string(),
                state.cursor(),
                state.selected_range(),
            )
        };
        f.helix.sync_from_editor(cursor, selected, &text);
        cx.notify();
    }

}
