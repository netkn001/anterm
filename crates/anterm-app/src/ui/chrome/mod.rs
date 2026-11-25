//! Search palette, tray, and re-exports from `anterm-ui-kit` and `anterm-settings-ui`.

pub(crate) use anterm_settings_ui as settings;
pub(crate) use anterm_ui_kit::{
    assets, dialog, notice, perf, prefill, presets, prompt, rounding, scrollbar, theme,
    window_chrome,
};

pub(crate) mod keymap {
    pub(crate) use anterm_ui_kit::keymap::*;

    pub(crate) fn init(cx: &mut gpui::App) {
        crate::register_app_hooks(cx);
        anterm_ui_kit::keymap::init(cx);
    }
}

pub(crate) mod palette;
pub(crate) mod search;
pub(crate) mod tray;
