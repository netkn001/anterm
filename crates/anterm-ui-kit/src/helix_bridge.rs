//! Thin GPUI adapter around [`anterm_editor::helix`].
//!
//! Translates GPUI's `Keystroke` and `gpui_kit::component::input::Rope` into the
//! framework-free types consumed by `anterm-editor`.

use std::ops::Range;

use anterm_editor::helix::KeyInput;
use gpui::Keystroke;

/// Convert a GPUI [`Keystroke`] into an [`anterm_editor::helix::KeyInput`].
pub fn key_input(ks: &Keystroke) -> KeyInput<'_> {
    KeyInput {
        key: ks.key.as_str(),
        key_char: ks.key_char.as_deref(),
        control: ks.modifiers.control,
        alt: ks.modifiers.alt,
        shift: ks.modifiers.shift,
        platform: ks.modifiers.platform,
    }
}

/// Byte range `[head..next]` covered by the Helix block cursor in a `Rope`
/// buffer. Clamps `raw_head` to a valid UTF-8 character boundary and advances
/// over at most one non-newline code point without slicing fixed byte windows.
pub fn block_cursor_byte_range(
    text: &gpui_kit::component::input::Rope,
    raw_head: usize,
) -> Range<usize> {
    use gpui_kit::component::input::RopeExt as _;
    let len = text.len();
    let head = text.floor_char_boundary(raw_head.min(len));
    let next = match (head < len).then(|| text.char_at(head)).flatten() {
        Some(ch) if ch != '\n' && ch != '\r' => (head + ch.len_utf8()).min(len),
        _ => head,
    };
    head..next
}

#[cfg(test)]
mod tests {
    use super::block_cursor_byte_range;

    #[test]
    fn rope_block_cursor_byte_range_handles_multibyte_utf8_and_mid_char_offsets() {
        let md = "1234567中\r\nCodex、WorkBuddy 和 🚀\n";
        let rope = gpui_kit::component::input::Rope::from(md);

        assert_eq!(block_cursor_byte_range(&rope, 0), 0..1);
        assert_eq!(block_cursor_byte_range(&rope, 7), 7..10);
        assert_eq!(block_cursor_byte_range(&rope, 8), 7..10);
        assert_eq!(block_cursor_byte_range(&rope, 9), 7..10);
        assert_eq!(block_cursor_byte_range(&rope, 10), 10..10);
        assert_eq!(block_cursor_byte_range(&rope, 11), 11..11);

        for raw in 0..=(md.len() + 16) {
            let range = block_cursor_byte_range(&rope, raw);
            assert!(rope.is_char_boundary(range.start), "start at raw={raw}");
            assert!(rope.is_char_boundary(range.end), "end at raw={raw}");
            assert!(range.start <= range.end);
        }
    }
}
