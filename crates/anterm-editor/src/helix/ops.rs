use std::ops::Range;

use super::motion::{
    clamp_char_boundary, line_end_including_newline, line_end_offset, line_start_offset,
    move_vertical,
};
use super::{HelixMode, HelixOutcome, HelixState};

// ---- Pure Text Transforms (`>`, `<`, `J`, `~`, `` ` ``, `ms`, `md`, `mr`) --

pub fn indent_lines(
    text: &str,
    range: Range<usize>,
    count: usize,
    indent_unit: &str,
) -> (Range<usize>, String) {
    let start_line = line_start_offset(text, range.start);
    let end_probe = if range.end > range.start && text.as_bytes().get(range.end - 1) == Some(&b'\n')
    {
        range.end - 1
    } else {
        range.end
    };
    let end_line = line_end_offset(text, end_probe);
    let prefix = indent_unit.repeat(count.max(1));
    let slice = &text[start_line..end_line];
    let mut out = String::new();
    for (i, line) in slice.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if !line.trim().is_empty() {
            out.push_str(&prefix);
        }
        out.push_str(line);
    }
    (start_line..end_line, out)
}

pub fn outdent_lines(
    text: &str,
    range: Range<usize>,
    count: usize,
    tab_width: usize,
) -> (Range<usize>, String) {
    let start_line = line_start_offset(text, range.start);
    let end_probe = if range.end > range.start && text.as_bytes().get(range.end - 1) == Some(&b'\n')
    {
        range.end - 1
    } else {
        range.end
    };
    let end_line = line_end_offset(text, end_probe);
    let max_spaces = count.max(1) * tab_width.max(1);
    let slice = &text[start_line..end_line];
    let mut out = String::new();
    for (i, line) in slice.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut removed = 0usize;
        let mut byte_cut = 0usize;
        for (rel, ch) in line.char_indices() {
            if removed >= max_spaces {
                break;
            }
            if ch == ' ' {
                removed += 1;
                byte_cut = rel + 1;
            } else if ch == '\t' {
                removed += tab_width.max(1);
                byte_cut = rel + 1;
            } else {
                break;
            }
        }
        out.push_str(&line[byte_cut..]);
    }
    (start_line..end_line, out)
}

pub fn join_lines_in_range(text: &str, range: Range<usize>) -> Option<(Range<usize>, String)> {
    let start_line = line_start_offset(text, range.start);
    let end_line_with_nl = line_end_including_newline(text, range.end.max(range.start));
    let target_end = if text[start_line..range.end.max(range.start)].contains('\n') {
        line_end_offset(text, range.end)
    } else if end_line_with_nl < text.len() {
        line_end_offset(text, end_line_with_nl)
    } else {
        return None;
    };
    let slice = &text[start_line..target_end];
    if !slice.contains('\n') {
        return None;
    }
    let mut out = String::new();
    for (i, line) in slice.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if i == 0 {
            out.push_str(line.trim_end());
        } else {
            let trimmed = line.trim_start();
            if !out.is_empty() && !trimmed.is_empty() {
                out.push(' ');
            }
            out.push_str(trimmed);
        }
    }
    Some((start_line..target_end, out))
}

pub fn toggle_case_str(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_uppercase() {
                c.to_lowercase().collect::<Vec<_>>()
            } else if c.is_lowercase() {
                c.to_uppercase().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

// ---- Search & Multi-Cursor Helpers (`/`, `*`, `n`, `N`, `s`, `C`) ----------

/// Find the next (`forward == true`) or previous (`forward == false`) match of
/// `query` in `text`, using smartcase (case-insensitive unless `query` contains
/// an uppercase letter) and wrapping around the buffer.
pub fn find_next_match(
    text: &str,
    query: &str,
    from_offset: usize,
    forward: bool,
) -> Option<Range<usize>> {
    if query.is_empty() || text.is_empty() {
        return None;
    }
    let case_sensitive = query.chars().any(char::is_uppercase);
    let matches: Vec<Range<usize>> = if case_sensitive {
        text.match_indices(query)
            .map(|(i, m)| i..(i + m.len()))
            .collect()
    } else {
        let pat = format!("(?i){}", regex::escape(query));
        let re = regex::Regex::new(&pat).ok()?;
        re.find_iter(text).map(|m| m.start()..m.end()).collect()
    };
    if matches.is_empty() {
        return None;
    }
    let o = clamp_char_boundary(text, from_offset);
    if forward {
        matches
            .iter()
            .find(|r| r.start > o)
            .or_else(|| matches.first())
            .cloned()
    } else {
        matches
            .iter()
            .rev()
            .find(|r| r.start < o)
            .or_else(|| matches.last())
            .cloned()
    }
}

/// Split `range` in `text` into sub-selections matching `pattern` (`s` command).
pub fn split_selection_on_pattern(
    text: &str,
    range: Range<usize>,
    pattern: &str,
) -> Vec<Range<usize>> {
    let start = clamp_char_boundary(text, range.start);
    let end = clamp_char_boundary(text, range.end);
    if pattern.is_empty() || start >= end {
        return Vec::new();
    }
    let slice = &text[start..end];
    if let Ok(re) = regex::Regex::new(pattern) {
        re.find_iter(slice)
            .filter(|m| !m.is_empty())
            .map(|m| (start + m.start())..(start + m.end()))
            .collect()
    } else {
        slice
            .match_indices(pattern)
            .map(|(i, m)| (start + i)..(start + i + m.len()))
            .collect()
    }
}

/// Compute the byte range for a new cursor on the line below the last selection (`C`).
pub fn add_cursor_below_range(text: &str, last_offset: usize) -> Option<usize> {
    let next = move_vertical(text, last_offset, 1);
    if line_start_offset(text, next) > line_start_offset(text, last_offset) {
        Some(next)
    } else {
        None
    }
}

// ---- Yank / Paste / Delete / Change Operations -----------------------------

impl HelixState {
    pub(super) fn op_delete(&mut self, text: &str) -> HelixOutcome {
        let range = self.effective_range(text);
        let deleted = text[range.clone()].to_string();
        let reg = self.selected_register.take();
        let yank_to_clipboard = self.registers.write(reg, deleted);
        self.clear_pending();
        self.mode = HelixMode::Normal;
        HelixOutcome::Edit {
            new_anchor: range.start,
            new_head: range.start,
            range,
            new_text: String::new(),
            yank_to_clipboard,
        }
    }

    pub(super) fn op_change(&mut self, text: &str) -> HelixOutcome {
        let range = self.effective_range(text);
        let deleted = text[range.clone()].to_string();
        let reg = self.selected_register.take();
        let yank_to_clipboard = self.registers.write(reg, deleted);
        self.clear_pending();
        self.mode = HelixMode::Insert;
        HelixOutcome::Edit {
            new_anchor: range.start,
            new_head: range.start,
            range,
            new_text: String::new(),
            yank_to_clipboard,
        }
    }

    pub(super) fn op_blackhole_delete(&mut self, text: &str) -> HelixOutcome {
        let range = self.effective_range(text);
        self.clear_pending();
        self.mode = HelixMode::Normal;
        HelixOutcome::Edit {
            new_anchor: range.start,
            new_head: range.start,
            range,
            new_text: String::new(),
            yank_to_clipboard: None,
        }
    }

    pub(super) fn op_blackhole_change(&mut self, text: &str) -> HelixOutcome {
        let range = self.effective_range(text);
        self.clear_pending();
        self.mode = HelixMode::Insert;
        HelixOutcome::Edit {
            new_anchor: range.start,
            new_head: range.start,
            range,
            new_text: String::new(),
            yank_to_clipboard: None,
        }
    }

    pub(super) fn op_yank(&mut self, text: &str) -> HelixOutcome {
        let range = self.effective_range(text);
        let yanked = text[range].to_string();
        let reg = self.selected_register.take();
        let clipboard = self.registers.write(reg, yanked);
        self.clear_pending();
        HelixOutcome::Yank { clipboard }
    }

    pub(super) fn op_paste(
        &mut self,
        text: &str,
        after: bool,
        system_clipboard: Option<&str>,
    ) -> HelixOutcome {
        let reg = self.selected_register.take();
        let content = self.registers.read(reg, system_clipboard);
        self.clear_pending();
        if content.is_empty() {
            return HelixOutcome::Handled;
        }
        let eff = self.effective_range(text);
        if content.ends_with('\n') {
            if after {
                let line_end = line_end_including_newline(text, eff.end.saturating_sub(1));
                let needs_leading_nl =
                    line_end == text.len() && !text.is_empty() && !text.ends_with('\n');
                let inserted = if needs_leading_nl {
                    format!("\n{}", content.strip_suffix('\n').unwrap_or(&content))
                } else {
                    content
                };
                let start = if needs_leading_nl {
                    line_end + 1
                } else {
                    line_end
                };
                let end = line_end + inserted.len();
                HelixOutcome::Edit {
                    range: line_end..line_end,
                    new_text: inserted,
                    new_anchor: start,
                    new_head: end,
                    yank_to_clipboard: None,
                }
            } else {
                let line_start = line_start_offset(text, eff.start);
                let end = line_start + content.len();
                HelixOutcome::Edit {
                    range: line_start..line_start,
                    new_text: content,
                    new_anchor: line_start,
                    new_head: end,
                    yank_to_clipboard: None,
                }
            }
        } else {
            let insert_at = if after { eff.end } else { eff.start };
            let end = insert_at + content.len();
            HelixOutcome::Edit {
                range: insert_at..insert_at,
                new_text: content,
                new_anchor: insert_at,
                new_head: end,
                yank_to_clipboard: None,
            }
        }
    }

    pub(super) fn op_yank_clipboard(&mut self, text: &str) -> HelixOutcome {
        let range = self.effective_range(text);
        let yanked = text[range].to_string();
        let clipboard = self.registers.write(Some('+'), yanked);
        HelixOutcome::Yank { clipboard }
    }

    pub(super) fn op_paste_clipboard(
        &mut self,
        text: &str,
        system_clipboard: Option<&str>,
    ) -> HelixOutcome {
        let content = self.registers.read(Some('+'), system_clipboard);
        if content.is_empty() {
            return HelixOutcome::Handled;
        }
        let eff = self.effective_range(text);
        let end = eff.end + content.len();
        HelixOutcome::Edit {
            range: eff.end..eff.end,
            new_text: content,
            new_anchor: eff.end,
            new_head: end,
            yank_to_clipboard: None,
        }
    }
}
