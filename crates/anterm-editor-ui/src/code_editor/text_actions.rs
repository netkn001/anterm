//! Code-editor line, selection, and text-transform actions owned by `anterm-app`
//! on top of upstream `gpui-kit`'s [`EditorState`].

use gpui::{Context, EntityInputHandler as _, Window};
use gpui_kit::component::input::EditorState;

use crate::CodeEditorWorkspace;

/// Key context applied to the code editor container so editor-only shortcuts
/// outrank app-wide chords without affecting single-line inputs or terminals.
pub use anterm_ui_kit::actions::CODE_EDITOR_CONTEXT;
pub use anterm_ui_kit::actions::editor_text_actions::*;

fn byte_range_to_utf16(text: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let start = text[..range.start].encode_utf16().count();
    let end = start + text[range].encode_utf16().count();
    start..end
}

fn transform_selection(
    state: &mut EditorState,
    window: &mut Window,
    cx: &mut Context<EditorState>,
    f: impl FnOnce(&str) -> String,
) {
    let text = state.value().to_string();
    let sel = state.selected_range();
    if sel.is_empty() || sel.end > text.len() {
        return;
    }
    let replacement = f(&text[sel.clone()]);
    if replacement == text[sel.clone()] {
        return;
    }
    let utf16 = byte_range_to_utf16(&text, sel);
    state.replace_text_in_range(Some(utf16), &replacement, window, cx);
}

fn comment_prefix(language: &str) -> &'static str {
    match language {
        "python" | "ruby" | "bash" | "shell" | "sh" | "yaml" | "toml" | "make" | "cmake"
        | "elixir" => "# ",
        "lua" | "sql" => "-- ",
        "html" | "xml" => "<!-- ",
        _ => "// ",
    }
}

impl CodeEditorWorkspace {
    pub fn editor_transform_uppercase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |s, cx| transform_selection(s, window, cx, str::to_uppercase));
    }

    pub fn editor_transform_lowercase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |s, cx| transform_selection(s, window, cx, str::to_lowercase));
    }

    pub fn editor_transform_title_case(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |s, cx| {
            transform_selection(s, window, cx, |src| {
                let mut out = String::with_capacity(src.len());
                let mut cap = true;
                for ch in src.chars() {
                    if ch.is_alphabetic() {
                        if cap {
                            for u in ch.to_uppercase() {
                                out.push(u);
                            }
                            cap = false;
                        } else {
                            for l in ch.to_lowercase() {
                                out.push(l);
                            }
                        }
                    } else {
                        cap = true;
                        out.push(ch);
                    }
                }
                out
            })
        });
    }

    pub fn editor_trim_trailing_whitespace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let trimmed: String = text
                .split_inclusive('\n')
                .map(|line| {
                    if let Some(body) = line.strip_suffix('\n') {
                        format!("{}\n", body.trim_end_matches([' ', '\t', '\r']))
                    } else {
                        line.trim_end_matches([' ', '\t', '\r']).to_string()
                    }
                })
                .collect();
            if trimmed != text {
                let utf16 = byte_range_to_utf16(&text, 0..text.len());
                state.replace_text_in_range(Some(utf16), &trimmed, window, cx);
            }
        });
    }

    pub fn editor_join_lines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let sel = state.selected_range();
            let Some((range, joined)) = anterm_editor::helix::join_lines_in_range(&text, sel)
            else {
                return;
            };
            let utf16 = byte_range_to_utf16(&text, range);
            state.replace_text_in_range(Some(utf16), &joined, window, cx);
        });
    }

    pub fn editor_remove_surrounding_brackets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let cursor = state.cursor();
            for (open_ch, close_ch) in [
                ('(', ')'),
                ('[', ']'),
                ('{', '}'),
                ('<', '>'),
                ('"', '"'),
                ('\'', '\''),
                ('`', '`'),
            ] {
                if let Some((open, close)) =
                    anterm_editor::helix::find_enclosing_pair(&text, cursor, open_ch, close_ch)
                {
                    let inner = text[open.end..close.start].to_string();
                    let utf16 = byte_range_to_utf16(&text, open.start..close.end);
                    state.replace_text_in_range(Some(utf16), &inner, window, cx);
                    return;
                }
            }
        });
    }

    pub fn editor_toggle_line_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, lang)) = self
            .active_buffer()
            .map(|f| (f.input.clone(), f.language().to_string()))
        else {
            return;
        };
        let prefix = comment_prefix(&lang);
        let prefix_trimmed = prefix.trim_end();
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let sel = state.selected_range();
            let start = anterm_editor::helix::line_start_offset(&text, sel.start);
            let end = anterm_editor::helix::line_end_offset(&text, sel.end);
            let block = &text[start..end];
            let all_commented = block
                .lines()
                .filter(|l| !l.trim().is_empty())
                .all(|l| l.trim_start().starts_with(prefix_trimmed));
            let replaced: String = block
                .split_inclusive('\n')
                .map(|line| {
                    if all_commented {
                        let indent_len = line.len() - line.trim_start().len();
                        let rest = &line[indent_len..];
                        if let Some(stripped) = rest
                            .strip_prefix(prefix)
                            .or_else(|| rest.strip_prefix(prefix_trimmed))
                        {
                            return format!("{}{}", &line[..indent_len], stripped);
                        }
                        line.to_string()
                    } else if line.trim().is_empty() {
                        line.to_string()
                    } else {
                        format!("{prefix}{line}")
                    }
                })
                .collect();
            let utf16 = byte_range_to_utf16(&text, start..end);
            state.replace_text_in_range(Some(utf16), &replaced, window, cx);
        });
    }

    pub fn editor_delete_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let sel = state.selected_range();
            let start = anterm_editor::helix::line_start_offset(&text, sel.start);
            let mut end = anterm_editor::helix::line_end_offset(&text, sel.end);
            if end < text.len() && text.as_bytes()[end] == b'\n' {
                end += 1;
            }
            let utf16 = byte_range_to_utf16(&text, start..end);
            state.replace_text_in_range(Some(utf16), "", window, cx);
        });
    }

    pub fn editor_select_line(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let sel = state.selected_range();
            let (anchor, head) =
                anterm_editor::helix::extend_line_selection(&text, sel.start, sel.end, 1);
            state.set_selected_range(anchor.min(head)..anchor.max(head), cx);
        });
    }

    pub fn editor_move_to_matching_bracket(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let cursor = state.cursor();
            if let Some(target) = anterm_editor::helix::find_matching_bracket(&text, cursor) {
                state.set_selected_range(target..target, cx);
            }
        });
    }

    pub fn editor_duplicate_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let sel = state.selected_range();
            let start = anterm_editor::helix::line_start_offset(&text, sel.start);
            let end = anterm_editor::helix::line_end_offset(&text, sel.end);
            let line = text[start..end].to_string();
            let insert = format!("\n{line}");
            let utf16 = byte_range_to_utf16(&text, end..end);
            state.replace_text_in_range(Some(utf16), &insert, window, cx);
        });
    }

    pub fn editor_move_line(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.active_buffer().map(|f| f.input.clone()) else {
            return;
        };
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let sel = state.selected_range();
            let start = anterm_editor::helix::line_start_offset(&text, sel.start);
            let end = anterm_editor::helix::line_end_offset(&text, sel.end);
            if !down {
                if start == 0 {
                    return;
                }
                let prev_end = start - 1;
                let prev_start = anterm_editor::helix::line_start_offset(&text, prev_end);
                let prev = &text[prev_start..prev_end];
                let curr = &text[start..end];
                let swapped = format!("{curr}\n{prev}");
                let utf16 = byte_range_to_utf16(&text, prev_start..end);
                state.replace_text_in_range(Some(utf16), &swapped, window, cx);
            } else {
                if end >= text.len() {
                    return;
                }
                let next_start = end + 1;
                let next_end = anterm_editor::helix::line_end_offset(&text, next_start);
                let curr = &text[start..end];
                let next = &text[next_start..next_end];
                let swapped = format!("{next}\n{curr}");
                let utf16 = byte_range_to_utf16(&text, start..next_end);
                state.replace_text_in_range(Some(utf16), &swapped, window, cx);
            }
        });
    }
}
