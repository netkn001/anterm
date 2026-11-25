//! Cross-panel semantic intents, settings navigation sections, theme editing
//! slots, shell argument quoting, and shared formatting/time/key-hint helpers.

use std::path::PathBuf;

use anterm_core::config::{RightPanelTab, WindowBackdrop};
use anterm_core::machine::TabId;
use anterm_i18n::{L10nKey, t, t_fmt, t_plural};
use gpui::{App, Keystroke};
use gpui_kit::component::kbd::Kbd;

use crate::config::Config;
use crate::host_ops::HostId;

pub const FONT_SIZE_STEP: f32 = 1.0;
pub const UI_FONT_SIZE_STEP: f32 = 1.0;
pub const LINE_HEIGHT_STEP: f32 = 0.05;
pub const RECORD_COMMIT_DELAY_MS: u64 = 650;

/// Narrow read-only snapshot of the active workspace tab passed down to
/// detached panels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveWorkspaceContext {
    pub tab_id: Option<TabId>,
    pub host_id: HostId,
    pub cwd: Option<PathBuf>,
}

/// Semantic requests emitted by detached feature panels and fulfilled by the
/// hosting workspace window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceIntent {
    /// Open a file in the code editor (optionally jumping to 1-based line/column)
    /// and reveal it in the file tree.
    OpenFile {
        path: PathBuf,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Reveal a directory in the file tree.
    RevealDirectory(PathBuf),
    /// Return keyboard focus to the active terminal pane.
    FocusActiveTerminal,
    /// Open the SSH profile editor for `profile_id` (falling back to the SSH
    /// section if the profile no longer exists).
    OpenSshProfile(uuid::Uuid),
    /// Restart the focused pane's SSH session if it is disconnected.
    RestartActiveSshSession,
    /// Show the right detail panel on `tab`.
    ShowRightPanel(RightPanelTab),
    /// Hide the right detail panel.
    HideRightPanel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Appearance,
    Terminal,
    KeyboardMouse,
    Ssh,
    Agents,
    Keybindings,
    About,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 7] = [
        SettingsSection::General,
        SettingsSection::Appearance,
        SettingsSection::Terminal,
        SettingsSection::KeyboardMouse,
        SettingsSection::Ssh,
        SettingsSection::Agents,
        SettingsSection::About,
    ];

    pub fn navigation_section(self) -> Self {
        match self {
            Self::Keybindings => Self::KeyboardMouse,
            other => other,
        }
    }

    pub fn title(self) -> L10nKey {
        match self {
            Self::General => L10nKey::SettingsNavGeneral,
            Self::Appearance => L10nKey::SettingsNavAppearance,
            Self::Terminal => L10nKey::SettingsNavTerminal,
            Self::KeyboardMouse => L10nKey::SettingsNavInput,
            Self::Ssh => L10nKey::SettingsNavSsh,
            Self::Agents => L10nKey::SettingsNavAgents,
            Self::Keybindings => L10nKey::SettingsNavKeybindings,
            Self::About => L10nKey::SettingsNavAbout,
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            Self::General => "icons/settings/general.svg",
            Self::Appearance => "icons/settings/appearance.svg",
            Self::Terminal => "icons/settings/terminal.svg",
            Self::KeyboardMouse | Self::Keybindings => "icons/settings/keyboard.svg",
            Self::Ssh => "icons/settings/ssh.svg",
            Self::Agents => "icons/settings/integrations.svg",
            Self::About => "icons/settings/about.svg",
        }
    }

    pub fn profile_label(self) -> &'static str {
        match self {
            SettingsSection::General => "settings:general",
            SettingsSection::Appearance => "settings:appearance",
            SettingsSection::Terminal => "settings:terminal",
            SettingsSection::KeyboardMouse => "settings:keyboard-mouse",
            SettingsSection::Ssh => "settings:ssh",
            SettingsSection::Agents => "settings:agents",
            SettingsSection::Keybindings => "settings:keybindings",
            SettingsSection::About => "settings:about",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeEdit {
    Background,
    Foreground,
    Accent,
    Cursor,
    Selection,
    Ansi(usize),
}

const ANSI_COLOR_LABELS: [L10nKey; 16] = [
    L10nKey::AppThemeAnsiBlack,
    L10nKey::AppThemeAnsiRed,
    L10nKey::AppThemeAnsiGreen,
    L10nKey::AppThemeAnsiYellow,
    L10nKey::AppThemeAnsiBlue,
    L10nKey::AppThemeAnsiMagenta,
    L10nKey::AppThemeAnsiCyan,
    L10nKey::AppThemeAnsiWhite,
    L10nKey::AppThemeAnsiBrightBlack,
    L10nKey::AppThemeAnsiBrightRed,
    L10nKey::AppThemeAnsiBrightGreen,
    L10nKey::AppThemeAnsiBrightYellow,
    L10nKey::AppThemeAnsiBrightBlue,
    L10nKey::AppThemeAnsiBrightMagenta,
    L10nKey::AppThemeAnsiBrightCyan,
    L10nKey::AppThemeAnsiBrightWhite,
];

pub fn theme_edit_label(edit: ThemeEdit) -> &'static str {
    t(match edit {
        ThemeEdit::Background => L10nKey::AppThemeColorBackground,
        ThemeEdit::Foreground => L10nKey::AppThemeColorForeground,
        ThemeEdit::Accent => L10nKey::AppThemeColorAccent,
        ThemeEdit::Cursor => L10nKey::AppThemeColorCursor,
        ThemeEdit::Selection => L10nKey::AppThemeColorSelection,
        ThemeEdit::Ansi(i) => ANSI_COLOR_LABELS[i.min(15)],
    })
}

pub fn hsla_to_u32(color: gpui::Hsla) -> u32 {
    let rgba: gpui::Rgba = color.into();
    let to = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u32;
    (to(rgba.r) << 16) | (to(rgba.g) << 8) | to(rgba.b)
}

#[cfg(target_os = "windows")]
pub fn window_backdrop_label_key(backdrop: WindowBackdrop) -> L10nKey {
    match backdrop {
        WindowBackdrop::Auto => L10nKey::SettingsBackdropAuto,
        WindowBackdrop::Blur => L10nKey::SettingsBackdropBlur,
        WindowBackdrop::Mica => L10nKey::SettingsBackdropMica,
        WindowBackdrop::MicaAlt => L10nKey::SettingsBackdropMicaAlt,
        WindowBackdrop::Acrylic => L10nKey::SettingsBackdropAcrylic,
        WindowBackdrop::Off => L10nKey::SettingsBackdropOff,
    }
}

pub fn clear_window_override_values(config: &mut Config, backdrop_is_local: bool) {
    config.window_opacity = None;
    config.window_blur = None;
    if backdrop_is_local {
        config.window_backdrop = WindowBackdrop::Auto;
    }
}

/// Returns `Err` when a quote never closes: that text names no argv at all.
pub fn split_shell_args(input: &str) -> Result<Vec<String>, ()> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => match chars.clone().next() {
                Some(next @ ('"' | '\\')) => {
                    chars.next();
                    current.push(next);
                }
                _ => current.push('\\'),
            },
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err(());
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

/// The exact inverse of [`split_shell_args`].
pub fn join_shell_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_shell_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_shell_arg(arg: &str) -> String {
    if !arg.is_empty()
        && !arg
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"')
    {
        return arg.to_string();
    }
    if !arg.contains('\'') {
        return format!("'{arg}'");
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    for c in arg.chars() {
        if c == '"' || c == '\\' {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// Empty means unset and saves; anything else must name a directory that exists.
pub fn wd_path_saveable(path: &str) -> bool {
    let path = path.trim();
    path.is_empty() || std::path::Path::new(path).is_dir()
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const DAY: u64 = 86_400;
const WEEK: u64 = 7 * DAY;
const MONTH: u64 = 31_536_000 / 12;
const YEAR: u64 = 365 * DAY;

pub fn relative_time(now: u64, then: u64) -> String {
    if then == 0 || then >= now {
        return t(L10nKey::HomeTimeJustNow).to_string();
    }
    let secs = now - then;
    match secs {
        s if s < 60 => t(L10nKey::HomeTimeJustNow).to_string(),
        s if s < 3_600 => t_plural(L10nKey::HomeTimeMinutesAgo, (s / 60) as usize, &[]),
        s if s < 7_200 => t(L10nKey::HomeTimeHourAgo).to_string(),
        s if s < DAY => t_plural(L10nKey::HomeTimeHoursAgo, (s / 3_600) as usize, &[]),
        s if s < 2 * DAY => t(L10nKey::HomeTimeYesterday).to_string(),
        s if s < WEEK => t_plural(L10nKey::HomeTimeDaysAgo, (s / DAY) as usize, &[]),
        s if s < MONTH => t_plural(L10nKey::HomeTimeWeeksAgo, (s / WEEK) as usize, &[]),
        s if s < YEAR => t_plural(L10nKey::HomeTimeMonthsAgo, (s / MONTH) as usize, &[]),
        _ => t(L10nKey::HomeTimeOverYearAgo).to_string(),
    }
}

pub fn compact_age(secs: u64) -> String {
    let unit = |key, n: u64| t_fmt(key, &[("count", &n.to_string())]);
    match secs {
        s if s < 60 => t(L10nKey::AgeUnderMinute).to_string(),
        s if s < 3_600 => unit(L10nKey::AgeMinutes, s / 60),
        s if s < DAY => unit(L10nKey::AgeHours, s / 3_600),
        s => unit(L10nKey::AgeDays, s / DAY),
    }
}

pub fn waiting_for(secs: u64) -> String {
    t_fmt(L10nKey::AgentWaitingFor, &[("age", &compact_age(secs))])
}

pub fn key_stroke(action: &str, cx: &App) -> Option<Keystroke> {
    let spec = crate::keymap::effective_key(action, cx)?;
    let first = spec.split_whitespace().next()?;
    Keystroke::parse_with_os(first, crate::platform::current_os()).ok()
}

pub fn key_hint(action: &str, cx: &App) -> Option<String> {
    Some(Kbd::format(&key_stroke(action, cx)?))
}
