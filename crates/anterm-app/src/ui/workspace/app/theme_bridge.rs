//! Workspace-level font zoom and command-palette theme preview helpers.
//!
//! All Settings theme-editor, draft, window-surface, and font-picker controls
//! now live directly on [`crate::ui::chrome::settings::SettingsWindow`].

use gpui::{Context, Window, px};

use crate::core::config::Config;
use crate::core::config_store::ConfigStore;
use crate::ui::chrome::theme::{apply_theme, set_menus};

use super::{AntermApp, FONT_SIZE_MAX, FONT_SIZE_MIN};

impl AntermApp {
    fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        self.font_size = size;
        let px_size = px(size);
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                leaf.update(cx, |v, cx| v.set_font_size(px_size, cx));
            }
        }
        self.update_config(cx, |cfg| cfg.font_size = size);
    }

    pub(crate) fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.set_font_size(self.font_size + delta, cx);
    }

    pub(crate) fn reset_font_size(&mut self, cx: &mut Context<Self>) {
        self.set_font_size(Config::default().font_size, cx);
    }

    /// Commits a theme picked from the workspace command palette.
    pub(crate) fn set_preset(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_preview_restore = None;
        self.write_preset(id, cx);
        self.apply_theme_change(true, window, cx);
    }

    fn write_preset(&mut self, id: &str, cx: &mut Context<Self>) {
        let dark_now = crate::ui::chrome::theme::system_dark(cx);
        ConfigStore::update_in_memory(cx, |cfg| {
            if !cfg.theme_follow_system {
                cfg.theme_preset = id.to_string();
            } else if dark_now {
                cfg.theme_preset_dark = id.to_string();
            } else {
                cfg.theme_preset_light = id.to_string();
            }
        });
    }

    /// Previews a preset while the command palette's theme picker is open.
    pub(crate) fn preview_preset(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_preview_restore.is_none() {
            self.theme_preview_restore = Some(crate::ui::chrome::theme::effective_preset_id(cx));
        }
        self.write_preset(id, cx);
        self.apply_theme_change(false, window, cx);
    }

    /// Restores the preset that was live before a palette preview started.
    pub(crate) fn cancel_preset_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.theme_preview_restore.take() else {
            return;
        };
        self.write_preset(&id, cx);
        self.apply_theme_change(false, window, cx);
    }

    fn apply_theme_change(&mut self, persist: bool, window: &mut Window, cx: &mut Context<Self>) {
        apply_theme(Some(window), cx);
        set_menus(cx);
        if persist {
            let _ = ConfigStore::persist_current(cx);
        }
        cx.notify();
    }
}
