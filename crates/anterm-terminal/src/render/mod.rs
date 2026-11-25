//! Terminal grid rendering helpers, Kitty graphics, palette, and scrollbar.

pub mod boxdraw;
pub mod color_scheme;
pub mod highlight;
pub mod images;
pub use anterm_ui_kit::palette_colors as palette;
pub mod scrollbar;
