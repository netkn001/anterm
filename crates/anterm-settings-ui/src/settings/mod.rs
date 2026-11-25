use gpui::{
    AnyElement, App, Background, Context, Div, ElementId, Entity, Focusable as _, FontWeight,
    KeyDownEvent, MouseButton, SharedString, Stateful, Subscription, Window, div, prelude::*, px,
    rgb,
};
use gpui_kit::component::InteractiveElementExt as _;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::link::Link;
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::{Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use std::cell::{Cell, RefCell};

use uuid::Uuid;

use anterm_ui_kit::config::{
    BellMode, Config, ConfirmClose, CursorStyle, LinkFileOpen, MouseZoomModifier, NewTabPosition,
    NotifyMode, TabBarPosition, UI_FONT_SIZE_DEFAULT, UpdateChannel, WindowBackdrop,
};
use anterm_ui_kit::keychain::{CredentialStore as _, OsCredentialStore, key_account_from_contents};
use anterm_core::ssh_profile::{SshProfile, to_connect_string};
use anterm_ui_kit::update::{localized_update_install_hint, localized_update_phase};
use anterm_ui_kit::intent::{FONT_SIZE_STEP, LINE_HEIGHT_STEP, ThemeEdit, UI_FONT_SIZE_STEP};
use anterm_ui_kit::window_chrome::TITLE_BAR_HEIGHT;
use anterm_ui_kit::host_ops::HostId;
use anterm_i18n::{L10nKey, t, t_fmt, t_plural};
use anterm_ui_kit::presets;

mod actions;
mod agents;
mod catalog;
mod editor;
mod forms;
mod hosts;
pub use anterm_ui_kit::kit;
mod pages;
mod shell;
pub use shell::segmented_control;
mod shortcuts;
mod theme_picker;
pub mod window;
pub use window::SettingsWindow;

#[cfg(test)]
mod tests;

pub use catalog::{
    PROMPT_CURSOR_SHAPES, keybinding_matches_query,
    prompt_cursor_shape_labels, section_match_count,
};
#[cfg(test)]
pub use catalog::{best_matching_section, total_match_count};
#[cfg(test)]
use catalog::keybinding_match_count;
use catalog::{SearchEntry, entry_matches, row_matches_query, settings_search_entries};

#[cfg(test)]
use forms::SshFieldError;
pub use forms::{
    ForwardRuleForm, SshFormDraft, SshFormErrors, SshProfileForm,
};
#[cfg(test)]
use forms::{
    DEFAULT_SOCKS_PORT, PasswordPlan, first_readable_key_or, parse_host_port_checked, tildify_with,
};
use forms::{
    DEFAULT_KEY_HINT, DEFAULT_SSH_PORT, first_readable_key, first_readable_key_in,
    forward_row_inputs, host_port_text, password_plan, seed_forward_row, seed_hinted, seed_input,
    seed_textarea,
    split_lines, stored_passphrase, stored_password, tildify, validate_ssh_draft,
};

/// The settings nav, and the page's padding either side of its column.
const NAV_W: f32 = 220.;
const PAGE_PAD: f32 = 112.;

/// The narrowest the nav is still itself: an item that still shows its label
/// beside its icon.
///
/// Sized for the longest nav label in *any* locale, not the one the developer
/// happens to be reading: at 140 the zh-CN "窗口与标签页" lost the right half
/// of its last character.
const NAV_W_MIN: f32 = 176.;

/// The reading column every page caps itself at. Wider than this and a
/// description stops being a paragraph and becomes a line to scan across.
const READING_COLUMN: f32 = 620.;

/// How far the content scrollbar stays clear of the top and bottom of the
/// window, so it does not run into the window's rounded corner.
const SCROLLBAR_WINDOW_INSET: f32 = 12.;

/// What the page gets before the nav gives anything back: a row's widest
/// control with a description beside it that still reads as a paragraph.
const CONTENT_W: f32 = 420.;

/// How wide the nav is at this window width: its full width while the page
/// beside it still gets `CONTENT_W`, then shrinking toward its floor.
fn nav_width(viewport: f32, scale: f32) -> f32 {
    let full = NAV_W * scale.max(1.);
    let floor = NAV_W_MIN * scale.max(1.);
    let short = (full + CONTENT_W + PAGE_PAD - viewport).max(0.);
    (full - short).clamp(floor, full).round()
}

/// What a row on the page really has to lay out in.
fn settings_row_width(viewport: f32, scale: f32) -> f32 {
    (viewport - nav_width(viewport, scale) - PAGE_PAD).clamp(0., READING_COLUMN * scale)
}

/// How much wider every piece of text on this page is than the px thresholds
/// below assume.
///
/// Those thresholds are widths a *label* needs, measured at the default
/// interface font. The interface has a font size of its own and it goes up to
/// 24 — half as wide again — while a slider or a text field beside that label
/// stays the px width it was built at. Without this the row that has to stack
/// first is the one that never does.
fn ui_scale(cx: &App) -> f32 {
    cx.global::<Config>().ui_font_size / UI_FONT_SIZE_DEFAULT
}

/// How wide a text field in the right-hand column is — the dropdowns' width,
/// so the column is one straight edge whichever control a row holds.
const FIELD_W: f32 = anterm_ui_kit::kit::CONTROL_W;

/// A field for a path or a command line: the one kind of value unreadable cut
/// short, so the one kind allowed past [`FIELD_W`].
const PATH_FIELD_W: f32 = 240.;

/// The host editor's label column, and how wide a field beside one grows to.
const SSH_LABEL_W: f32 = 96.;
const FORM_FIELD_W: f32 = 300.;

/// Width a settings row needs before its label and its control fit side by
/// side: a wide control, the gap between them, and enough left for a
/// description to read as prose rather than as a column of words.
const STACK_ROW_BELOW: f32 = 500.;

/// The scrollback presets, and the labels their cells carry, in draw order.
/// One list each so the number a cell writes is the number it shows —
/// `preset_row_labels_name_the_value_they_write` holds the two together.
const SCROLLBACK_BUCKETS: [usize; 3] = [1_000, 10_000, 100_000];
const SCROLLBACK_LABELS: [&str; 3] = ["1,000", "10,000", "100,000"];

/// The notify-threshold presets. The last one is drawn in minutes, which is
/// why these labels are written out rather than derived.
const NOTIFY_THRESHOLD_BUCKETS: [u64; 4] = [5, 10, 30, 60];
const NOTIFY_THRESHOLD_LABELS: [&str; 4] = ["5s", "10s", "30s", "1m"];

/// Which preset a live value *is*, and — when it is none of them — the label
/// for the trailing cell that names it.
///
/// The match is exact on purpose. Matching a *range* is what made
/// `scrollback_limit: 5000` light up "10,000" and `notify_threshold_secs: 20`
/// light up "30s", with no digits anywhere on the row to correct the
/// impression, and clicking the cell that was wrongly lit overwrote the real
/// value with the bucket's (#550).
fn preset_choice<T: Copy + PartialEq>(
    buckets: &[T],
    value: T,
    name: impl FnOnce(T) -> String,
) -> (Option<usize>, Option<String>) {
    match buckets.iter().position(|&b| b == value) {
        Some(ix) => (Some(ix), None),
        None => (
            None,
            Some(t_fmt(
                L10nKey::SettingsCustomValue,
                &[("value", &name(value))],
            )),
        ),
    }
}

/// `50000` beside cells reading `10,000` and `100,000` looks like a different
/// kind of number, so the custom cell groups its digits the way the presets
/// next to it are written. Every locale anterm ships writes these counts the
/// same way — the preset labels themselves are one set of literals for all
/// three.
fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.char_indices() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The wheel handler for a list that scrolls inside the settings page. GPUI
/// scrolls every scroller under the pointer, so put this on the list's content
/// child, where it runs before the viewport's own listener: one event belongs to
/// one scroller. Reaching an edge consumes the event, while starting at that
/// edge lets it bubble to the page.
fn nested_list_wheel(
    scroll: gpui::ScrollHandle,
) -> impl Fn(&gpui::ScrollEvent, &mut Window, &mut App) + 'static {
    move |event, window, cx| {
        let delta = event.delta.pixel_delta(window.line_height());
        let dy = if delta.y == px(0.) { delta.x } else { delta.y };
        let offset = scroll.offset();
        let y = (offset.y + dy).clamp(-scroll.max_offset().y, px(0.));
        if y != offset.y {
            scroll.set_offset(gpui::point(offset.x, y));
            window.refresh();
            cx.stop_propagation();
        }
    }
}

fn settings_row_id(label: &str, _desc: &str) -> SharedString {
    let id = settings_search_entries()
        .iter()
        .find(|entry| t(entry.title) == label)
        .map(|entry| format!("{:?}", entry.title))
        .or_else(|| {
            L10nKey::ALL
                .iter()
                .find(|&&key| t(key) == label)
                .map(|key| format!("{key:?}"))
        })
        .unwrap_or_else(|| label.to_string());
    SharedString::from(format!("settings-row-{id}"))
}

fn settings_header_id(title: &str) -> SharedString {
    let id = L10nKey::ALL
        .iter()
        .find(|&&key| t(key) == title)
        .map(|key| format!("{key:?}"))
        .unwrap_or_else(|| title.to_string());
    SharedString::from(format!("settings-header-{id}"))
}

/// Whether the reset control has any effective override to clear on this
/// platform. A synchronized Windows backdrop remains stored elsewhere but is
/// inert here, so only platforms that expose it locally may count it.
fn window_overrides_active(config: &Config, backdrop_is_local: bool) -> bool {
    config.window_opacity.is_some()
        || config.window_blur.is_some()
        || (backdrop_is_local && config.window_backdrop != WindowBackdrop::Auto)
}

pub use anterm_ui_kit::intent::SettingsSection;

pub struct ThemeEditor {
    #[allow(dead_code)]
    pub for_id: String,
    pub seed: Vec<(ThemeEdit, Entity<ColorPickerState>)>,
    pub ansi: Vec<(ThemeEdit, Entity<ColorPickerState>)>,
    pub _subs: Vec<Subscription>,
}

pub struct SettingsState {
    pub focus_handle: gpui::FocusHandle,
    pub section: SettingsSection,
    pub search: Entity<InputState>,
    pub shortcut_search: Entity<InputState>,
    pub save_error: Option<String>,
    pub saved_config: Config,
    pub search_active: bool,
    pub search_return_offset: gpui::Point<gpui::Pixels>,
    pub search_selection: usize,
    pub search_rows: RefCell<Option<Vec<(L10nKey, AnyElement)>>>,
    pub focused_setting: Option<L10nKey>,
    /// The page's own scroll, and an anchor on it that the first matching row
    /// claims. Searching tells you "Appearance (2)"; these are what carry you
    /// to the two, which on a long page start well below the fold.
    pub content_scroll: gpui::ScrollHandle,
    pub search_anchor: gpui::ScrollAnchor,
    /// Set when the query or the section changes, and spent by the next render
    /// that has somewhere to go. A `Cell` because that render only holds `&self`.
    pub reveal_first_hit: Cell<bool>,
    /// The families the font menus list, read once when the page opens.
    pub font_names: std::rc::Rc<Vec<String>>,
    pub shell_program_input: Entity<InputState>,
    pub shell_args_input: Entity<InputState>,
    pub wd_path_input: Entity<InputState>,
    pub link_file_command_input: Entity<InputState>,
    pub http_proxy_input: Entity<InputState>,
    pub theme_editor: Option<ThemeEditor>,
    pub theme_draft: Option<(presets::Theme, presets::Theme)>,
    pub theme_draft_error: Option<String>,
    /// The one popover open on the page — a dropdown, a theme list, a row's
    /// menu — by the id its trigger was drawn with.
    pub menu: Option<SharedString>,
    /// The query box inside a popover that has one, and the row the arrow
    /// keys have reached in it.
    pub menu_query: Entity<InputState>,
    pub menu_hi: usize,
    pub menu_scroll: gpui::ScrollHandle,
    /// Whatever held keyboard focus when the page was last drawn, so a text
    /// field can draw its focus ring from a builder that has no `Window`.
    pub focused: RefCell<Option<gpui::FocusHandle>>,
    /// A recorded shortcut that another action already answers to, held until
    /// the user says whether to take it over.
    pub kb_conflict: Option<KeyConflict>,
    pub recording: Option<Recording>,
    pub rebinding_note: Option<String>,
    pub ssh_form: Option<SshProfileForm>,
    /// The host whose row is expanded to show its details.
    pub ssh_open: Option<Uuid>,
    /// Every host, grouped by where it is defined, rather than the recent few.
    pub ssh_show_all: bool,
    /// Remove was pressed once on the host being edited; the second press
    /// removes it.
    pub ssh_confirm_remove: bool,
    /// The host whose ssh command was just copied, for the moment the button
    /// says so.
    pub ssh_copied: Option<Uuid>,
    pub ssh_filter: Entity<InputState>,
    pub ssh_collapsed_groups: std::collections::HashSet<String>,
    pub ssh_list_scroll: gpui::ScrollHandle,
    pub agent_hooks_host: HostId,
    pub agent_hooks_states: AgentHooksView,
    pub agent_hooks_seq: u64,
    pub agent_hooks_note: Option<(anterm_core::agent_hooks::HookAgent, String)>,
    pub agent_query: Entity<InputState>,
    pub agent_list_scroll: gpui::ScrollHandle,
    /// Every agent, not just the ones installed on the selected machine.
    pub agent_show_all: bool,
    /// Agents acted on since the list was last narrowed, kept in view so a row
    /// does not vanish under the pointer the moment it is uninstalled.
    pub agent_touched: std::collections::HashSet<anterm_core::agent_hooks::HookAgent>,
    /// Agents with an install or removal in flight.
    pub agent_busy: std::collections::HashSet<anterm_core::agent_hooks::HookAgent>,
    pub _subs: Vec<Subscription>,
}

/// A recorded chord and the action it would be taken from.
pub struct KeyConflict {
    pub action: String,
    pub spec: String,
    pub other: String,
}

#[derive(Clone)]
pub enum AgentHooksView {
    Loading,
    Ready(Vec<AgentHookRow>),
    Unavailable(String),
}

#[derive(Clone)]
pub struct AgentHookRow {
    pub agent: anterm_core::agent_hooks::HookAgent,
    pub state: anterm_core::agent_hooks::HooksState,
    pub target: String,
}

#[derive(Clone)]
pub struct AgentHooksMachine {
    pub host: HostId,
    pub label: String,
}

/// The Appearance page's three ways of choosing a theme.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

fn ssh_group_key(p: &SshProfile) -> &str {
    p.group.as_deref().unwrap_or("")
}

fn ssh_group_label(key: &str) -> &str {
    match key {
        anterm_core::ssh_config::IMPORTED_GROUP => "~/.ssh/config",
        "" => t(L10nKey::SettingsInAnterm),
        other => other,
    }
}

fn ssh_group_rank(key: &str) -> u8 {
    match key {
        anterm_core::ssh_config::IMPORTED_GROUP => 0,
        "" => 2,
        _ => 1,
    }
}

fn ssh_row_matches(p: &SshProfile, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let hit = |s: &str| s.to_lowercase().contains(query);
    hit(&p.name) || hit(&p.host) || hit(&p.user) || hit(&p.port.to_string())
}

/// How many *other* profiles reach the same `user@host:port` as this one.
///
/// The keychain is keyed by the endpoint, not by the profile, so two hosts that
/// differ only in how they get there — one direct, one through a jump host —
/// hand the same saved password back and forth. Every path that is about to
/// remove that password has to know this first: deleting a profile keeps the
/// secret while someone else still needs it, and forgetting one says out loud
/// who else it takes down. Both used to work the answer out on their own, which
/// is exactly how the two policies would have drifted apart.
fn profiles_sharing_endpoint(cfg: &Config, id: Uuid) -> usize {
    let Some(profile) = cfg.ssh_profiles.iter().find(|p| p.id == id) else {
        return 0;
    };
    cfg.ssh_profiles
        .iter()
        .filter(|p| {
            p.id != id && (&p.user, &p.host, p.port) == (&profile.user, &profile.host, profile.port)
        })
        .count()
}

pub struct Recording {
    pub action: String,
    pub chords: Vec<String>,
    pub _intercept: Subscription,
}

pub fn font_default_label() -> &'static str {
    t(L10nKey::SettingsFontDefault)
}

/// The same first row for the interface face, spelled for what it actually
/// does. The bold and italic dropdowns fall back to the *terminal's* primary
/// family, which is what their label promises; the interface falls back to the
/// system UI font instead, so it cannot borrow that label without telling the
/// reader the chrome will come out in Hack.
pub fn ui_font_default_label() -> &'static str {
    t(L10nKey::SettingsUiFontDefault)
}

#[cfg(target_os = "macos")]
const LINK_MODIFIER_LABEL: &str = "⌘";
#[cfg(not(target_os = "macos"))]
const LINK_MODIFIER_LABEL: &str = "Ctrl";

#[cfg(test)]
pub use anterm_ui_kit::actions::humanize_action;

impl SettingsWindow {
    pub fn with_settings_edits_resolved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let save_failed = self
            .active_settings()
            .is_some_and(|s| s.save_error.is_some());
        if !save_failed && !self.ssh_form_dirty(cx) && !self.theme_draft_dirty() {
            action(self, window, cx);
            return;
        }
        let answer = window.prompt::<gpui::PromptButton>(
            gpui::PromptLevel::Warning,
            t(L10nKey::SettingsUnsavedTitle),
            Some(t(L10nKey::SettingsUnsavedBody)),
            &[
                gpui::PromptButton::ok(t(L10nKey::SettingsSaveChanges)),
                gpui::PromptButton::new(t(L10nKey::EditorDiscard)),
                gpui::PromptButton::cancel(t(L10nKey::SettingsKeepEditing)),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match choice {
                    0 => {
                        if this
                            .active_settings()
                            .is_some_and(|s| s.save_error.is_some())
                        {
                            this.persist_settings_config(cx);
                            if this
                                .active_settings()
                                .is_some_and(|s| s.save_error.is_some())
                            {
                                return;
                            }
                        }
                        if this.ssh_form_dirty(cx)
                            && this.save_editing_profile(window, cx).is_none()
                        {
                            return;
                        }
                        if !this.save_theme_draft(window, cx) {
                            return;
                        }
                    }
                    1 => {
                        this.discard_unsaved_settings(window, cx);
                        this.cancel_theme_draft(window, cx);
                        if let Some(s) = this.active_settings_mut() {
                            s.ssh_form = None;
                        }
                    }
                    _ => return,
                }
                action(this, window, cx);
            });
        })
        .detach();
    }

    pub fn navigate_settings(
        &mut self,
        target: SettingsSection,
        setting: Option<L10nKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ssh_form_dirty(cx)
            || self.theme_draft_dirty()
            || self
                .active_settings()
                .is_some_and(|s| s.save_error.is_some())
        {
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.navigate_settings(target, setting, window, cx)
            });
            return;
        }
        if let Some(state) = self.active_settings() {
            state
                .search
                .clone()
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        let setting = match setting {
            Some(L10nKey::SettingsCustomPath)
                if cx.global::<Config>().working_directory.strategy
                    != anterm_ui_kit::config::WdStrategy::Custom =>
            {
                Some(L10nKey::SettingsStartIn)
            }
            Some(L10nKey::SettingsOpenFilesCommand)
                if cx.global::<Config>().file_open_mode() != LinkFileOpen::Command =>
            {
                Some(L10nKey::OpenFilesWith)
            }
            other => other,
        };
        if let Some(state) = self.active_settings_mut() {
            state.search_active = false;
            state.focused_setting = setting;
            state.reveal_first_hit.set(setting.is_some());
            state.content_scroll.set_offset(gpui::point(px(0.), px(0.)));
            state.menu = None;
        }
        self.select_settings_section(target, cx);
    }

    /// Whether the row measured this render came out narrower than a threshold
    /// quoted at the default interface font — the only way those px thresholds
    /// mean anything to a reader who scaled the interface up.
    fn settings_row_under(&self, at_default_font: f32, cx: &App) -> bool {
        self.settings_row_width.get() < at_default_font * ui_scale(cx)
    }

    /// The scroll anchor for the first thing on the page the query matched,
    /// whatever kind of element that is. A section header can be the only
    /// match on its page — "ansi", "how shells work" — and while the dimming
    /// around it already picks it out, nothing was carrying the page to it:
    /// search "ansi" from the bottom of Appearance and every row greys out
    /// with the one answer left above the fold.
    fn first_hit_anchor(&self, label: &str, cx: &Context<Self>) -> Option<gpui::ScrollAnchor> {
        let s = self.active_settings()?;
        if s.search_active || s.search_rows.borrow().is_some() {
            return None;
        }
        if let Some(target) = s.focused_setting {
            if t(target) == label && !self.settings_hit_anchored.replace(true) {
                return Some(s.search_anchor.clone());
            }
            return None;
        }
        let query = s.search.read(cx).value().trim().to_lowercase();
        if query.is_empty() || section_match_count(s.section, &query) == 0 {
            return None;
        }
        if !row_matches_query(s.section, label, &query) {
            return None;
        }
        match self.settings_hit_anchored.replace(true) {
            false => Some(s.search_anchor.clone()),
            true => None,
        }
    }

    fn toggle_ssh_group(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            if !s.ssh_collapsed_groups.remove(&key) {
                s.ssh_collapsed_groups.insert(key);
            }
        }
        // Collapsing the list never discards the profile being edited.
        cx.notify();
    }

    fn ssh_form_mut(&mut self) -> Option<&mut SshProfileForm> {
        self.active_settings_mut().and_then(|s| s.ssh_form.as_mut())
    }

    pub fn ssh_form_load(
        &mut self,
        profile: &SshProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ssh_form_dirty(cx) {
            let profile = profile.clone();
            self.with_settings_edits_resolved(window, cx, move |this, window, cx| {
                this.ssh_form_load(&profile, window, cx)
            });
            return;
        }
        let jump_name = profile
            .jump_host
            .and_then(|id| {
                cx.global::<Config>()
                    .ssh_profiles
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_default();

        let name = seed_hinted(window, cx, &profile.name, t(L10nKey::SettingsNameHint));
        let host = seed_hinted(window, cx, &profile.host, t(L10nKey::SettingsHostHint));
        let port = seed_input(window, cx, &profile.port.to_string(), false);
        let user = seed_hinted(window, cx, &profile.user, t(L10nKey::SettingsUserHint));
        let jump = seed_input(window, cx, &jump_name, false);
        let forwards: Vec<ForwardRuleForm> = profile
            .forwards
            .iter()
            .map(|r| seed_forward_row(window, cx, r))
            .collect();
        // One line, comma-separated: the form is a row of single-line fields,
        // and a profile with more than one key is the exception.
        let identity_files = seed_hinted(
            window,
            cx,
            &profile.identity_files.join(", "),
            DEFAULT_KEY_HINT,
        );

        // One keychain read per host opened, not one per keystroke: the
        // password box shows what is actually stored, the way every other SSH
        // client shows it, so it can be read back, corrected or cleared
        // without connecting first.
        let loaded_password = stored_password(profile);
        let loaded_key = first_readable_key(profile);
        let loaded_passphrase = loaded_key
            .as_deref()
            .map(stored_passphrase)
            .unwrap_or_default();
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t(L10nKey::SettingsPasswordHint))
                .default_value(loaded_password.clone())
        });
        let passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t(L10nKey::SettingsPasswordHint))
                .default_value(loaded_passphrase.clone())
        });
        let proxy_command = seed_input(
            window,
            cx,
            profile.proxy_command.as_deref().unwrap_or(""),
            false,
        );
        let socks = seed_input(window, cx, &host_port_text(&profile.socks_proxy), false);
        let http = seed_input(window, cx, &host_port_text(&profile.http_proxy), false);
        let kex = seed_input(window, cx, &profile.algorithms.kex.join(", "), false);
        let cipher = seed_input(window, cx, &profile.algorithms.cipher.join(", "), false);
        let mac = seed_input(window, cx, &profile.algorithms.mac.join(", "), false);
        let hostkey = seed_input(window, cx, &profile.algorithms.hostkey.join(", "), false);
        let compression = seed_input(
            window,
            cx,
            &profile.algorithms.compression.join(", "),
            false,
        );
        let keepalive_interval = seed_input(
            window,
            cx,
            &profile
                .keepalive_interval_s
                .map(|n| n.to_string())
                .unwrap_or_default(),
            false,
        );
        let keepalive_count = seed_input(
            window,
            cx,
            &profile
                .keepalive_count_max
                .map(|n| n.to_string())
                .unwrap_or_default(),
            false,
        );
        let connect_timeout = seed_input(
            window,
            cx,
            &profile
                .connect_timeout_s
                .map(|n| n.to_string())
                .unwrap_or_default(),
            false,
        );
        let login_scripts = seed_textarea(window, cx, &profile.login_scripts.join("\n"));

        let mut subs = Vec::new();
        // The passphrase belongs to whichever key the field above names, so
        // when that answer changes the box has to change with it. Without this
        // a form opened on one key and pointed at another would carry the
        // first key's passphrase across and save it over the second's. Host and
        // user count too: they fill in a `%h` / `%r` in the key's path.
        for input in [&identity_files, &host, &user] {
            subs.push(
                cx.subscribe_in(input, window, |this, _i, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        this.resync_key_passphrase(window, cx);
                    }
                }),
            );
        }
        let mut watch = vec![
            &name,
            &host,
            &port,
            &user,
            &jump,
            &password,
            &passphrase,
            &identity_files,
            &proxy_command,
            &socks,
            &http,
            &kex,
            &cipher,
            &mac,
            &hostkey,
            &compression,
            &keepalive_interval,
            &keepalive_count,
            &connect_timeout,
        ];
        for row in &forwards {
            watch.extend(forward_row_inputs(row));
        }
        for input in watch {
            subs.push(
                cx.subscribe_in(input, window, |this, _i, ev: &InputEvent, _w, cx| {
                    if matches!(ev, InputEvent::Change) {
                        let _ = this.ssh_form_mut();
                        cx.notify();
                    }
                }),
            );
        }
        subs.push(cx.subscribe_in(
            &login_scripts,
            window,
            |this, _i, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    let _ = this.ssh_form_mut();
                    cx.notify();
                }
            },
        ));

        let form = SshProfileForm {
            editing: profile.id,
            carry_group: profile.group.clone(),
            carry_credential_ref: profile.credential_ref.clone(),
            name,
            host,
            port,
            user,
            auth: profile.auth,
            password,
            passphrase,
            loaded_password,
            loaded_passphrase,
            loaded_endpoint: (profile.user.clone(), profile.host.clone(), profile.port),
            loaded_key,
            jump,
            forwards,
            identity_files,
            proxy_command,
            socks,
            http,
            kex,
            cipher,
            mac,
            hostkey,
            compression,
            keepalive_interval,
            keepalive_count,
            connect_timeout,
            login_scripts,
            agent_forward: profile.agent_forward,
            x11: profile.x11,
            skip_banner: profile.skip_banner,
            shell_integration: profile.shell_integration,
            remote_clipboard_write: profile.remote_clipboard_write,
            verify_host_keys: profile.verify_host_keys,
            warn_on_close: profile.warn_on_close,
            _subs: subs,
        };
        let editing = form.editing;
        let known = cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .any(|p| p.id == editing);
        if let Some(s) = self.active_settings_mut() {
            s.ssh_form = Some(form);
            s.ssh_confirm_remove = false;
            // A saved host edits in its own row; a new one gets a row of its
            // own at the top, so nothing else is shown opened.
            s.ssh_open = known.then_some(editing);
        }
        cx.notify();
    }

    /// Reads the form out of its entities and runs it past
    /// [`validate_ssh_draft`]. The profile that comes back is what the form
    /// would save; the errors are what stands in the way.
    fn ssh_form_collect(&self, cx: &App) -> Option<(SshProfile, SshFormErrors)> {
        let form = self.active_settings()?.ssh_form.as_ref()?;
        let val = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        // The multi-line and comma-separated fields do their own splitting, so
        // they travel whole rather than trimmed.
        let raw = |e: &Entity<InputState>| e.read(cx).value().to_string();

        let draft = SshFormDraft {
            id: form.editing,
            name: val(&form.name),
            group: form.carry_group.clone(),
            host: val(&form.host),
            port: val(&form.port),
            user: val(&form.user),
            jump: val(&form.jump),
            proxy_command: val(&form.proxy_command),
            socks: val(&form.socks),
            http: val(&form.http),
            auth: form.auth,
            identity_files: raw(&form.identity_files).replace(',', "\n"),
            agent_forward: form.agent_forward,
            credential_ref: form.carry_credential_ref.clone(),
            forwards: form.forwards.iter().filter_map(|r| r.collect(cx)).collect(),
            keepalive_interval: val(&form.keepalive_interval),
            keepalive_count: val(&form.keepalive_count),
            connect_timeout: val(&form.connect_timeout),
            warn_on_close: form.warn_on_close,
            skip_banner: form.skip_banner,
            shell_integration: form.shell_integration,
            remote_clipboard_write: form.remote_clipboard_write,
            login_scripts: form.login_scripts.read(cx).value().to_string(),
            x11: form.x11,
            kex: raw(&form.kex),
            cipher: raw(&form.cipher),
            mac: raw(&form.mac),
            hostkey: raw(&form.hostkey),
            compression: raw(&form.compression),
            verify_host_keys: form.verify_host_keys,
        };
        Some(validate_ssh_draft(
            draft,
            &cx.global::<Config>().ssh_profiles,
        ))
    }

    /// Point the passphrase box at the key the form now names.
    ///
    /// A passphrase is stored against the contents of the key it unlocks, so
    /// the box is only ever right about one key at a time. A box the user has
    /// started typing in is left alone — it is the one place where what is on
    /// screen outranks what the keychain holds.
    fn resync_key_passphrase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.active_settings().and_then(|s| s.ssh_form.as_ref()) else {
            return;
        };
        let files = split_lines(&form.identity_files.read(cx).value().replace(',', "\n"));
        let host = form.host.read(cx).value().trim().to_string();
        let user = form.user.read(cx).value().trim().to_string();
        let key = first_readable_key_in(&files, &host, &user);
        if key == form.loaded_key {
            return;
        }
        let stored = key.as_deref().map(stored_passphrase).unwrap_or_default();
        // A box the user has already typed in keeps what they typed — only
        // the key it will be saved against moves under it.
        let untouched = form.passphrase.read(cx).value().as_ref() == form.loaded_passphrase;
        let input = form.passphrase.clone();
        if let Some(form) = self.ssh_form_mut() {
            form.loaded_key = key;
            form.loaded_passphrase = stored.clone();
        }
        if untouched {
            input.update(cx, |i, cx| i.set_value(stored, window, cx));
        }
        cx.notify();
    }

    pub fn save_editing_profile(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Uuid> {
        let (profile, errors) = self.ssh_form_collect(cx)?;
        // Save and Connect are both disabled while anything is wrong, but this
        // is the door all of them go through, and what gets past it lands in
        // the config file — where a host-less profile is a blank row nobody
        // can identify or delete on sight.
        if !errors.is_empty() {
            return None;
        }
        let id = profile.id;
        self.update_config(cx, |cfg| {
            if let Some(slot) = cfg.ssh_profiles.iter_mut().find(|p| p.id == id) {
                *slot = profile.clone();
            } else {
                cfg.ssh_profiles.push(profile.clone());
            }
        });
        if self
            .active_settings()
            .is_some_and(|s| s.save_error.is_some())
        {
            return None;
        }
        self.save_ssh_form_secrets(&profile, window, cx);
        Some(id)
    }

    pub fn cancel_ssh_form(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.active_settings_mut() {
            s.ssh_form = None;
            s.ssh_confirm_remove = false;
        }
        cx.notify();
    }

    /// Move the two secrets in the form into the keychain — or out of it.
    ///
    /// The config file never holds either of them, so this is the whole of
    /// what saving means for a password: an entry keyed by the endpoint the
    /// profile now names. Editing the address moves the entry rather than
    /// leaving the old one behind to be offered to a host that no longer
    /// exists, and clearing the field removes it.
    fn save_ssh_form_secrets(
        &mut self,
        profile: &SshProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.active_settings().and_then(|s| s.ssh_form.as_ref()) else {
            return;
        };
        let typed = form.password.read(cx).value().to_string();
        let typed_passphrase = form.passphrase.read(cx).value().to_string();
        let (old_user, old_host, old_port) = form.loaded_endpoint.clone();
        let was = form.loaded_password.clone();
        let was_passphrase = form.loaded_passphrase.clone();
        let moved = (old_user.as_str(), old_host.as_str(), old_port)
            != (profile.user.as_str(), profile.host.as_str(), profile.port);

        let mut failures: Vec<String> = Vec::new();
        let endpoint = |u: &str, h: &str, p: u16| format!("{u}@{h}:{p}");
        let plan = password_plan(&was, &typed, moved);

        // The entry the form read from, once it is no longer the entry the
        // form would write to. Left alone when another profile still dials the
        // same address — the keychain accounts by endpoint, not by profile.
        let stranded = plan.drop_old && !old_host.trim().is_empty();
        if stranded
            && !self.endpoint_still_in_use(&old_user, &old_host, old_port, profile.id, cx)
            && let Err(e) = OsCredentialStore.delete_password(&old_user, &old_host, old_port)
        {
            failures.push(t_fmt(
                L10nKey::SettingsCouldntForgetPassword,
                &[
                    ("endpoint", &endpoint(&old_user, &old_host, old_port)),
                    ("error", &e.to_string()),
                ],
            ));
        }
        if plan.store
            && !profile.host.trim().is_empty()
            && let Err(e) =
                OsCredentialStore.set_password(&profile.user, &profile.host, profile.port, &typed)
        {
            failures.push(t_fmt(
                L10nKey::SettingsCouldntSavePassword,
                &[
                    (
                        "endpoint",
                        &endpoint(&profile.user, &profile.host, profile.port),
                    ),
                    ("error", &e.to_string()),
                ],
            ));
        }

        // A passphrase belongs to the key it unlocks, not to this profile, so
        // a save only ever touches the entry for the key named here. Pointing
        // the profile at a different key leaves the first key's passphrase
        // alone — other hosts use that key too.
        let key = first_readable_key(profile);
        if typed_passphrase != was_passphrase {
            match key.as_deref() {
                Some(path) => self.write_key_passphrase(path, &typed_passphrase, &mut failures),
                // Nowhere to put it: the field names no key, or names one that
                // is not on this machine. Storing nothing quietly would lose a
                // passphrase the user watched themselves type.
                None if !typed_passphrase.is_empty() => {
                    failures.push(t(L10nKey::SettingsPassphraseNeedsKey).to_string())
                }
                None => {}
            }
        }

        for line in failures {
            window.push_notification(line, cx);
        }

        // What the form would now read back, so a save leaves it clean.
        if let Some(form) = self.ssh_form_mut() {
            form.loaded_password = typed;
            form.loaded_passphrase = typed_passphrase;
            form.loaded_endpoint = (profile.user.clone(), profile.host.clone(), profile.port);
            form.loaded_key = key;
        }
    }

    /// A blank secret deletes rather than stores: an empty string is not a
    /// passphrase, and leaving one behind would keep offering it.
    fn write_key_passphrase(&self, key_path: &str, secret: &str, failures: &mut Vec<String>) {
        let path = anterm_core::ssh_profile::expand_tilde(key_path);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                failures.push(t_fmt(
                    L10nKey::SettingsCouldntSavePassphrase,
                    &[("key", key_path), ("error", &e.to_string())],
                ));
                return;
            }
        };
        let account = key_account_from_contents(&bytes);
        let result = if secret.is_empty() {
            OsCredentialStore.delete_key_passphrase(&account)
        } else {
            OsCredentialStore
                .set_key_passphrase(&account, secret)
                .map(|_| ())
        };
        if let Err(e) = result {
            failures.push(t_fmt(
                L10nKey::SettingsCouldntSavePassphrase,
                &[("key", key_path), ("error", &e.to_string())],
            ));
        }
    }

    /// Whether some other saved host still dials this endpoint. The keychain
    /// entry is the address's, not the profile's — the same reason "Forget
    /// password" counts the hosts it would sign out.
    fn endpoint_still_in_use(
        &self,
        user: &str,
        host: &str,
        port: u16,
        except: Uuid,
        cx: &App,
    ) -> bool {
        cx.global::<Config>()
            .ssh_profiles
            .iter()
            .any(|p| p.id != except && p.user == user && p.host == host && p.port == port)
    }

    /// Whether the SSH profile form on screen holds edits that were never
    /// saved. Save is enabled off exactly this, so closing on it is the same
    /// question the button already answers — and it compares what the form
    /// would save even when the form cannot be saved yet, so a half-typed new
    /// host is still something Escape has to ask about.
    pub fn ssh_form_dirty(&self, cx: &App) -> bool {
        let Some(form) = self.active_settings().and_then(|s| s.ssh_form.as_ref()) else {
            return false;
        };
        let saved = cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .find(|p| p.id == form.editing)
            .cloned();
        self.ssh_form_collect(cx).map(|(profile, _)| profile) != saved || form.secrets_changed(cx)
    }

    /// Closing from Escape or the X is the user leaving; every other caller
    /// closes as the tail of something they explicitly chose, and has already
    /// saved or does not care.
    pub fn close_settings_checked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.with_settings_edits_resolved(window, cx, |this, window, cx| {
            this.close_settings(window, cx)
        });
    }

    pub fn add_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = SshProfile::new(String::new());
        self.ssh_form_load(&profile, window, cx);
        // A blank form is there to be typed into: land on its first field
        // rather than leave the keystrokes with nothing to go to. (A form held
        // back behind an unsaved-edits prompt has not opened yet, and is not
        // this profile's.)
        let first = self
            .ssh_form_mut()
            .filter(|form| form.editing == profile.id)
            .map(|form| form.name.clone());
        if let Some(first) = first {
            first.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn delete_profile_confirmed(&mut self, id: Uuid, cx: &mut Context<Self>) {
        // "Forget password" lives on the menu that is about to stop existing,
        // so deleting the profile used to strand its keychain entry with no UI
        // left to remove it. Only let go of the secret when nothing else on the
        // list still points at the same endpoint.
        let cfg = cx.global::<Config>();
        let endpoint = cfg
            .ssh_profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| (p.user.clone(), p.host.clone(), p.port));
        let shared = profiles_sharing_endpoint(cfg, id) > 0;
        if let Some((user, host, port)) = endpoint.filter(|_| !shared) {
            use anterm_ui_kit::keychain::{CredentialStore, OsCredentialStore};
            let _ = OsCredentialStore.delete_password(&user, &host, port);
        }
        // The same argument for the key passphrases this profile taught the
        // app about: the comment above says "the secret", but until now only
        // the password was let go of, so a deleted profile stranded its
        // passphrase entries with no UI left to reach them. A key is only
        // forgotten when no surviving profile still lists it.
        use anterm_ui_kit::keychain::{CredentialStore as _, OsCredentialStore};
        let mine = cfg
            .ssh_profiles
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.expanded_identity_files())
            .unwrap_or_default();
        let kept: std::collections::HashSet<String> = cfg
            .ssh_profiles
            .iter()
            .filter(|p| p.id != id)
            .flat_map(|p| p.expanded_identity_files())
            .collect();
        for path in mine.iter().filter(|p| !kept.contains(*p)) {
            // Keyed by the key file's contents, so a key already gone from
            // disk cannot be looked up — and has no live passphrase to leak.
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let account = anterm_ui_kit::keychain::key_account_from_contents(&bytes);
            let _ = OsCredentialStore.delete_key_passphrase(&account);
        }

        self.update_config(cx, |cfg| {
            cfg.ssh_profiles.retain(|p| p.id != id);
            cfg.ssh_profile_frecency.remove(&id);
        });
        if let Some(s) = self.active_settings_mut() {
            if s.ssh_form.as_ref().is_some_and(|f| f.editing == id) {
                s.ssh_form = None;
            }
            if s.ssh_open == Some(id) {
                s.ssh_open = None;
            }
            s.ssh_confirm_remove = false;
        }
        cx.notify();
    }

    /// Import `~/.ssh/config`, and say what that did.
    ///
    /// Every branch here ends in a notification because every branch used to
    /// end in nothing: a missing file, a file of nothing but `Host *`, and a
    /// clean import of six hosts were all the same silent button press, and the
    /// only way to tell them apart was to go count the host list.
    pub fn import_ssh_config_profiles(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // One id for all three outcomes, so pressing the button again replaces
        // what it said last time instead of stacking a second toast on top of
        // an answer that is now out of date.
        const NOTIFICATION: &str = "ssh-config-import";
        // A toast is 448pt wide. A config with a dozen unsupported keywords in
        // it would push the counts out of view, so the notification names the
        // first few and the log line below carries the whole list, with the
        // hosts each keyword was set on.
        const OPTIONS_SHOWN: usize = 5;

        let report = anterm_core::ssh_config::import_report();
        let source = report.source.display().to_string();

        if !report.source_read {
            window.push_notification(
                Notification::error(t_fmt(
                    L10nKey::SettingsImportUnreadable,
                    &[("path", &source)],
                ))
                .id1::<Self>(NOTIFICATION),
                cx,
            );
            return;
        }
        if report.profiles.is_empty() {
            window.push_notification(
                Notification::warning(t_fmt(L10nKey::SettingsImportNoHosts, &[("path", &source)]))
                    .id1::<Self>(NOTIFICATION),
                cx,
            );
            return;
        }

        let read = report.profiles.len();
        let ignored = report.ignored;
        let mut stats = anterm_core::ssh_config::MergeStats::default();
        self.update_config(cx, |cfg| {
            stats = anterm_core::ssh_config::merge_imported(&mut cfg.ssh_profiles, report.profiles);
        });

        let dropped: Vec<String> = ignored
            .iter()
            .map(|opt| format!("{} ({})", opt.option, opt.hosts.join(", ")))
            .collect();
        log::info!(
            "imported {read} alias(es) from {source} ({} file(s) read): {} added, {} updated, \
             {} unchanged; no anterm setting for: [{}]",
            report.files_read,
            stats.added,
            stats.updated,
            stats.unchanged,
            dropped.join("; ")
        );

        let mut notification = Notification::new()
            .with_type(NotificationType::Success)
            .title(t_plural(
                L10nKey::SettingsImportSummary,
                stats.added,
                &[
                    ("updated", &stats.updated.to_string()),
                    ("unchanged", &stats.unchanged.to_string()),
                ],
            ))
            .id1::<Self>(NOTIFICATION);
        if !ignored.is_empty() {
            let mut options: Vec<String> = ignored
                .iter()
                .take(OPTIONS_SHOWN)
                .map(|opt| opt.option.clone())
                .collect();
            let rest = ignored.len() - options.len();
            if rest > 0 {
                options.push(t_fmt(
                    L10nKey::SettingsImportMoreOptions,
                    &[("count", &rest.to_string())],
                ));
            }
            notification = notification
                .message(t_plural(
                    L10nKey::SettingsImportIgnored,
                    ignored.len(),
                    &[("options", &options.join(", "))],
                ))
                // A list of what the import could not carry is something to
                // read and act on, and four seconds is not long enough to do
                // either. The counts alone still fade on their own.
                .autohide(false);
        }
        window.push_notification(notification, cx);
    }
}

