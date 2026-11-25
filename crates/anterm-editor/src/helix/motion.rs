use std::ops::Range;

// ---- UTF-8 & Text Motion Primitives ---------------------------------------

pub fn clamp_char_boundary(text: &str, offset: usize) -> usize {
    let mut o = offset.min(text.len());
    while o > 0 && !text.is_char_boundary(o) {
        o -= 1;
    }
    o
}

pub fn prev_char_offset(text: &str, offset: usize) -> usize {
    let o = clamp_char_boundary(text, offset);
    text[..o]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub fn next_char_offset(text: &str, offset: usize) -> usize {
    let o = clamp_char_boundary(text, offset);
    text[o..]
        .chars()
        .next()
        .map(|c| o + c.len_utf8())
        .unwrap_or(text.len())
}

pub fn move_left(text: &str, offset: usize, count: usize) -> usize {
    let mut cur = clamp_char_boundary(text, offset);
    for _ in 0..count {
        cur = prev_char_offset(text, cur);
    }
    cur
}

pub fn move_right(text: &str, offset: usize, count: usize) -> usize {
    let mut cur = clamp_char_boundary(text, offset);
    for _ in 0..count {
        cur = next_char_offset(text, cur);
    }
    cur
}

pub fn line_start_offset(text: &str, offset: usize) -> usize {
    let o = clamp_char_boundary(text, offset);
    text[..o].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

pub fn line_end_offset(text: &str, offset: usize) -> usize {
    let o = clamp_char_boundary(text, offset);
    let end = text[o..].find('\n').map(|i| o + i).unwrap_or(text.len());
    if end > 0 && text.as_bytes().get(end - 1) == Some(&b'\r') {
        end - 1
    } else {
        end
    }
}

pub fn line_end_including_newline(text: &str, offset: usize) -> usize {
    let o = clamp_char_boundary(text, offset);
    text[o..]
        .find('\n')
        .map(|i| o + i + 1)
        .unwrap_or(text.len())
}

pub fn line_first_non_blank_offset(text: &str, offset: usize) -> usize {
    let start = line_start_offset(text, offset);
    let end = line_end_offset(text, offset);
    for (rel, ch) in text[start..end].char_indices() {
        if !ch.is_whitespace() {
            return start + rel;
        }
    }
    end
}

/// Convert 1-based line number to byte offset at the start of that line.
pub fn line_number_to_offset(text: &str, line_1_based: usize) -> usize {
    let target = line_1_based.saturating_sub(1);
    if target == 0 {
        return 0;
    }
    let mut current_line = 0;
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            current_line += 1;
            if current_line == target {
                return idx + 1;
            }
        }
    }
    line_start_offset(text, text.len())
}

/// Move `delta_lines` vertically while preserving the character column on the line.
pub fn move_vertical(text: &str, offset: usize, delta_lines: isize) -> usize {
    if delta_lines == 0 || text.is_empty() {
        return clamp_char_boundary(text, offset);
    }
    let o = clamp_char_boundary(text, offset);
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let cur_line = line_starts
        .partition_point(|&start| start <= o)
        .saturating_sub(1);
    let cur_start = line_starts[cur_line];
    let col_chars = text[cur_start..o].chars().count();

    let target_line = (cur_line as isize + delta_lines)
        .clamp(0, line_starts.len().saturating_sub(1) as isize) as usize;
    let target_start = line_starts[target_line];
    let target_end = line_end_offset(text, target_start);
    text[target_start..target_end]
        .char_indices()
        .nth(col_chars)
        .map(|(rel, _)| target_start + rel)
        .unwrap_or(target_end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Punct,
}

fn classify_char(c: char, big_word: bool) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if big_word || c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// Move to the start of the next word (`w` / `W`).
pub fn word_next_start(text: &str, offset: usize, big_word: bool) -> usize {
    let o = clamp_char_boundary(text, offset);
    let mut iter = text[o..].char_indices().peekable();
    let Some((_, first)) = iter.next() else {
        return text.len();
    };
    let first_class = classify_char(first, big_word);
    if first_class != CharClass::Whitespace {
        while let Some(&(_, ch)) = iter.peek() {
            if classify_char(ch, big_word) != first_class {
                break;
            }
            iter.next();
        }
    }
    while let Some(&(rel, ch)) = iter.peek() {
        if !ch.is_whitespace() {
            return o + rel;
        }
        iter.next();
    }
    text.len()
}

/// Move to the end of the current or next word (`e` / `E`).
pub fn word_next_end(text: &str, offset: usize, big_word: bool) -> usize {
    let mut o = clamp_char_boundary(text, offset);
    if o >= text.len() {
        return text.len();
    }
    // Step at least one char if sitting at the end of a word.
    let next = next_char_offset(text, o);
    if next < text.len()
        && let (Some(cur_ch), Some(next_ch)) =
            (text[o..].chars().next(), text[next..].chars().next())
        && (classify_char(cur_ch, big_word) != classify_char(next_ch, big_word)
            || cur_ch.is_whitespace())
    {
        o = next;
    }
    let mut iter = text[o..].char_indices().peekable();
    // Skip leading whitespace.
    while let Some(&(_, ch)) = iter.peek() {
        if !ch.is_whitespace() {
            break;
        }
        iter.next();
    }
    let Some(&(start_rel, first)) = iter.peek() else {
        return text.len();
    };
    let target_class = classify_char(first, big_word);
    let mut end_rel = start_rel + first.len_utf8();
    iter.next();
    for (rel, ch) in iter {
        if classify_char(ch, big_word) != target_class {
            break;
        }
        end_rel = rel + ch.len_utf8();
    }
    o + end_rel
}

/// Move to the start of the previous word (`b` / `B`).
pub fn word_prev_start(text: &str, offset: usize, big_word: bool) -> usize {
    let o = clamp_char_boundary(text, offset);
    if o == 0 {
        return 0;
    }
    let mut chars: Vec<(usize, char)> = text[..o].char_indices().collect();
    while let Some(&(_, ch)) = chars.last() {
        if !ch.is_whitespace() {
            break;
        }
        chars.pop();
    }
    let Some(&(last_idx, last_ch)) = chars.last() else {
        return 0;
    };
    let target_class = classify_char(last_ch, big_word);
    let mut start_idx = last_idx;
    while let Some(&(idx, ch)) = chars.last() {
        if classify_char(ch, big_word) != target_class {
            break;
        }
        start_idx = idx;
        chars.pop();
    }
    start_idx
}

/// Find the `count`-th occurrence of `target` forward or backward (`f`, `t`, `F`, `T`).
pub fn find_char_motion(
    text: &str,
    offset: usize,
    target: char,
    forward: bool,
    till: bool,
    count: usize,
) -> Option<usize> {
    let o = clamp_char_boundary(text, offset);
    let mut remaining = count.max(1);
    if forward {
        let search_start = next_char_offset(text, o);
        for (rel, ch) in text[search_start..].char_indices() {
            if ch == target {
                remaining -= 1;
                if remaining == 0 {
                    let found = search_start + rel;
                    return Some(if till { found } else { found + ch.len_utf8() });
                }
            }
        }
        None
    } else {
        for (idx, ch) in text[..o].char_indices().rev() {
            if ch == target {
                remaining -= 1;
                if remaining == 0 {
                    return Some(if till { idx + ch.len_utf8() } else { idx });
                }
            }
        }
        None
    }
}

/// Select the current line, or extend an existing full-line selection by `count` lines (`x`).
pub fn extend_line_selection(
    text: &str,
    anchor: usize,
    head: usize,
    count: usize,
) -> (usize, usize) {
    let a = clamp_char_boundary(text, anchor);
    let h = clamp_char_boundary(text, head);
    let min = a.min(h);
    let max = a.max(h);
    let start_of_min = line_start_offset(text, min);
    let already_line_selected = a <= h
        && a == start_of_min
        && max > min
        && (max == text.len() || text.as_bytes().get(max - 1) == Some(&b'\n'));

    let mut cur_end = if already_line_selected {
        max
    } else {
        line_end_including_newline(text, h)
    };
    let extra = if already_line_selected {
        count
    } else {
        count.saturating_sub(1)
    };
    for _ in 0..extra {
        if cur_end < text.len() {
            cur_end = line_end_including_newline(text, cur_end);
        }
    }
    let new_anchor = if already_line_selected {
        start_of_min
    } else {
        line_start_offset(text, h)
    };
    (new_anchor, cur_end)
}

// ---- Brackets, Text Objects, and Surround ---------------------------------

pub fn bracket_pair(ch: char) -> Option<(char, char)> {
    match ch {
        '(' | ')' | 'b' => Some(('(', ')')),
        '[' | ']' => Some(('[', ']')),
        '{' | '}' | 'B' => Some(('{', '}')),
        '<' | '>' => Some(('<', '>')),
        _ => None,
    }
}

pub fn surround_pair(ch: char) -> (char, char) {
    bracket_pair(ch).unwrap_or((ch, ch))
}

/// Find the matching bracket for the character at (or nearest on the line to) `offset` (`mm`).
pub fn find_matching_bracket(text: &str, offset: usize) -> Option<usize> {
    let o = clamp_char_boundary(text, offset);
    let is_bracket = |c: char| matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>');
    let mut bracket_pos = None;
    if let Some(ch) = text[o..].chars().next()
        && is_bracket(ch)
    {
        bracket_pos = Some((o, ch));
    } else {
        let line_end = line_end_offset(text, o);
        for (rel, ch) in text[o..line_end].char_indices() {
            if is_bracket(ch) {
                bracket_pos = Some((o + rel, ch));
                break;
            }
        }
    }
    let (pos, ch) = bracket_pos?;
    let (open, close, forward) = match ch {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        '<' => ('<', '>', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        '}' => ('{', '}', false),
        '>' => ('<', '>', false),
        _ => return None,
    };
    if forward {
        let mut depth = 0usize;
        for (rel, c) in text[pos..].char_indices() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(pos + rel);
                }
            }
        }
        None
    } else {
        let mut depth = 0usize;
        for (idx, c) in text[..=pos].char_indices().rev() {
            if c == close {
                depth += 1;
            } else if c == open {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx);
                }
            }
        }
        None
    }
}

/// Find the innermost balanced `(open, close)` pair enclosing `offset`.
/// Returns `(outer_range, inner_range)`.
pub fn find_enclosing_pair(
    text: &str,
    offset: usize,
    open: char,
    close: char,
) -> Option<(Range<usize>, Range<usize>)> {
    let o = clamp_char_boundary(text, offset);
    if open == close {
        // Quote-like symmetric delimiter: search on the current line first, then whole text.
        let line_start = line_start_offset(text, o);
        let line_end = line_end_offset(text, o);
        if let Some(res) =
            find_symmetric_pair_in_slice(&text[line_start..line_end], o - line_start, open)
        {
            return Some((
                (line_start + res.0.start)..(line_start + res.0.end),
                (line_start + res.1.start)..(line_start + res.1.end),
            ));
        }
        return find_symmetric_pair_in_slice(text, o, open);
    }

    // Asymmetric bracket pair: scan backward for unmatched `open`, then forward for matching `close`.
    let mut depth = 0usize;
    let mut open_idx = None;
    // If cursor is directly on `open`, use it.
    if text[o..].chars().next() == Some(open) {
        open_idx = Some(o);
    } else {
        let scan_end = if text[o..].chars().next() == Some(close) {
            o
        } else {
            next_char_offset(text, o)
        };
        for (idx, ch) in text[..scan_end].char_indices().rev() {
            if ch == close && idx < o {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    open_idx = Some(idx);
                    break;
                }
                depth -= 1;
            }
        }
    }
    let start = open_idx?;
    let mut fwd_depth = 0usize;
    for (rel, ch) in text[start..].char_indices() {
        if ch == open {
            fwd_depth += 1;
        } else if ch == close {
            fwd_depth = fwd_depth.saturating_sub(1);
            if fwd_depth == 0 {
                let close_start = start + rel;
                let close_end = close_start + ch.len_utf8();
                let inner_start = start + open.len_utf8();
                return Some((start..close_end, inner_start..close_start));
            }
        }
    }
    None
}

fn find_symmetric_pair_in_slice(
    slice: &str,
    rel_offset: usize,
    delim: char,
) -> Option<(Range<usize>, Range<usize>)> {
    let positions: Vec<usize> = slice
        .char_indices()
        .filter_map(|(i, c)| (c == delim).then_some(i))
        .collect();
    for pair in positions.chunks_exact(2) {
        let open_pos = pair[0];
        let close_pos = pair[1];
        let close_end = close_pos + delim.len_utf8();
        if rel_offset >= open_pos && rel_offset <= close_end {
            let inner_start = open_pos + delim.len_utf8();
            return Some((open_pos..close_end, inner_start..close_pos));
        }
    }
    None
}

/// Compute the byte range for a text object (`mi<obj>` when `!around`, `ma<obj>` when `around`).
pub fn text_object_range(
    text: &str,
    offset: usize,
    obj: char,
    around: bool,
) -> Option<Range<usize>> {
    let o = clamp_char_boundary(text, offset);
    if text.is_empty() {
        return None;
    }
    match obj {
        'w' | 'W' => {
            let big = obj == 'W';
            let ch = text[o..].chars().next()?;
            let class = classify_char(ch, big);
            if class == CharClass::Whitespace {
                return None;
            }
            let mut start = o;
            for (idx, c) in text[..o].char_indices().rev() {
                if classify_char(c, big) != class {
                    break;
                }
                start = idx;
            }
            let mut end = o;
            for (rel, c) in text[o..].char_indices() {
                if classify_char(c, big) != class {
                    break;
                }
                end = o + rel + c.len_utf8();
            }
            if around {
                let line_end = line_end_offset(text, end);
                let mut trailing_end = end;
                for (rel, c) in text[end..line_end].char_indices() {
                    if !c.is_whitespace() {
                        break;
                    }
                    trailing_end = end + rel + c.len_utf8();
                }
                if trailing_end > end {
                    end = trailing_end;
                } else {
                    let line_start = line_start_offset(text, start);
                    for (idx, c) in text[line_start..start].char_indices().rev() {
                        if !c.is_whitespace() {
                            break;
                        }
                        start = line_start + idx;
                    }
                }
            }
            Some(start..end)
        }
        'p' => {
            // Paragraph text object: contiguous non-blank lines.
            let line_starts: Vec<usize> = std::iter::once(0)
                .chain(text.match_indices('\n').map(|(i, _)| i + 1))
                .collect();
            let is_blank = |line_idx: usize| -> bool {
                let s = line_starts[line_idx];
                let e = line_end_offset(text, s);
                text[s..e].trim().is_empty()
            };
            let cur_line = line_starts.partition_point(|&s| s <= o).saturating_sub(1);
            if is_blank(cur_line) {
                return None;
            }
            let mut first = cur_line;
            while first > 0 && !is_blank(first - 1) {
                first -= 1;
            }
            let mut last = cur_line;
            while last + 1 < line_starts.len() && !is_blank(last + 1) {
                last += 1;
            }
            if around {
                while last + 1 < line_starts.len() && is_blank(last + 1) {
                    last += 1;
                }
            }
            let start = line_starts[first];
            let end = line_end_including_newline(text, line_starts[last]);
            Some(start..end)
        }
        _ => {
            let (open, close) = surround_pair(obj);
            let (outer, inner) = find_enclosing_pair(text, o, open, close)?;
            Some(if around { outer } else { inner })
        }
    }
}
