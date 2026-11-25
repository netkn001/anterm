//! Config mutations, theme-draft editing, agent hooks, and keybinding
//! recording owned directly by [`SettingsWindow`].
//!
//! Every config mutation writes through [`ConfigStore`], which updates the
//! global [`Config`] so every open workspace window synchronizes reactively via
//! its own `observe_global_in::<Config>` subscription without `SettingsWindow`
//! ever reaching into `AntermApp`.

use std::sync::Arc;

use gpui::{App, Context, Entity, Subscription, Window, prelude::*, px};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{InputEvent, InputState};

#[cfg(target_os = "windows")]
use anterm_ui_kit::config::WindowBackdrop;
use anterm_ui_kit::config::{
    BellMode, Config, ConfirmClose, CursorStyle as ConfigCursorStyle, LinkFileOpen,
    MouseZoomModifier, NewTabPosition, NotifyMode, PromptCursorStyle, ShellConfig, StartupMode,
    TabBarPosition, UI_FONT_SIZE_MAX, UI_FONT_SIZE_MIN, UpdateChannel, WdStrategy,
};
use anterm_ui_kit::config_store::ConfigStore;
use anterm_ui_kit::presets::Fill;
use anterm_ui_kit::theme::{apply_theme, set_menus};
use anterm_ui_kit::config::{FONT_SIZE_MAX, FONT_SIZE_MIN, LINE_HEIGHT_MAX, LINE_HEIGHT_MIN};
use anterm_ui_kit::intent::{
    RECORD_COMMIT_DELAY_MS, ThemeEdit, clear_window_override_values, hsla_to_u32, join_shell_args,
    split_shell_args, wd_path_saveable,
};
use anterm_i18n::{L10nKey, set_locale, t, t_fmt};

use super::window::SettingsWindow;
use super::{
    AgentHookRow, AgentHooksMachine, AgentHooksView, KeyConflict, Recording, SettingsSection,
    ThemeEditor, ThemeMode, font_default_label, ui_font_default_label,
};

impl SettingsWindow {
    // ── ConfigStore persistence & rollback ───────────────────────────────

    pub fn persist_settings_config(&mut self, cx: &mut Context<Self>) {
        let _ = ConfigStore::persist_current(cx);
        let error = ConfigStore::save_error(cx).map(str::to_owned);
        let config = ConfigStore::read(cx).clone();
        if error.is_none() {
            self.state.saved_config = config;
        }
        self.state.save_error = error;
        cx.notify();
    }

    pub fn discard_unsaved_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self
            .state
            .save_error
            .is_some()
            .then(|| self.state.saved_config.clone());
        let restored = ConfigStore::discard_unsaved(cx)
            || match snapshot {
                Some(snapshot) => {
                    ConfigStore::replace_from_disk(snapshot, cx);
                    true
                }
                None => false,
            };
        if restored {
            self.state.save_error = None;
            apply_theme(Some(window), cx);
            self.rebuild_theme_editor(window, cx);
            cx.notify();
        }
    }

    pub fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut Config),
    ) {
        let error = ConfigStore::update(cx, mutate).err();
        let config = cx.global::<Config>().clone();
        if error.is_none() {
            self.state.saved_config = config;
        }
        self.state.save_error = error;
        cx.notify();
    }

    // ── Scalar config setters ────────────────────────────────────────────

    pub fn set_cursor_style(&mut self, style: ConfigCursorStyle, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.cursor_style = style);
    }

    pub fn set_prompt_cursor_style(
        &mut self,
        style: PromptCursorStyle,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.prompt_cursor_style = style);
    }

    pub fn set_link_url(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.link_url = on);
    }

    pub fn set_link_file_open(&mut self, mode: LinkFileOpen, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.link_file_open = Some(mode));
    }

    pub fn set_ssh_loopback_forward(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.ssh_loopback_forward = on);
    }

    pub fn set_verify_host_keys(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.verify_host_keys = on);
    }

    pub fn set_ssh_warn_on_close(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.ssh_warn_on_close = on);
    }

    pub fn set_check_for_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.check_for_updates = on);
    }

    pub fn set_auto_download_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.auto_download_updates = on);
    }

    pub fn set_update_channel(&mut self, channel: UpdateChannel, cx: &mut Context<Self>) {
        if cx.global::<Config>().update_channel == channel {
            return;
        }
        self.update_config(cx, |cfg| cfg.update_channel = channel);
        anterm_ui_kit::update::switch_channel(cx);
    }

    pub fn set_dim_inactive_panes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.dim_inactive_panes = on);
    }

    pub fn set_auto_hide_titlebar_buttons(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.auto_hide_titlebar_buttons = on);
    }

    pub fn set_cursor_blink(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.cursor_blink = on);
    }

    pub fn set_scrollback_limit(&mut self, lines: usize, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.scrollback_limit = lines.clamp(100, anterm_ui_kit::config::MAX_SCROLLBACK)
        });
    }

    pub fn set_new_tab_position(&mut self, pos: NewTabPosition, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.new_tab_position = pos);
    }

    pub fn set_confirm_close(&mut self, mode: ConfirmClose, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.confirm_close = mode);
    }

    pub fn set_tab_bar_position(&mut self, pos: TabBarPosition, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.tab_bar_position = pos);
    }

    pub fn set_notify_mode(&mut self, mode: NotifyMode, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.notify_on_command_finish = mode);
    }

    pub fn set_notify_threshold(&mut self, secs: u64, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.notify_threshold_secs = secs.clamp(1, 3600));
    }

    pub fn set_bell_mode(&mut self, mode: BellMode, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.bell = mode);
    }

    pub fn set_restore_session(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.restore_session = on);
    }

    pub fn set_per_pane_history(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.per_pane_history = on);
    }

    pub fn set_show_tray_icon(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.show_tray_icon = on);
    }

    pub fn set_font_thicken(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.font_thicken = on);
    }

    pub fn set_macos_option_as_alt(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.macos_option_as_alt = on);
    }

    pub fn set_mouse_hide_while_typing(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.mouse_hide_while_typing = on);
        anterm_ui_kit::theme::apply_cursor_hide_mode(cx);
    }

    pub fn set_focus_follows_mouse(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.focus_follows_mouse = on);
    }

    pub fn set_mouse_reporting(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.mouse_reporting = on);
    }

    pub fn set_mouse_scroll_multiplier(&mut self, mult: f32, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.mouse_scroll_multiplier = mult.clamp(0.1, 10.0)
        });
    }

    pub fn set_smooth_scroll(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.smooth_scroll = on);
    }

    pub fn set_mouse_zoom_modifier(
        &mut self,
        modifier: MouseZoomModifier,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.mouse_zoom_modifier = modifier);
    }

    pub fn set_clipboard_trim(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.clipboard_trim_trailing_spaces = on);
    }

    pub fn set_copy_on_select(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.copy_on_select = on);
    }

    pub fn set_smart_select(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.smart_select = on);
    }

    pub fn set_prompt_editor(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.prompt_editor = on);
    }

    pub fn set_tab_completion(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.tab_completion = on);
    }

    pub fn set_history_search(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.history_search = on);
    }

    pub fn set_startup_mode(&mut self, mode: StartupMode, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.startup_mode = mode);
    }

    pub fn set_remember_window_size(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.remember_window_size = on);
    }

    pub fn set_editor_git_gutter(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.editor_git_gutter = on);
    }

    // ── Fonts & typography ───────────────────────────────────────────────

    pub fn settings_font_names(cx: &App) -> Vec<String> {
        let cfg = cx.global::<Config>();
        let mut names = cx.text_system().all_font_names();
        for configured in [
            Some(&cfg.font_family),
            cfg.ui_font_family.as_ref(),
            cfg.font_family_bold.as_ref(),
            cfg.font_family_italic.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !names.contains(configured) {
                names.push(configured.clone());
            }
        }
        names.sort_unstable();
        names.dedup();
        names
    }

    fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        self.update_config(cx, |cfg| cfg.font_size = size);
    }

    pub fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let current = cx.global::<Config>().font_size;
        self.set_font_size(current + delta, cx);
    }

    pub fn reset_font_size(&mut self, cx: &mut Context<Self>) {
        self.set_font_size(Config::default().font_size, cx);
    }

    pub fn ui_font_size(&self, cx: &App) -> f32 {
        ConfigStore::read(cx).ui_font_size
    }

    fn set_ui_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(UI_FONT_SIZE_MIN, UI_FONT_SIZE_MAX);
        if ConfigStore::read(cx).ui_font_size == size {
            return;
        }
        self.update_config(cx, |cfg| cfg.ui_font_size = size);
        anterm_ui_kit::theme::apply_ui_font_size(size, cx);
    }

    pub fn change_ui_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.set_ui_font_size(self.ui_font_size(cx) + delta, cx);
    }

    pub fn reset_ui_font_size(&mut self, cx: &mut Context<Self>) {
        self.set_ui_font_size(Config::default().ui_font_size, cx);
    }

    fn set_line_height(&mut self, mul: f32, cx: &mut Context<Self>) {
        let mul = mul.clamp(LINE_HEIGHT_MIN, LINE_HEIGHT_MAX);
        self.update_config(cx, |cfg| cfg.line_height = mul);
    }

    pub fn change_line_height(&mut self, delta: f32, cx: &mut Context<Self>) {
        let current = cx.global::<Config>().line_height;
        self.set_line_height(current + delta, cx);
    }

    pub fn reset_line_height(&mut self, cx: &mut Context<Self>) {
        self.set_line_height(Config::default().line_height, cx);
    }

    pub fn set_font_ligatures(&mut self, on: bool, cx: &mut Context<Self>) {
        let features = on.then(|| {
            anterm_ui_kit::config::FontFeatures(Arc::new(vec![
                ("calt".to_string(), 1),
                ("liga".to_string(), 1),
            ]))
        });
        self.update_config(cx, |cfg| cfg.font_features = features);
    }

    pub fn commit_font_family(&mut self, family: String, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.font_family = family);
    }

    pub fn commit_font_family_emphasis(
        &mut self,
        bold: bool,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let family = (name != font_default_label()).then_some(name);
        self.update_config(cx, |cfg| {
            if bold {
                cfg.font_family_bold = family;
            } else {
                cfg.font_family_italic = family;
            }
        });
    }

    pub fn commit_ui_font_family(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let family = (name != ui_font_default_label()).then_some(name);
        if cx.global::<Config>().ui_font_family == family {
            return;
        }
        self.update_config(cx, |cfg| cfg.ui_font_family = family);
        apply_theme(Some(window), cx);
        cx.refresh_windows();
        cx.notify();
    }

    // ── Theme presets, draft editor, and window surface overrides ────────

    pub fn set_preset(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_draft_dirty() {
            let id = id.to_string();
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_preset(&id, window, cx)
            });
            return;
        }
        let dark_now = anterm_ui_kit::theme::system_dark(cx);
        let id = id.to_string();
        self.update_config(cx, |cfg| {
            if !cfg.theme_follow_system {
                cfg.theme_preset = id;
            } else if dark_now {
                cfg.theme_preset_dark = id;
            } else {
                cfg.theme_preset_light = id;
            }
        });
        self.after_theme_change(window, cx);
    }

    pub fn set_slot_preset(
        &mut self,
        dark_slot: bool,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_draft_dirty() {
            let id = id.to_string();
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_slot_preset(dark_slot, &id, window, cx)
            });
            return;
        }
        let id = id.to_string();
        self.update_config(cx, |cfg| {
            if dark_slot {
                cfg.theme_preset_dark = id;
            } else {
                cfg.theme_preset_light = id;
            }
        });
        self.after_theme_change(window, cx);
    }

    pub fn set_theme_follow_system(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_draft_dirty() {
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_theme_follow_system(on, window, cx)
            });
            return;
        }
        if on {
            let manual = cx.global::<Config>().theme_preset.clone();
            let manual_dark = anterm_ui_kit::presets::by_id(cx, &manual).dark;
            self.update_config(cx, |cfg| {
                cfg.theme_follow_system = true;
                if manual_dark {
                    cfg.theme_preset_dark = manual;
                } else {
                    cfg.theme_preset_light = manual;
                }
            });
        } else {
            let effective = anterm_ui_kit::theme::effective_preset_id(cx);
            self.update_config(cx, |cfg| {
                cfg.theme_follow_system = false;
                cfg.theme_preset = effective;
            });
        }
        self.after_theme_change(window, cx);
    }

    pub fn set_theme_mode(
        &mut self,
        mode: ThemeMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_draft_dirty() {
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.set_theme_mode(mode, window, cx)
            });
            return;
        }
        match mode {
            ThemeMode::System => {
                if !cx.global::<Config>().theme_follow_system {
                    self.set_theme_follow_system(true, window, cx);
                }
            }
            ThemeMode::Light | ThemeMode::Dark => {
                self.update_config(cx, |cfg| {
                    cfg.theme_follow_system = false;
                    cfg.theme_preset = if mode == ThemeMode::Dark {
                        cfg.theme_preset_dark.clone()
                    } else {
                        cfg.theme_preset_light.clone()
                    };
                });
                self.after_theme_change(window, cx);
            }
        }
    }

    pub fn pick_slot_theme(
        &mut self,
        dark_slot: bool,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<Config>().theme_follow_system {
            self.set_slot_preset(dark_slot, id, window, cx);
            return;
        }
        if !self.theme_draft_dirty() {
            let id_owned = id.to_string();
            ConfigStore::update_in_memory(cx, |cfg| {
                if dark_slot {
                    cfg.theme_preset_dark = id_owned;
                } else {
                    cfg.theme_preset_light = id_owned;
                }
            });
        }
        self.set_preset(id, window, cx);
    }

    pub fn set_theme_legible_palette(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.theme_legible_palette = on);
        self.after_theme_change(window, cx);
    }

    fn after_theme_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        apply_theme(Some(window), cx);
        set_menus(cx);
        self.rebuild_theme_editor(window, cx);
        cx.notify();
    }

    pub fn open_themes_folder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = anterm_ui_kit::presets::themes_dir() else {
            log::warn!("no config directory, so no themes folder to open");
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("could not create {}: {e}", dir.display());
            anterm_ui_kit::host_ops::HostOps::notify_err(
                window,
                cx,
                &t_fmt(
                    L10nKey::OpenInFileManagerFailed,
                    &[("path", &dir.display().to_string())],
                ),
                &e,
            );
            return;
        }
        cx.open_with_system(&dir);
    }

    pub fn theme_draft_dirty(&self) -> bool {
        self.state.theme_draft.is_some()
    }

    pub fn save_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, draft)) = self.state.theme_draft.clone() else {
            return true;
        };
        if let Err(error) = anterm_ui_kit::presets::write_theme_file(&draft) {
            self.state.theme_draft_error = Some(error.to_string());
            anterm_ui_kit::host_ops::HostOps::notify_err(
                window,
                cx,
                t(L10nKey::ThemeSaveFailed),
                &error,
            );
            return false;
        }
        self.state.theme_draft = None;
        self.state.theme_draft_error = None;
        cx.notify();
        true
    }

    pub fn cancel_theme_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.theme_draft_error = None;
        if let Some((original, _)) = self.state.theme_draft.take() {
            let mut themes = anterm_ui_kit::presets::all(cx);
            if let Some(slot) = themes.iter_mut().find(|t| t.id == original.id) {
                *slot = original;
            }
            cx.set_global(anterm_ui_kit::presets::Themes(themes));
            apply_theme(Some(window), cx);
            self.rebuild_theme_editor(window, cx);
            cx.notify();
        }
    }

    fn mutate_active_theme(
        &mut self,
        mutate: impl FnOnce(&mut anterm_ui_kit::presets::Theme),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = anterm_ui_kit::theme::effective_preset_id(cx);
        let mut theme = anterm_ui_kit::presets::by_id(cx, &id);
        if !theme.editable() {
            return;
        }
        mutate(&mut theme);
        let original = self
            .state
            .theme_draft
            .as_ref()
            .filter(|(original, _)| original.id == id)
            .map(|(original, _)| original.clone())
            .unwrap_or_else(|| anterm_ui_kit::presets::by_id(cx, &id));
        if anterm_ui_kit::presets::to_yaml(&original)
            == anterm_ui_kit::presets::to_yaml(&theme)
        {
            self.state.theme_draft = None;
        } else {
            self.state.theme_draft = Some((original, theme.clone()));
        }
        let mut themes = anterm_ui_kit::presets::all(cx);
        if let Some(slot) = themes.iter_mut().find(|t| t.id == id) {
            *slot = theme;
        }
        cx.set_global(anterm_ui_kit::presets::Themes(themes));
        apply_theme(Some(window), cx);
        cx.notify();
    }

    pub fn edit_active_theme(
        &mut self,
        edit: ThemeEdit,
        value: gpui::Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let c = hsla_to_u32(value);
        self.mutate_active_theme(
            |theme| match edit {
                ThemeEdit::Background => theme.background = Fill::Solid(c),
                ThemeEdit::Foreground => theme.foreground = c,
                ThemeEdit::Accent => theme.accent = c,
                ThemeEdit::Cursor => theme.caret = Some(c),
                ThemeEdit::Selection => theme.selection = Some(c),
                ThemeEdit::Ansi(i) => theme.ansi16[i] = ((c >> 16) as u8, (c >> 8) as u8, c as u8),
            },
            window,
            cx,
        );
    }

    pub fn effective_window_opacity(cx: &App) -> f32 {
        let config = cx.global::<Config>();
        let theme = anterm_ui_kit::presets::by_id(
            cx,
            &anterm_ui_kit::theme::effective_preset_id(cx),
        );
        let blur = config.window_blur.unwrap_or(theme.blur);
        config.window_opacity.or(theme.opacity).unwrap_or_else(|| {
            anterm_ui_kit::theme::default_window_opacity(config.window_backdrop, blur)
        })
    }

    pub fn set_window_blur(
        &mut self,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.window_blur = Some(on));
        apply_theme(Some(window), cx);
    }

    #[cfg(target_os = "windows")]
    pub fn set_window_backdrop(
        &mut self,
        backdrop: WindowBackdrop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_config(cx, |cfg| cfg.window_backdrop = backdrop);
        apply_theme(Some(window), cx);
    }

    pub fn reset_window_overrides(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_config(cx, |config| {
            clear_window_override_values(config, cfg!(target_os = "windows"));
        });
        apply_theme(Some(window), cx);
    }

    pub fn pick_theme_image(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.mutate_active_theme(
                        |theme| {
                            let opacity = theme.image.as_ref().map(|i| i.opacity).unwrap_or(0.3);
                            theme.image = Some(anterm_ui_kit::presets::Image { path, opacity });
                        },
                        window,
                        cx,
                    );
                    this.rebuild_theme_editor(window, cx);
                });
            }
        })
        .detach();
    }

    pub fn remove_theme_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mutate_active_theme(|theme| theme.image = None, window, cx);
        self.rebuild_theme_editor(window, cx);
    }

    pub fn set_theme_image_opacity(
        &mut self,
        v: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_active_theme(
            |theme| {
                if let Some(img) = theme.image.as_mut() {
                    img.opacity = v.clamp(0.0, 1.0);
                }
            },
            window,
            cx,
        );
    }

    pub fn rebuild_theme_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = anterm_ui_kit::theme::effective_preset_id(cx);
        let theme = anterm_ui_kit::presets::by_id(cx, &id);
        if !theme.editable() {
            self.state.theme_editor = None;
            return;
        }

        let neutrals = theme.neutrals();
        let seed_specs: [(ThemeEdit, u32); 5] = [
            (ThemeEdit::Background, theme.background_color()),
            (ThemeEdit::Foreground, theme.foreground),
            (ThemeEdit::Accent, theme.accent),
            (ThemeEdit::Cursor, theme.caret.unwrap_or(theme.accent)),
            (ThemeEdit::Selection, neutrals.selection),
        ];

        let mut subs = Vec::new();
        let mut make =
            |edit: ThemeEdit, value: u32, subs: &mut Vec<Subscription>, cx: &mut Context<Self>| {
                let eff: gpui::Hsla = gpui::rgb(value).into();
                let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(eff));
                subs.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _picker, ev: &ColorPickerEvent, window, cx| {
                        let ColorPickerEvent::Change(value) = ev;
                        if let Some(v) = value {
                            this.edit_active_theme(edit, *v, window, cx);
                        }
                    },
                ));
                state
            };

        let seed = seed_specs
            .iter()
            .map(|&(edit, value)| (edit, make(edit, value, &mut subs, cx)))
            .collect();
        let ansi = (0..16)
            .map(|i| {
                let (r, g, b) = theme.ansi16[i];
                let value = (r as u32) << 16 | (g as u32) << 8 | b as u32;
                (
                    ThemeEdit::Ansi(i),
                    make(ThemeEdit::Ansi(i), value, &mut subs, cx),
                )
            })
            .collect();

        self.state.theme_editor = Some(ThemeEditor {
            for_id: theme.id.clone(),
            seed,
            ansi,
            _subs: subs,
        });
    }

    // ── Language, shell, working directory, proxy, file command ──────────

    pub fn normalize_gui_language(code: &str) -> &'static str {
        anterm_i18n::find_language(code)
            .map(|lang| lang.code)
            .unwrap_or_else(anterm_i18n::default_language_code)
    }

    pub fn set_gui_language(
        &mut self,
        code: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let code = Self::normalize_gui_language(code);
        set_locale(code);
        self.update_config(cx, |cfg| cfg.gui_language = code.to_string());
        set_menus(cx);
        anterm_ui_kit::explorer_context_menu::refresh_labels();
        self.refresh_locale(window, cx);
        anterm_ui_kit::app_hooks::refresh_locale(
            cx,
            Some(window.window_handle()),
        );
    }

    pub fn build_shell_inputs(
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>, Entity<InputState>) {
        let cfg = cx.global::<Config>();
        let (shell_program, shell_args) = match &cfg.shell {
            Some(s) => (s.program.clone(), join_shell_args(&s.args)),
            None => (String::new(), String::new()),
        };
        let wd_path = cfg.working_directory.path.clone();
        let platform_default = if cfg!(windows) {
            "PowerShell"
        } else {
            t(L10nKey::AppPlaceholderLoginShell)
        };
        let shell_program_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(platform_default)
                .default_value(shell_program)
        });
        let shell_args_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(L10nKey::AppPlaceholderNone))
                .default_value(shell_args)
        });
        let wd_path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/path/to/directory")
                .default_value(wd_path)
        });
        let commit_shell = |this: &mut Self, ev: &InputEvent, cx: &mut Context<Self>| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_shell(cx);
            }
        };
        let commit_wd = |this: &mut Self, ev: &InputEvent, cx: &mut Context<Self>| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_working_directory_path(cx);
            }
        };
        subs.push(
            cx.subscribe_in(&shell_program_input, window, move |this, _i, ev, _w, cx| {
                commit_shell(this, ev, cx)
            }),
        );
        subs.push(
            cx.subscribe_in(&shell_args_input, window, move |this, _i, ev, _w, cx| {
                commit_shell(this, ev, cx)
            }),
        );
        subs.push(
            cx.subscribe_in(&wd_path_input, window, move |this, _i, ev, _w, cx| {
                commit_wd(this, ev, cx)
            }),
        );
        (shell_program_input, shell_args_input, wd_path_input)
    }

    pub fn build_link_file_command_input(
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = cx
            .global::<Config>()
            .link_file_command
            .clone()
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t(L10nKey::AppPlaceholderOpenInDefaultApp))
                .default_value(value)
        });
        subs.push(
            cx.subscribe_in(&input, window, move |this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_link_file_command(cx);
                }
            }),
        );
        input
    }

    pub fn build_http_proxy_input(
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let value = cx.global::<Config>().http_proxy.clone().unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://127.0.0.1:7890")
                .default_value(value)
        });
        subs.push(
            cx.subscribe_in(&input, window, move |this, _i, ev, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit_http_proxy(cx);
                }
            }),
        );
        input
    }

    fn commit_http_proxy(&mut self, cx: &mut Context<Self>) {
        let value = self
            .state
            .http_proxy_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        if !value.is_empty() && !anterm_core::daemon::install::proxy::is_valid_manual(&value) {
            cx.notify();
            return;
        }
        let value = (!value.is_empty()).then_some(value);
        if cx.global::<Config>().http_proxy == value {
            return;
        }
        self.update_config(cx, |cfg| cfg.http_proxy = value);
    }

    fn commit_link_file_command(&mut self, cx: &mut Context<Self>) {
        let command = self
            .state
            .link_file_command_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let command = (!command.is_empty()).then_some(command);
        if cx.global::<Config>().link_file_command == command {
            return;
        }
        self.update_config(cx, |cfg| cfg.link_file_command = command);
    }

    pub fn commit_shell_from_picker(&mut self, cx: &mut Context<Self>) {
        self.commit_shell(cx);
        cx.notify();
    }

    fn commit_shell(&mut self, cx: &mut Context<Self>) {
        let program = self
            .state
            .shell_program_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let raw_args = self.state.shell_args_input.read(cx).value();
        let args: Vec<String> = match split_shell_args(&raw_args) {
            Ok(args) => args,
            Err(_) => {
                cx.notify();
                cx.global::<Config>()
                    .shell
                    .as_ref()
                    .map(|s| s.args.clone())
                    .unwrap_or_default()
            }
        };
        let shell = (!program.is_empty()).then_some(ShellConfig { program, args });
        if cx.global::<Config>().shell == shell {
            return;
        }
        self.update_config(cx, |cfg| cfg.shell = shell);
        anterm_ui_kit::app_hooks::refresh_shells(cx);
    }

    pub fn set_working_directory_strategy(
        &mut self,
        strategy: WdStrategy,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<Config>().working_directory.strategy == strategy {
            return;
        }
        self.update_config(cx, |cfg| cfg.working_directory.strategy = strategy);
    }

    fn commit_working_directory_path(&mut self, cx: &mut Context<Self>) {
        let path = self.state.wd_path_input.read(cx).value().trim().to_string();
        if !wd_path_saveable(&path) {
            cx.notify();
            return;
        }
        if cx.global::<Config>().working_directory.path == path {
            return;
        }
        self.update_config(cx, |cfg| cfg.working_directory.path = path);
    }

    // ── Reset individual setting row ─────────────────────────────────────

    pub fn reset_settings_value(
        &mut self,
        title: L10nKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let defaults = Config::default();
        match title {
            L10nKey::SettingsDimInactivePanes => {
                self.set_dim_inactive_panes(defaults.dim_inactive_panes, cx)
            }
            L10nKey::SettingsAutoHideTitlebarButtons => {
                self.set_auto_hide_titlebar_buttons(defaults.auto_hide_titlebar_buttons, cx)
            }
            L10nKey::SettingsCursorBlink => self.set_cursor_blink(defaults.cursor_blink, cx),
            L10nKey::SettingsCursorShape => self.set_cursor_style(defaults.cursor_style, cx),
            L10nKey::SettingsPromptCursorShape => {
                self.set_prompt_cursor_style(defaults.prompt_cursor_style, cx)
            }
            L10nKey::SettingsScrollback => self.set_scrollback_limit(defaults.scrollback_limit, cx),
            L10nKey::SettingsNewTabPosition => {
                self.set_new_tab_position(defaults.new_tab_position, cx)
            }
            L10nKey::SettingsConfirmClose => self.set_confirm_close(defaults.confirm_close, cx),
            L10nKey::SettingsTabBarPosition => {
                self.set_tab_bar_position(defaults.tab_bar_position, cx)
            }
            L10nKey::SettingsEditorHelixMode => self.set_helix_mode(defaults.helix_mode, cx),
            L10nKey::SettingsEditorGitGutter => {
                self.set_editor_git_gutter(defaults.editor_git_gutter, cx)
            }
            L10nKey::SettingsEditorLsp => self.set_editor_lsp(defaults.editor_lsp, cx),
            L10nKey::SettingsEditorSoftWrap => {
                self.set_editor_soft_wrap(defaults.editor_soft_wrap, cx)
            }
            L10nKey::SettingsEditorMarkdownPreview => {
                self.set_editor_markdown_preview(defaults.editor_markdown_preview, cx)
            }
            L10nKey::SettingsNotifyOnCommandFinish => {
                self.set_notify_mode(defaults.notify_on_command_finish, cx)
            }
            L10nKey::SettingsNotifyThreshold => {
                self.set_notify_threshold(defaults.notify_threshold_secs, cx)
            }
            L10nKey::SettingsTerminalBell => self.set_bell_mode(defaults.bell, cx),
            L10nKey::SettingsRestoreLastLayout => {
                self.set_restore_session(defaults.restore_session, cx)
            }
            L10nKey::SettingsPerPaneHistory => {
                self.set_per_pane_history(defaults.per_pane_history, cx)
            }
            L10nKey::SettingsShowTrayIcon => self.set_show_tray_icon(defaults.show_tray_icon, cx),
            L10nKey::SettingsOptionAsMeta => {
                self.set_macos_option_as_alt(defaults.macos_option_as_alt, cx)
            }
            L10nKey::SettingsHideMouseWhileTyping => {
                self.set_mouse_hide_while_typing(defaults.mouse_hide_while_typing, cx)
            }
            L10nKey::SettingsFocusFollowsMouse => {
                self.set_focus_follows_mouse(defaults.focus_follows_mouse, cx)
            }
            L10nKey::SettingsReportMouseToApps => {
                self.set_mouse_reporting(defaults.mouse_reporting, cx)
            }
            L10nKey::SettingsScrollSpeed => {
                self.set_mouse_scroll_multiplier(defaults.mouse_scroll_multiplier, cx)
            }
            L10nKey::SettingsSmoothScroll => self.set_smooth_scroll(defaults.smooth_scroll, cx),
            L10nKey::SettingsMouseZoom => {
                self.set_mouse_zoom_modifier(defaults.mouse_zoom_modifier, cx)
            }
            L10nKey::SettingsTrimTrailingSpaces => {
                self.set_clipboard_trim(defaults.clipboard_trim_trailing_spaces, cx)
            }
            L10nKey::SettingsCopyJoinWrapped => {
                self.update_config(cx, |c| c.copy_join_wrapped = defaults.copy_join_wrapped)
            }
            L10nKey::SettingsCopyOnSelect => self.set_copy_on_select(defaults.copy_on_select, cx),
            L10nKey::SettingsSmartSelection => self.set_smart_select(defaults.smart_select, cx),
            L10nKey::SettingsPromptEditor => self.set_prompt_editor(defaults.prompt_editor, cx),
            L10nKey::SettingsTabCompletion => self.set_tab_completion(defaults.tab_completion, cx),
            L10nKey::SettingsHistorySearch => self.set_history_search(defaults.history_search, cx),
            L10nKey::SettingsStartupWindow => self.set_startup_mode(defaults.startup_mode, cx),
            L10nKey::SettingsRememberWindowSize => {
                self.set_remember_window_size(defaults.remember_window_size, cx)
            }
            L10nKey::SettingsCheckUpdatesOnLaunch => {
                self.set_check_for_updates(defaults.check_for_updates, cx)
            }
            L10nKey::SettingsAutoDownload => {
                self.set_auto_download_updates(defaults.auto_download_updates, cx)
            }
            L10nKey::SettingsUpdateChannel => self.set_update_channel(defaults.update_channel, cx),
            L10nKey::DetectUrls => self.set_link_url(defaults.link_url, cx),
            L10nKey::ForwardSshLoopbackLinks => {
                self.set_ssh_loopback_forward(defaults.ssh_loopback_forward, cx)
            }
            L10nKey::SettingsVerifyHostKeys => {
                self.set_verify_host_keys(defaults.verify_host_keys, cx)
            }
            L10nKey::WarnBeforeClosing => {
                self.set_ssh_warn_on_close(defaults.ssh_warn_on_close, cx)
            }
            L10nKey::SettingsLanguage => self.set_gui_language(
                Self::normalize_gui_language(&defaults.gui_language),
                window,
                cx,
            ),
            L10nKey::SettingsFontSize => self.reset_font_size(cx),
            L10nKey::SettingsUiFontSize => self.reset_ui_font_size(cx),
            L10nKey::SettingsLineHeight => self.reset_line_height(cx),
            L10nKey::SettingsFontFamily => {
                self.commit_font_family(defaults.font_family.clone(), cx)
            }
            L10nKey::SettingsBoldFont => {
                self.commit_font_family_emphasis(true, font_default_label().to_string(), cx)
            }
            L10nKey::SettingsItalicFont => {
                self.commit_font_family_emphasis(false, font_default_label().to_string(), cx)
            }
            L10nKey::SettingsUiFontFamily => {
                self.commit_ui_font_family(ui_font_default_label().to_string(), window, cx)
            }
            L10nKey::SettingsSyncWithSystem => {
                self.set_theme_follow_system(defaults.theme_follow_system, window, cx)
            }
            L10nKey::SettingsLegiblePalette => {
                self.set_theme_legible_palette(defaults.theme_legible_palette, window, cx)
            }
            L10nKey::SettingsOpacity => {
                self.update_config(cx, |cfg| cfg.window_opacity = None);
                apply_theme(Some(window), cx);
            }
            L10nKey::SettingsBlur => {
                self.update_config(cx, |cfg| cfg.window_blur = None);
                apply_theme(Some(window), cx);
            }
            #[cfg(target_os = "windows")]
            L10nKey::SettingsBackdrop => {
                self.set_window_backdrop(defaults.window_backdrop, window, cx)
            }
            L10nKey::SettingsFontLigatures => self.set_font_ligatures(
                defaults
                    .font_features
                    .as_ref()
                    .is_some_and(|f| f.is_calt_enabled() == Some(true)),
                cx,
            ),
            L10nKey::OpenFilesWith => {
                self.update_config(cx, |cfg| cfg.link_file_open = defaults.link_file_open);
            }
            L10nKey::SettingsProgram => {
                self.update_config(cx, |cfg| cfg.shell = defaults.shell.clone())
            }
            L10nKey::SettingsArguments => self.update_config(cx, |cfg| {
                if let Some(shell) = &mut cfg.shell {
                    shell.args.clear();
                }
            }),
            L10nKey::SettingsStartIn => self.update_config(cx, |cfg| {
                cfg.working_directory.strategy = defaults.working_directory.strategy
            }),
            L10nKey::SettingsCustomPath => self.update_config(cx, |cfg| {
                cfg.working_directory.path = defaults.working_directory.path.clone()
            }),
            L10nKey::SettingsAppHttpProxy => {
                self.update_config(cx, |cfg| cfg.http_proxy = defaults.http_proxy.clone())
            }
            L10nKey::SettingsOpenFilesCommand => self.update_config(cx, |cfg| {
                cfg.link_file_command = defaults.link_file_command.clone()
            }),
            _ => return,
        }
        self.refresh_settings_controls(title, window, cx);
    }

    fn refresh_settings_controls(
        &mut self,
        title: L10nKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut subs = Vec::new();
        match title {
            L10nKey::SettingsProgram | L10nKey::SettingsArguments => {
                let (program, args, _) = Self::build_shell_inputs(&mut subs, window, cx);
                self.state.shell_program_input = program;
                self.state.shell_args_input = args;
            }
            L10nKey::SettingsCustomPath => {
                let value = cx.global::<Config>().working_directory.path.clone();
                self.state
                    .wd_path_input
                    .clone()
                    .update(cx, |s, cx| s.set_value(value, window, cx));
            }
            L10nKey::SettingsOpenFilesCommand => {
                self.state.link_file_command_input =
                    Self::build_link_file_command_input(&mut subs, window, cx);
            }
            L10nKey::SettingsAppHttpProxy => {
                self.state.http_proxy_input = Self::build_http_proxy_input(&mut subs, window, cx);
            }
            _ => {}
        }
        self.state._subs.extend(subs);
        cx.notify();
    }

    // ── Section selection & search ───────────────────────────────────────

    pub fn select_settings_section(
        &mut self,
        target: SettingsSection,
        cx: &mut Context<Self>,
    ) {
        let moved = self.state.section != target;
        self.state.section = target;
        self.state.recording = None;
        if moved {
            self.state.reveal_first_hit.set(true);
        }
        if target == SettingsSection::Agents {
            self.state.agent_hooks_states = AgentHooksView::Loading;
        }
        self.ensure_agent_hooks_loaded(cx);
        cx.notify();
    }

    pub fn autoselect_settings_search(&mut self, cx: &mut Context<Self>) {
        let active = !self.state.search.read(cx).value().trim().is_empty();
        if active && !self.state.search_active {
            self.state.search_return_offset = self.state.content_scroll.offset();
        }
        if active {
            self.state.content_scroll.set_offset(gpui::point(px(0.), px(0.)));
        } else if self.state.search_active {
            self.state
                .content_scroll
                .set_offset(self.state.search_return_offset);
        }
        self.state.search_active = active;
        self.state.search_selection = 0;
        if active {
            self.state.focused_setting = None;
        }
        cx.notify();
    }

    // ── Agent hooks ──────────────────────────────────────────────────────

    pub fn ensure_agent_hooks_loaded(&mut self, cx: &mut Context<Self>) {
        if self.state.section == SettingsSection::Agents {
            self.load_agent_hooks_states(cx);
        }
    }

    pub fn agent_hooks_machines(&self, _cx: &mut App) -> Vec<AgentHooksMachine> {
        vec![AgentHooksMachine {
            host: anterm_ui_kit::host_ops::HostId::LOCAL,
            label: t(L10nKey::AppAgentHooksThisComputer).to_string(),
        }]
    }

    pub fn agent_hooks_offline_count(&self, _cx: &mut App) -> usize {
        0
    }

    pub fn select_agent_hooks_host(
        &mut self,
        host: anterm_ui_kit::host_ops::HostId,
        cx: &mut Context<Self>,
    ) {
        if self.state.agent_hooks_host == host {
            return;
        }
        self.state.agent_hooks_host = host;
        self.state.agent_hooks_note = None;
        self.state.agent_hooks_states = AgentHooksView::Loading;
        self.load_agent_hooks_states(cx);
        cx.notify();
    }

    fn load_agent_hooks_states(&mut self, cx: &mut Context<Self>) {
        use anterm_core::agent_hooks::{HookAgent, HookTarget};

        let host_id = self.state.agent_hooks_host;
        self.state.agent_hooks_seq += 1;
        let seq = self.state.agent_hooks_seq;
        let Some((host, home)) = self.agent_hooks_link(host_id, cx) else {
            self.state.agent_hooks_states =
                AgentHooksView::Unavailable(Self::agent_hooks_offline_msg());
            cx.notify();
            return;
        };

        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                let target = match &home {
                    Some(home) => HookTarget::remote(h, home.clone()),
                    None => HookTarget::local(h)?,
                };
                Some(
                    HookAgent::ALL
                        .into_iter()
                        .map(|agent| AgentHookRow {
                            agent,
                            state: anterm_core::agent_hooks::hooks_state(&target, agent),
                            target: agent.target_display(&target),
                        })
                        .collect::<Vec<_>>(),
                )
            },
            move |this, rows, cx| {
                if this.state.agent_hooks_seq == seq {
                    this.state.agent_hooks_states = match rows {
                        Some(rows) => AgentHooksView::Ready(rows),
                        None => AgentHooksView::Unavailable(
                            t(L10nKey::AppAgentHooksNoHomeDir).to_string(),
                        ),
                    };
                    cx.notify();
                }
            },
        );
    }

    fn agent_hooks_offline_msg() -> String {
        t(L10nKey::AppAgentHooksOffline).to_string()
    }

    fn agent_hooks_link(
        &self,
        host_id: anterm_ui_kit::host_ops::HostId,
        cx: &mut App,
    ) -> Option<(anterm_ui_kit::host_ops::SharedHost, Option<std::path::PathBuf>)> {
        if !host_id.is_local() {
            return None;
        }
        let host = anterm_ui_kit::host_registry::HostRegistry::get(cx, host_id)?;
        Some((host, None))
    }

    pub fn settings_install_agent_hooks(
        &mut self,
        agent: anterm_core::agent_hooks::HookAgent,
        cx: &mut Context<Self>,
    ) {
        self.run_agent_hooks_action(agent, true, cx);
    }

    pub fn settings_uninstall_agent_hooks(
        &mut self,
        agent: anterm_core::agent_hooks::HookAgent,
        cx: &mut Context<Self>,
    ) {
        self.run_agent_hooks_action(agent, false, cx);
    }

    fn agent_hooks_outcome_msg(outcome: &anterm_core::agent_hooks::HookOutcome) -> String {
        use anterm_core::agent_hooks::HookOutcome as O;
        match outcome {
            O::Installed => t(L10nKey::AppAgentHooksInstalled).to_string(),
            O::MuseInstallManually(command) => t_fmt(
                L10nKey::AppAgentHooksMuseManualInstall,
                &[("command", command)],
            ),
            O::InstalledEnableCodexThere => {
                t(L10nKey::AppAgentHooksInstalledEnableCodexThere).to_string()
            }
            O::InstalledCodexEnableFailed(e) => t_fmt(
                L10nKey::AppAgentHooksInstalledCodexEnableFailed,
                &[("error", e)],
            ),
            O::Removed => t(L10nKey::AppAgentHooksRemoved).to_string(),
            O::NothingInstalled => t(L10nKey::AppAgentHooksNothingInstalled).to_string(),
            O::NoAntermHooks => t(L10nKey::AppAgentHooksNoAntermHooks).to_string(),
        }
    }

    fn run_agent_hooks_action(
        &mut self,
        agent: anterm_core::agent_hooks::HookAgent,
        install: bool,
        cx: &mut Context<Self>,
    ) {
        use anterm_core::agent_hooks::HookTarget;

        let host_id = self.state.agent_hooks_host;
        let Some((host, home)) = self.agent_hooks_link(host_id, cx) else {
            self.state.agent_hooks_note = Some((agent, Self::agent_hooks_offline_msg()));
            self.state.agent_hooks_states =
                AgentHooksView::Unavailable(Self::agent_hooks_offline_msg());
            cx.notify();
            return;
        };

        self.state.agent_busy.insert(agent);
        self.state.agent_touched.insert(agent);
        cx.notify();
        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                let target = match &home {
                    Some(home) => HookTarget::remote(h, home.clone()),
                    None => HookTarget::local(h).ok_or_else(|| {
                        anyhow::anyhow!("{}", t(L10nKey::AppAgentHooksHomeDirUnresolved))
                    })?,
                };
                if install {
                    anterm_core::agent_hooks::install_hooks(&target, agent)
                } else {
                    anterm_core::agent_hooks::uninstall_hooks(&target, agent)
                }
            },
            move |this, result, cx| {
                this.state.agent_busy.remove(&agent);
                this.state.agent_hooks_note = Some((
                    agent,
                    match result {
                        Ok(outcome) => Self::agent_hooks_outcome_msg(&outcome),
                        Err(e) => t_fmt(
                            match install {
                                true => L10nKey::AppAgentHooksInstallFailed,
                                false => L10nKey::AppAgentHooksRemoveFailed,
                            },
                            &[("error", &e.to_string())],
                        ),
                    },
                ));
                this.load_agent_hooks_states(cx);
                cx.notify();
            },
        );
    }

    // ── Keybinding recorder & overrides ──────────────────────────────────

    pub fn start_recording_key(
        &mut self,
        action: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |ev, _window, cx| {
            let keystroke = ev.keystroke.clone();
            let _ = this.update(cx, |this, cx| this.on_record_key(&keystroke, cx));
            cx.stop_propagation();
        });
        self.record_gen = self.record_gen.wrapping_add(1);
        self.state.rebinding_note = None;
        self.state.kb_conflict = None;
        self.state.recording = Some(Recording {
            action,
            chords: Vec::new(),
            _intercept: intercept,
        });
        cx.notify();
    }

    fn on_record_key(&mut self, keystroke: &gpui::Keystroke, cx: &mut Context<Self>) {
        let Some((action, has_chords)) = self
            .state
            .recording
            .as_ref()
            .map(|r| (r.action.clone(), !r.chords.is_empty()))
        else {
            return;
        };
        match keystroke.key.as_str() {
            "escape" => {
                self.stop_recording(cx);
                return;
            }
            "backspace" | "delete" => {
                if has_chords {
                    if let Some(r) = self.state.recording.as_mut() {
                        r.chords.pop();
                    }
                    let still_has = self
                        .state
                        .recording
                        .as_ref()
                        .is_some_and(|r| !r.chords.is_empty());
                    if still_has {
                        self.schedule_recording_commit(cx);
                    } else {
                        self.record_gen = self.record_gen.wrapping_add(1);
                    }
                    cx.notify();
                } else {
                    self.stop_recording(cx);
                    self.unbind_keybinding(action, cx);
                }
                return;
            }
            _ => {}
        }
        let Some(spec) = anterm_ui_kit::keymap::spec_from_keystroke(keystroke) else {
            return;
        };
        if let Some(r) = self.state.recording.as_mut() {
            r.chords.push(spec);
        }
        self.schedule_recording_commit(cx);
        cx.notify();
    }

    fn schedule_recording_commit(&mut self, cx: &mut Context<Self>) {
        self.record_gen = self.record_gen.wrapping_add(1);
        let generation = self.record_gen;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(RECORD_COMMIT_DELAY_MS)).await;
            let _ = this.update(cx, |this, cx| {
                if this.record_gen == generation {
                    this.commit_recording(cx);
                }
            });
        })
        .detach();
    }

    fn commit_recording(&mut self, cx: &mut Context<Self>) {
        let Some((action, chords)) = self
            .state
            .recording
            .as_ref()
            .filter(|r| !r.chords.is_empty())
            .map(|r| (r.action.clone(), r.chords.clone()))
        else {
            return;
        };
        self.stop_recording(cx);
        let spec = chords.join(" ");
        if let Some(other) = Self::keybinding_owner(&action, &spec, cx) {
            self.state.kb_conflict = Some(KeyConflict {
                action,
                spec,
                other,
            });
            cx.notify();
            return;
        }
        self.assign_keybinding(action, spec, cx);
    }

    fn keybinding_owner(action: &str, spec: &str, cx: &App) -> Option<String> {
        use anterm_ui_kit::keymap::same_chord;
        anterm_ui_kit::keymap::effective_chords(cx)
            .into_iter()
            .find(|(a, chords)| a != action && chords.iter().any(|k| same_chord(k, spec)))
            .map(|(a, _)| a)
            .or_else(|| {
                anterm_ui_kit::keymap::extra_bindings(cx)
                    .into_iter()
                    .find(|(a, k)| a != action && same_chord(k, spec))
                    .map(|(a, _)| a)
            })
    }

    pub fn resolve_keybinding_conflict(&mut self, replace: bool, cx: &mut Context<Self>) {
        let Some(conflict) = self.state.kb_conflict.take() else {
            return;
        };
        if replace {
            self.assign_keybinding(conflict.action, conflict.spec, cx);
            self.state.rebinding_note = None;
        }
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.record_gen = self.record_gen.wrapping_add(1);
        self.state.recording = None;
        cx.notify();
    }

    fn assign_keybinding(&mut self, action: String, spec: String, cx: &mut Context<Self>) {
        use anterm_ui_kit::keymap::same_chord;
        let displaced: Option<(String, Vec<String>)> =
            anterm_ui_kit::keymap::effective_chords(cx)
                .into_iter()
                .find(|(a, chords)| *a != action && chords.iter().any(|k| same_chord(k, &spec)))
                .map(|(a, chords)| {
                    let rest = chords.into_iter().filter(|k| !same_chord(k, &spec));
                    (a, rest.collect())
                })
                .or_else(|| {
                    anterm_ui_kit::keymap::extra_bindings(cx)
                        .into_iter()
                        .find(|(a, k)| *a != action && same_chord(k, &spec))
                        .map(|(a, _)| (a, Vec::new()))
                });
        let in_prose = |name: &str| name.trim_end_matches('…').to_string();
        let note = displaced.as_ref().map(|(other, _)| {
            t_fmt(
                L10nKey::AppKeybindingDisplacedNote,
                &[
                    (
                        "action",
                        &in_prose(&anterm_ui_kit::keymap::action_entry(&action).1),
                    ),
                    (
                        "previous",
                        &in_prose(&anterm_ui_kit::keymap::action_entry(other).1),
                    ),
                ],
            )
        });
        self.update_config(cx, |cfg| {
            use anterm_ui_kit::config::KeybindingOverride;
            if let Some((other, rest)) = &displaced {
                cfg.keybindings
                    .insert(other.clone(), KeybindingOverride::Exact(rest.clone()));
            }
            cfg.keybindings
                .insert(action, KeybindingOverride::Exact(vec![spec]));
        });
        anterm_ui_kit::keymap::rebind(cx);
        self.state.rebinding_note = note;
        cx.notify();
    }

    pub fn unbind_keybinding(&mut self, action: String, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.keybindings.insert(
                action,
                anterm_ui_kit::config::KeybindingOverride::Exact(Vec::new()),
            );
        });
        anterm_ui_kit::keymap::rebind(cx);
        self.state.recording = None;
        self.state.rebinding_note = None;
        cx.notify();
    }

    pub fn reset_keybinding(&mut self, action: String, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            cfg.keybindings.remove(&action);
        });
        anterm_ui_kit::keymap::rebind(cx);
        self.state.recording = None;
        self.state.rebinding_note = None;
        cx.notify();
    }

    pub fn restore_default_keybindings_confirmed(&mut self, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| cfg.keybindings.clear());
        anterm_ui_kit::keymap::rebind(cx);
        self.state.recording = None;
        self.state.rebinding_note = None;
        cx.notify();
    }

    pub fn set_keybinding_preset(&mut self, preset: &str, cx: &mut Context<Self>) {
        let preset = preset.to_string();
        self.update_config(cx, |cfg| cfg.keybinding_preset = preset);
        anterm_ui_kit::keymap::rebind(cx);
        self.state.recording = None;
        self.state.rebinding_note = None;
        cx.notify();
    }

    pub fn set_keybinding_prefix(&mut self, prefix: &str, cx: &mut Context<Self>) {
        let prefix = prefix.to_string();
        self.update_config(cx, |cfg| cfg.prefix = prefix);
        anterm_ui_kit::keymap::rebind(cx);
        cx.notify();
    }
}
