//! Compact 16-byte/cell binary frame packer for GPU mobile terminal renderers
//! (iOS 120Hz Metal `MTLBuffer` / Android Vulkan/Canvas).
//!
//! Layout:
//! - Header (24 bytes, little-endian):
//!   - `[0..8]`   `revision: u64`
//!   - `[8..10]`  `cols: u16`
//!   - `[10..12]` `rows: u16`
//!   - `[12..14]` `cursor_col: u16`
//!   - `[14..16]` `cursor_row: u16`
//!   - `[16]`     `cursor_shape: u8` (0=block, 1=beam, 2=underline, 3=hollow, 4=hidden)
//!   - `[17]`     `flags: u8` (bit0=alt_screen, bit1=cursor_visible, bit2=full_redraw)
//!   - `[18..20]` `dirty_row_count: u16`
//!   - `[20..24]` `reserved: u32`
//! - Per dirty row:
//!   - `row_index: u16` | `cell_count: u16`
//!   - `cells[cell_count]` × `PackedCell` (16 bytes):
//!     - `[0..4]`   `codepoint: u32`
//!     - `[4..8]`   `fg_rgba: u32` (0xRRGGBBAA)
//!     - `[8..12]`  `bg_rgba: u32` (0xRRGGBBAA)
//!     - `[12..14]` `style: u16`
//!     - `[14..16]` `extra_idx: u16`

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

pub const FRAME_HEADER_BYTES: usize = 24;
pub const PACKED_CELL_BYTES: usize = 16;

pub mod frame_flag {
    pub const ALT_SCREEN: u8 = 1 << 0;
    pub const CURSOR_VISIBLE: u8 = 1 << 1;
    pub const FULL_REDRAW: u8 = 1 << 2;
}

pub mod cell_style {
    pub const BOLD: u16 = 1 << 0;
    pub const ITALIC: u16 = 1 << 1;
    pub const UNDERLINE: u16 = 1 << 2;
    pub const STRIKETHROUGH: u16 = 1 << 3;
    pub const DIM: u16 = 1 << 4;
    pub const INVERSE: u16 = 1 << 5;
    pub const WIDE_CHAR: u16 = 1 << 6;
    pub const WIDE_CHAR_SPACER: u16 = 1 << 7;
    pub const SELECTED: u16 = 1 << 8;
    pub const PREDICTED_ECHO: u16 = 1 << 9;
}

/// Pre-resolved RGBA palette (`0xRRGGBBAA`) matching the active theme.
#[derive(Clone, Debug)]
pub struct PaletteRgba {
    pub fg: u32,
    pub bg: u32,
    pub ansi16: [u32; 16],
}

impl Default for PaletteRgba {
    fn default() -> Self {
        // Default Anterm dark palette (matches high-contrast terminal readability).
        Self {
            fg: 0xE6EDF3FF,
            bg: 0x0D1117FF,
            ansi16: [
                0x484F58FF, 0xFF7B72FF, 0x3FB950FF, 0xD29922FF,
                0x58A6FFFF, 0xBC8CFFFF, 0x39C5CFFF, 0xB1BAC4FF,
                0x6E7681FF, 0xFFA198FF, 0x56D364FF, 0xE3B341FF,
                0x79C0FFFF, 0xD2A8FFFF, 0x56D4DDFF, 0xF0F6FCFF,
            ],
        }
    }
}

impl PaletteRgba {
    pub fn resolve(&self, color: Color, default_rgba: u32) -> u32 {
        match color {
            Color::Spec(rgb) => rgba(rgb.r, rgb.g, rgb.b, 0xFF),
            Color::Named(named) => match named {
                NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                NamedColor::Background => self.bg,
                NamedColor::Black => self.ansi16[0],
                NamedColor::Red => self.ansi16[1],
                NamedColor::Green => self.ansi16[2],
                NamedColor::Yellow => self.ansi16[3],
                NamedColor::Blue => self.ansi16[4],
                NamedColor::Magenta => self.ansi16[5],
                NamedColor::Cyan => self.ansi16[6],
                NamedColor::White => self.ansi16[7],
                NamedColor::BrightBlack => self.ansi16[8],
                NamedColor::BrightRed => self.ansi16[9],
                NamedColor::BrightGreen => self.ansi16[10],
                NamedColor::BrightYellow => self.ansi16[11],
                NamedColor::BrightBlue => self.ansi16[12],
                NamedColor::BrightMagenta => self.ansi16[13],
                NamedColor::BrightCyan => self.ansi16[14],
                NamedColor::BrightWhite => self.ansi16[15],
                _ => default_rgba,
            },
            Color::Indexed(idx) => {
                if (idx as usize) < 16 {
                    self.ansi16[idx as usize]
                } else if idx < 232 {
                    let v = idx - 16;
                    let r = cube_channel(v / 36);
                    let g = cube_channel((v / 6) % 6);
                    let b = cube_channel(v % 6);
                    rgba(r, g, b, 0xFF)
                } else {
                    let gray = 8 + (idx - 232) * 10;
                    rgba(gray, gray, gray, 0xFF)
                }
            }
        }
    }
}

#[inline]
fn cube_channel(step: u8) -> u8 {
    if step == 0 { 0 } else { 55 + step * 40 }
}

#[inline]
pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32)
}

/// Pack the visible viewport of `term` into a flat binary frame suitable for
/// direct upload into a Metal `MTLBuffer`.
pub fn pack_term_frame<T: alacritty_terminal::event::EventListener>(
    term: &Term<T>,
    palette: &PaletteRgba,
    revision: u64,
    full_redraw: bool,
    echo_overlay: Option<&str>,
) -> Vec<u8> {
    let cols = term.columns();
    let rows = term.screen_lines();
    let grid = term.grid();
    let display_offset = grid.display_offset() as i32;
    let cursor = term.renderable_content().cursor;
    let cursor_col = cursor.point.column.0.min(u16::MAX as usize) as u16;
    let cursor_screen_row = cursor.point.line.0 + display_offset;
    let cursor_in_view = cursor_screen_row >= 0 && (cursor_screen_row as usize) < rows;
    let cursor_row = if cursor_in_view {
        cursor_screen_row as u16
    } else {
        0
    };
    let cursor_shape: u8 = match cursor.shape {
        CursorShape::Block => 0,
        CursorShape::Beam => 1,
        CursorShape::Underline => 2,
        CursorShape::HollowBlock => 3,
        CursorShape::Hidden => 4,
    };
    let mut flags = 0u8;
    if term.mode().contains(TermMode::ALT_SCREEN) {
        flags |= frame_flag::ALT_SCREEN;
    }
    if cursor_in_view
        && term.mode().contains(TermMode::SHOW_CURSOR)
        && cursor.shape != CursorShape::Hidden
    {
        flags |= frame_flag::CURSOR_VISIBLE;
    }
    if full_redraw {
        flags |= frame_flag::FULL_REDRAW;
    }

    let capacity = FRAME_HEADER_BYTES + rows * (4 + cols * PACKED_CELL_BYTES);
    let mut buf = Vec::with_capacity(capacity);
    buf.extend_from_slice(&revision.to_le_bytes());
    buf.extend_from_slice(&(cols.min(u16::MAX as usize) as u16).to_le_bytes());
    buf.extend_from_slice(&(rows.min(u16::MAX as usize) as u16).to_le_bytes());
    buf.extend_from_slice(&cursor_col.to_le_bytes());
    buf.extend_from_slice(&cursor_row.to_le_bytes());
    buf.push(cursor_shape);
    buf.push(flags);
    buf.extend_from_slice(&(rows.min(u16::MAX as usize) as u16).to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());

    let echo_chars: Vec<char> = echo_overlay.map(|s| s.chars().collect()).unwrap_or_default();

    for screen_y in 0..rows {
        let line = Line(screen_y as i32 - display_offset);
        buf.extend_from_slice(&(screen_y as u16).to_le_bytes());
        buf.extend_from_slice(&(cols as u16).to_le_bytes());
        for col in 0..cols {
            let cell = &grid[Point::new(line, Column(col))];
            let mut ch = cell.c;
            let mut style = pack_style(cell.flags);
            let (mut fg, mut bg) = (
                palette.resolve(cell.fg, palette.fg),
                palette.resolve(cell.bg, palette.bg),
            );
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            // Overlay optimistic local echo characters starting at cursor when active.
            if cursor_in_view
                && screen_y == cursor_row as usize
                && !echo_chars.is_empty()
                && col >= cursor_col as usize
            {
                let idx = col - cursor_col as usize;
                if let Some(&pred) = echo_chars.get(idx) {
                    ch = pred;
                    style |= cell_style::PREDICTED_ECHO | cell_style::UNDERLINE | cell_style::DIM;
                }
            }

            buf.extend_from_slice(&(ch as u32).to_le_bytes());
            buf.extend_from_slice(&fg.to_le_bytes());
            buf.extend_from_slice(&bg.to_le_bytes());
            buf.extend_from_slice(&style.to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes());
        }
    }

    buf
}

fn pack_style(flags: Flags) -> u16 {
    let mut s = 0u16;
    if flags.contains(Flags::BOLD) {
        s |= cell_style::BOLD;
    }
    if flags.contains(Flags::ITALIC) {
        s |= cell_style::ITALIC;
    }
    if flags.intersects(
        Flags::UNDERLINE
            | Flags::DOUBLE_UNDERLINE
            | Flags::UNDERCURL
            | Flags::DOTTED_UNDERLINE
            | Flags::DASHED_UNDERLINE,
    ) {
        s |= cell_style::UNDERLINE;
    }
    if flags.contains(Flags::STRIKEOUT) {
        s |= cell_style::STRIKETHROUGH;
    }
    if flags.contains(Flags::DIM) {
        s |= cell_style::DIM;
    }
    if flags.contains(Flags::INVERSE) {
        s |= cell_style::INVERSE;
    }
    if flags.contains(Flags::WIDE_CHAR) {
        s |= cell_style::WIDE_CHAR;
    }
    if flags.contains(Flags::WIDE_CHAR_SPACER) {
        s |= cell_style::WIDE_CHAR_SPACER;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn pack_term_frame_has_exact_header_and_cell_byte_sizes() {
        let mut term = Term::new(Config::default(), &TermSize::new(10, 4), VoidListener);
        let mut proc: Processor = Processor::new();
        proc.advance(&mut term, b"Hi\r\nOK");

        let frame = pack_term_frame(&term, &PaletteRgba::default(), 7, true, Some("!"));
        let expected_len = FRAME_HEADER_BYTES + 4 * (4 + 10 * PACKED_CELL_BYTES);
        assert_eq!(frame.len(), expected_len);
        assert_eq!(u64::from_le_bytes(frame[0..8].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes(frame[8..10].try_into().unwrap()), 10);
        assert_eq!(u16::from_le_bytes(frame[10..12].try_into().unwrap()), 4);
        // Row 0, cell 0 should be 'H'
        let row0_cell0_cp = u32::from_le_bytes(
            frame[FRAME_HEADER_BYTES + 4..FRAME_HEADER_BYTES + 8]
                .try_into()
                .unwrap(),
        );
        assert_eq!(row0_cell0_cp, 'H' as u32);
    }
}
