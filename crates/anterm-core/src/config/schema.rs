use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub const SUPPORTED_GUI_LANGUAGES: &[&str] = &["en", "zh-CN"];

#[derive(Default, Clone, Eq, PartialEq, Hash)]
pub struct FontFeatures(pub Arc<Vec<(String, u32)>>);

impl FontFeatures {
    pub fn tag_value_list(&self) -> &[(String, u32)] {
        self.0.as_slice()
    }

    pub fn is_calt_enabled(&self) -> Option<bool> {
        self.0
            .iter()
            .find(|(feature, _)| feature == "calt")
            .map(|(_, value)| *value == 1)
    }
}

impl std::fmt::Debug for FontFeatures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("FontFeatures");
        for (tag, value) in self.tag_value_list() {
            debug.field(tag, value);
        }
        debug.finish()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum FeatureValue {
    Bool(bool),
    Number(serde_json::Number),
}

fn is_valid_feature_tag(tag: &str) -> bool {
    tag.len() == 4 && tag.chars().all(|c| c.is_ascii_alphanumeric())
}

impl<'de> serde::Deserialize<'de> for FontFeatures {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};

        struct FontFeaturesVisitor;

        impl<'de> Visitor<'de> for FontFeaturesVisitor {
            type Value = FontFeatures;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map of font features")
            }

            fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut feature_list = Vec::new();
                while let Some((key, value)) =
                    access.next_entry::<String, Option<FeatureValue>>()?
                {
                    if !is_valid_feature_tag(&key) {
                        log::error!("Incorrect font feature tag: {key}");
                        continue;
                    }
                    let Some(value) = value else { continue };
                    match value {
                        FeatureValue::Bool(enable) => {
                            feature_list.push((key, u32::from(enable)));
                        }
                        FeatureValue::Number(value) => match value.as_u64() {
                            Some(value) => feature_list.push((key, value as u32)),
                            None => {
                                log::error!(
                                    "Incorrect font feature value {value} for feature tag {key}"
                                );
                                continue;
                            }
                        },
                    }
                }
                Ok(FontFeatures(Arc::new(feature_list)))
            }
        }

        deserializer.deserialize_map(FontFeaturesVisitor)
    }
}

impl serde::Serialize for FontFeatures {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;
        for (tag, value) in self.tag_value_list() {
            map.serialize_entry(tag, value)?;
        }
        map.end()
    }
}

/// One action's line in `keybindings`.
///
/// The two shapes mean different things, so a save writes back whichever one
/// was read.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(untagged)]
pub enum KeybindingOverride {
    /// `"NextTab": "cmd-shift-]"` — a chord *beside* the ones the action
    /// already has, the way VS Code, Zed and kitty read a line like it (#868).
    /// Empty unbinds the action, which is what `""` has always meant here.
    Add(String),
    /// `"NextTab": ["cmd-shift-]"]` — exactly these chords, replacing the
    /// default and the preset's. `[]` unbinds. This is what the Settings page
    /// writes, because recording a shortcut there sets it.
    Exact(Vec<String>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub font_family: String,
    pub font_fallbacks: Vec<String>,
    pub font_family_bold: Option<String>,
    pub font_family_italic: Option<String>,
    pub font_features: Option<FontFeatures>,
    /// macOS only: let CoreGraphics font smoothing thicken glyph strokes — by
    /// more the lighter the text, which is why light-on-dark looks bolder.
    /// On by default, because it is what every macOS app draws with.
    ///
    /// Off tells gpui's rasterizer not to dilate, so anterm's glyphs are drawn
    /// at the face's own weight. `AppleFontSmoothing` is left alone, so menus
    /// and other natively drawn text keep the system's look (#1119). Applied
    /// at launch, so a change takes hold at the next one. Ignored elsewhere,
    /// where there is no such dilation to turn off.
    #[serde(default = "default_true")]
    pub font_thicken: bool,
    pub font_size: f32,
    pub line_height: f32,
    /// The interface's root font size, in pixels — everything outside the
    /// terminal grid is sized against it.
    ///
    /// It is the CSS-style `rem` the whole chrome is laid out in, not one
    /// label's size: raising it scales panel text, section headings and the
    /// spacing derived from them together. Its own default matches the size
    /// the chrome was drawn at, so an existing config renders unchanged.
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: f32,
    /// The face the chrome is drawn in; `None` is the system UI font.
    ///
    /// Only the chrome — the terminal grid keeps `font_family`. The
    /// struct-level `serde(default)` fills it in for a config written before
    /// the setting existed, and like every other optional key it is written
    /// out as `null` rather than dropped, so the file still lists what can be
    /// set by hand.
    pub ui_font_family: Option<String>,
    pub theme: String,
    pub theme_preset: String,
    pub theme_follow_system: bool,
    pub theme_preset_light: String,
    pub theme_preset_dark: String,
    #[serde(default = "default_true")]
    pub theme_legible_palette: bool,
    pub window_opacity: Option<f32>,
    pub window_blur: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub window_backdrop: WindowBackdrop,
    #[serde(default = "default_true")]
    pub dim_inactive_panes: bool,
    /// Paint the title bar's buttons — new tab and the two panel toggles —
    /// only while the pointer is over the bar they sit in. Off by default: a
    /// button that is not on screen is a button nobody finds, and the switch
    /// is for those who already know where it is and would rather rest their
    /// eyes on a bare bar. The struct-level `serde(default)` reads a file
    /// written before the key existed as off, which is how every window drew
    /// then.
    pub auto_hide_titlebar_buttons: bool,
    /// Lenient one entry at a time, for the same reason the nested keys below
    /// are: this is hand-edited, and it used to be all-or-nothing. A single
    /// value serde could not read — `"ActivateTab1": null`, a number, an object
    /// — failed the whole `Config`, which quarantines `config.json` and starts
    /// the app on built-in defaults; every rebinding in the file then read as
    /// its shipped default, and the next settings write persisted those
    /// defaults over what the user wrote (#901). A line that cannot be read is
    /// skipped with a warning, and the rest of the map still binds.
    #[serde(default, deserialize_with = "de_keybindings")]
    pub keybindings: HashMap<String, KeybindingOverride>,
    #[serde(default = "default_preset")]
    pub keybinding_preset: String,
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default, deserialize_with = "de_shell")]
    pub shell: Option<ShellConfig>,
    /// Lenient on purpose: this is a hand-edited key with a nested shape, and a
    /// typo in one entry must not fail the whole `Config` and hand the user
    /// back defaults that the next settings write would then persist over what
    /// they wrote.
    #[serde(default, deserialize_with = "de_lenient")]
    pub custom_shells: Vec<CustomShell>,

    pub link_url: bool,
    /// What a clicked file link opens in. `None` means the key predates this
    /// setting; [`Config::sanitize`] resolves it, and every reader should go
    /// through [`Config::file_open_mode`] rather than read it raw.
    #[serde(default, deserialize_with = "de_lenient")]
    pub link_file_open: Option<LinkFileOpen>,
    pub link_file_command: Option<String>,
    pub ssh_loopback_forward: bool,
    pub cursor_blink: bool,
    pub scrollback_limit: usize,
    #[serde(default, deserialize_with = "de_lenient")]
    pub new_tab_position: NewTabPosition,
    #[serde(default, deserialize_with = "de_lenient")]
    pub tab_bar_position: TabBarPosition,
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub right_panel_visible: bool,
    #[serde(default = "default_right_panel_width")]
    pub right_panel_width: f32,
    #[serde(default, deserialize_with = "de_lenient")]
    pub right_panel_tab: RightPanelTab,
    /// Global, not per-overlay — the same call VS Code's
    /// `diffEditor.renderSideBySide` makes.
    #[serde(default, deserialize_with = "de_lenient")]
    pub diff_view: DiffViewMode,
    /// How the code / diff surface shares the window with the terminal — for a
    /// tab that has not been told otherwise. The choice itself is per tab, made
    /// on the document header's context menu; this is the value a fresh tab
    /// starts from, and therefore the one every untold tab is still reading,
    /// which is why the menu never writes it.
    #[serde(default, deserialize_with = "de_lenient")]
    pub document_layout: DocumentLayout,
    /// The share of the terminal column — the flex area between the sidebar and
    /// the right panel — the document column takes when docked. The named
    /// widths land on a third, a half and two thirds; a drag leaves whatever it
    /// leaves, held to [`DOCUMENT_RATIO_MIN`]..=[`DOCUMENT_RATIO_MAX`]. Live
    /// layout narrows it further when the terminal's floor needs the width.
    #[serde(default = "default_document_ratio")]
    pub document_ratio: f32,
    /// The source control panel's history section starts collapsed: a graph
    /// unfurling the first time someone opens the panel is a worse first
    /// impression than one they asked for.
    #[serde(default)]
    pub scm_graph_expanded: bool,
    /// Whether the source control panel lists changed files as a directory
    /// tree rather than one flat run per group. Off by default: the flat list
    /// is what a handful of changes reads best as, and the tree is for the
    /// agent run that touched sixty files across a dozen modules.
    #[serde(default)]
    pub scm_changes_tree: bool,
    /// Whether a file opens in the code panel with soft wrap on. Not a
    /// setting anyone picks up front: it is whatever the status bar's Wrap
    /// toggle (or `ToggleDocumentWrap`) was last left at, so the next file
    /// opens the way the last one was being read.
    #[serde(default)]
    pub editor_soft_wrap: bool,
    /// Whether a Markdown file opens rendered rather than as source — the last
    /// state of the Preview / Edit toggle, remembered the same way as
    /// [`Self::editor_soft_wrap`]. Files that are not Markdown ignore it.
    #[serde(default)]
    pub editor_markdown_preview: bool,
    /// Whether the code editor starts language servers (rust-analyzer,
    /// typescript-language-server, …) for the files it opens, for
    /// diagnostics, completion, hover and go to definition. On by default;
    /// a language whose server is not installed simply goes without.
    #[serde(default = "default_true")]
    pub editor_lsp: bool,
    /// Whether the code editor marks lines that differ from the file's
    /// staged (index) version in its gutter. On by default; flipped by the
    /// `ToggleEditorGitGutter` command and remembered like
    /// [`Self::editor_soft_wrap`].
    #[serde(default = "default_true")]
    pub editor_git_gutter: bool,
    /// Whether Helix modal editing (`Normal`, `Insert`, `Select` modes) is active
    /// in the code editor, inline prompt editor, agent composer, and read-only
    /// views. Off by default.
    #[serde(default)]
    pub helix_mode: bool,
    #[serde(default, deserialize_with = "de_lenient")]
    pub notify_on_command_finish: NotifyMode,
    pub check_for_updates: bool,
    /// Which release feed update checks follow. Stable by default, so an
    /// installation only ever ends up on Nightly by asking for it.
    #[serde(default, deserialize_with = "de_lenient")]
    pub update_channel: UpdateChannel,
    /// Whether a found update is fetched and verified before the user asks for
    /// it. On by default: it turns "spend five minutes downloading" into "press
    /// restart", which is the whole difference between an update people apply
    /// and one they postpone forever. Nothing is ever *installed* without an
    /// explicit choice — the staged package waits in Settings.
    ///
    /// Worth turning off on a metered connection: the packages run 25–30 MB and
    /// a check happens every six hours.
    #[serde(default = "default_true")]
    pub auto_download_updates: bool,
    /// GUI-only locale selection. See `SUPPORTED_GUI_LANGUAGES`.
    #[serde(default = "default_gui_language")]
    pub gui_language: String,
    #[serde(default = "default_notify_threshold_secs")]
    pub notify_threshold_secs: u64,
    #[serde(default = "default_true")]
    pub restore_session: bool,
    /// When closing a tab or a pane asks first — see [`ConfirmClose`].
    #[serde(default, deserialize_with = "de_lenient")]
    pub confirm_close: ConfirmClose,
    #[serde(default = "default_true")]
    pub show_tray_icon: bool,
    #[serde(default, deserialize_with = "de_lenient")]
    pub bell: BellMode,
    /// Whether anterm edits the shell prompt itself. On by default: the inline
    /// editor is what gives a prompt selection, undo, a completion menu and the
    /// fuzzy history — none of which a shell's own line editor offers.
    ///
    /// Off hands every keystroke at the prompt straight to the PTY, so zsh's
    /// ZLE / readline / fish own editing again and the keybindings written in a
    /// dotfile work exactly as they do outside anterm. Shell integration itself
    /// stays on: prompt boundaries, cwd, exit status and notifications are
    /// unaffected. `tab_completion` and `history_search` are anterm's own menus,
    /// so both are moot while this is off.
    #[serde(default = "default_true")]
    pub prompt_editor: bool,
    #[serde(default = "default_true")]
    pub tab_completion: bool,
    #[serde(default = "default_true")]
    pub history_search: bool,

    #[serde(default, deserialize_with = "de_lenient")]
    pub cursor_style: CursorStyle,
    /// The caret's shape while the shell waits at a prompt, whether anterm's
    /// inline editor or the shell's own line editor draws it. `follow` keeps
    /// `cursor_style` everywhere; any other value leaves `cursor_style` to the
    /// programs the shell runs, which is how kitty and ghostty put a bar at
    /// the prompt and a block inside a TUI that never sets a shape itself.
    #[serde(default, deserialize_with = "de_lenient")]
    pub prompt_cursor_style: PromptCursorStyle,

    pub macos_option_as_alt: bool,
    pub mouse_hide_while_typing: bool,
    pub focus_follows_mouse: bool,
    pub mouse_scroll_multiplier: f32,
    /// Spread a wheel detent over several frames instead of jumping the whole
    /// distance at once. Trackpad gestures are left alone — they are already a
    /// continuous stream, and animating one would just add lag.
    #[serde(default = "default_true")]
    pub smooth_scroll: bool,
    #[serde(default = "default_true")]
    pub mouse_reporting: bool,
    /// Which modifier turns the wheel into a font zoom over a terminal.
    ///
    /// Off by default. It used to be the platform modifier, but on macOS that
    /// is ⌘, a key people are holding half the time for something else
    /// entirely, so the font jumped size while they scrolled (#668). ⌘+ / ⌘−
    /// still zoom; this only opts the wheel in.
    #[serde(default, deserialize_with = "de_lenient")]
    pub mouse_zoom_modifier: MouseZoomModifier,
    pub clipboard_trim_trailing_spaces: bool,
    /// In a Claude Code pane, copy joins the rows Claude wrapped itself, so a
    /// paragraph pastes as one line. ⌘⌥C and Copy Raw copy the rows as is.
    #[serde(default)]
    pub copy_join_wrapped: bool,
    pub copy_on_select: bool,
    /// Optional HTTP/SOCKS proxy for anterm's *own* update checks and release
    /// downloads, and (HTTP only) the mobile gateway's relay; when set it is
    /// tried before the system proxy and the environment.
    /// Programs running in a pane are unaffected — they inherit their proxy
    /// from their own environment, as in any other terminal.
    ///
    /// Normalised by [`Config::sanitize`], so `Some` always means a non-blank
    /// value. Examples: `http://127.0.0.1:7890`, `socks5://127.0.0.1:1080`.
    #[serde(default)]
    pub http_proxy: Option<String>,
    #[serde(default = "default_true")]
    pub smart_select: bool,
    #[serde(default = "default_word_separators")]
    pub word_separators: String,
    #[serde(default, deserialize_with = "de_lenient")]
    pub startup_mode: StartupMode,
    #[serde(default = "default_true")]
    pub remember_window_size: bool,

    #[serde(default)]
    pub working_directory: WorkingDirectory,
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// The saved SSH hosts. They live in [`super::SERVERS_FILE`], not in
    /// `config.json` (#911): people sync `config.json` between machines for its
    /// colours and keys, and a list of servers is not something every machine
    /// should carry. Still read from `config.json` so [`Config::load`] can
    /// move an older file's hosts across, never written back to it.
    #[serde(default, skip_serializing)]
    pub ssh_profiles: Vec<crate::ssh_profile::SshProfile>,
    #[serde(default = "default_true")]
    pub verify_host_keys: bool,
    #[serde(default)]
    pub ssh_warn_on_close: bool,
    /// How often each saved host is used, keyed by its id — which means
    /// nothing without the hosts it counts, so it lives in [`super::SERVERS_FILE`]
    /// beside them.
    #[serde(default, skip_serializing)]
    pub ssh_profile_frecency: HashMap<uuid::Uuid, ProfileUsage>,
    /// How often and how lately each shell was opened from the New Tab menu
    /// or its palette command, keyed by the label the inventory shows it
    /// under. Orders the menu's short list of shells the way
    /// `ssh_profile_frecency` orders its hosts.
    #[serde(default)]
    pub shell_frecency: HashMap<String, ProfileUsage>,

    #[serde(default)]
    pub command_frecency: HashMap<String, ProfileUsage>,

    #[serde(default)]
    pub agent_commands: HashMap<String, String>,
    /// The command line a quick launch types for an agent, keyed by its slug:
    /// `{"claude": "claude --dangerously-skip-permissions"}`. An agent with no
    /// entry is launched as its bare binary. A wrapper named here is detected
    /// as that agent without an `agent_commands` entry of its own.
    #[serde(default)]
    pub agent_launch: HashMap<String, String>,
    /// How often and how recently each agent (by slug) was launched or seen
    /// running — what orders the quick-launch commands and picks the agent
    /// "New Agent Tab" opens.
    #[serde(default)]
    pub agent_frecency: HashMap<String, ProfileUsage>,
    /// Approved `.anterm/setup` scripts: repo (`worktree::setup::trust_key`) to
    /// the sha256 of the content approved. A changed script asks again.
    #[serde(default)]
    pub worktree_setup_trust: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub restore_agent_sessions: bool,
    /// Give each pane its own shell history instead of one file every pane
    /// appends to and reads back.
    ///
    /// Seeded from the shell's real history file, so a new pane is not blank,
    /// and merged back into it when the pane closes, so nothing typed is lost.
    /// Off by default because shared history is what a terminal has always
    /// done, and someone who has not asked for the change would experience it
    /// as their history mysteriously forgetting the other window.
    #[serde(default)]
    pub per_pane_history: bool,

    /// This instance is the stand-in for a file that could not be read or
    /// parsed: `load` kept a copy aside and handed back defaults. Never
    /// serialized — it describes how the file *load* went, not a setting —
    /// and it makes [`Config::save`] refuse to run, because writing these
    /// defaults back over the user's hand-edited file is how one typo becomes
    /// permanent data loss (#537). Cleared only by a load that parses.
    #[serde(skip)]
    pub quarantined: bool,
    /// [`super::SERVERS_FILE`] is there but could not be read or parsed. The hosts
    /// in hand are not the ones in it, so [`Config::save`] refuses to run
    /// rather than write them over it — the same rule, for the same reason,
    /// as [`Self::quarantined`].
    #[serde(skip)]
    pub servers_unreadable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct ProfileUsage {
    pub count: u32,
    pub last_used: u64,
}

impl ProfileUsage {
    pub fn score(&self, now: u64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let age_days = now.saturating_sub(self.last_used) as f64 / 86_400.0;
        self.count as f64 / (1.0 + age_days / 7.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkingDirectory {
    #[serde(deserialize_with = "de_lenient")]
    pub strategy: WdStrategy,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WdStrategy {
    #[default]
    Inherit,
    Home,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    #[default]
    Normal,
    Maximized,
    Fullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromptCursorStyle {
    #[default]
    Follow,
    Block,
    Bar,
    Underline,
}

impl PromptCursorStyle {
    /// The shape this names, or `None` when the prompt follows `cursor_style`.
    pub fn shape(self) -> Option<CursorStyle> {
        match self {
            Self::Follow => None,
            Self::Block => Some(CursorStyle::Block),
            Self::Bar => Some(CursorStyle::Bar),
            Self::Underline => Some(CursorStyle::Underline),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NewTabPosition {
    #[default]
    AfterCurrent,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabBarPosition {
    Top,
    #[default]
    Left,
}

/// Native window backdrop material for the Windows GUI. Other platforms retain
/// the value for config synchronization but do not use it for rendering.
/// `Auto` keeps the legacy behavior where theme blur decides between blurred
/// and plain translucent, `Blur` explicitly requests classic WCA acrylic, the
/// material variants fall back to acrylic or plain translucency on older builds
/// inside `src/ui/theme.rs`, and `Off` never requests a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowBackdrop {
    #[default]
    Auto,
    Blur,
    Mica,
    MicaAlt,
    Acrylic,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotifyMode {
    Never,
    #[default]
    Unfocused,
    Always,
}

impl NotifyMode {
    /// Whether a notification goes out, given whether the reader is already
    /// watching what it is about.
    pub fn allows(self, watched: bool) -> bool {
        match self {
            NotifyMode::Never => false,
            NotifyMode::Unfocused => !watched,
            NotifyMode::Always => true,
        }
    }
}

/// Which release feed this installation follows.
///
/// The channel is a property of the installation, not something derived from
/// the version number. Without it the only thing separating a Nightly from a
/// Stable is how their versions happen to sort, which is how a Nightly ends up
/// being walked back onto Stable by an update it never asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    /// Follows the rolling `nightly` prerelease, which is rebuilt from `main`
    /// every night.
    Nightly,
}

/// The modifier that makes the mouse wheel resize the font.
///
/// `Platform` is ⌘ on macOS and Ctrl elsewhere, stored rather than the
/// resolved key so one config file can be shared between machines that
/// disagree about which key that is. `None`, the default, leaves the wheel to
/// scrolling.
///
/// Shift is deliberately not offered: shift+wheel is the escape hatch that
/// scrolls the scrollback out from under a mouse-reporting program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseZoomModifier {
    Platform,
    Ctrl,
    Alt,
    /// The wheel never zooms; every scroll goes to the buffer.
    #[default]
    None,
}

/// When closing a tab or a pane asks first (#1021).
///
/// `WhenBusy` is the question anterm has always asked, and the default: only
/// when something would be cut off — a program still running in the
/// foreground, an agent mid-turn. `Always` also asks about an idle shell, for
/// people who would rather confirm every close than ever lose one; `Never`
/// stops asking about busy panes.
///
/// The warning before dropping a live SSH connection is not governed by this.
/// It is an opt-in of its own, per host or for all of them, so it is still
/// honoured under `Never`: turning this down is not turning that off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfirmClose {
    Never,
    #[default]
    WhenBusy,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BellMode {
    None,
    #[default]
    Visual,
    Audible,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ShellConfig {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// A launcher the user put in the new-tab menu themselves.
///
/// `shell` names the one command that stands in for the platform default;
/// these are the rest — a distro, a container, a REPL, the same shell against a
/// different profile. They are launched exactly as written, which is why `args`
/// crosses as user-authored: anterm has no defaults to contribute to a command it
/// did not choose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct CustomShell {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

pub fn default_font_fallbacks() -> Vec<String> {
    let names: &[&str] = if cfg!(target_os = "macos") {
        &[
            "Menlo",
            "Hasklug Nerd Font Mono",
            "Maple Mono NF CN",
            "PingFang SC",
            "Apple Color Emoji",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "Maple Mono NF CN",
            "Cascadia Mono",
            "Microsoft YaHei",
            "Segoe UI Emoji",
        ]
    } else {
        &[
            "Maple Mono NF CN",
            "DejaVu Sans Mono",
            "Noto Sans CJK SC",
            "Noto Color Emoji",
        ]
    };
    names.iter().map(|n| n.to_string()).collect()
}

pub fn platform_last_resort_fallbacks() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["PingFang SC", "Apple Color Emoji"]
    } else if cfg!(target_os = "windows") {
        &["Microsoft YaHei", "Segoe UI Emoji"]
    } else {
        &["Noto Sans CJK SC", "Noto Color Emoji"]
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_family: "Hack".to_string(),
            font_fallbacks: default_font_fallbacks(),
            font_family_bold: None,
            font_family_italic: None,
            font_features: None,
            font_thicken: true,
            font_size: 15.0,
            line_height: 1.4,
            ui_font_size: default_ui_font_size(),
            ui_font_family: None,
            theme: "light".to_string(),
            theme_preset: "light".to_string(),
            theme_follow_system: false,
            theme_preset_light: "light".to_string(),
            theme_preset_dark: "dark".to_string(),
            theme_legible_palette: true,
            window_opacity: None,
            window_blur: None,
            window_backdrop: WindowBackdrop::default(),
            dim_inactive_panes: true,
            auto_hide_titlebar_buttons: false,
            keybindings: HashMap::new(),
            keybinding_preset: default_preset(),
            prefix: default_prefix(),
            shell: None,
            custom_shells: Vec::new(),
            link_url: true,
            link_file_open: Some(LinkFileOpen::Internal),
            link_file_command: None,
            ssh_loopback_forward: true,
            cursor_blink: true,
            scrollback_limit: 10_000,
            new_tab_position: NewTabPosition::AfterCurrent,
            tab_bar_position: TabBarPosition::Left,
            sidebar_width: default_sidebar_width(),
            sidebar_collapsed: false,
            right_panel_visible: false,
            right_panel_width: default_right_panel_width(),
            right_panel_tab: RightPanelTab::Info,
            diff_view: DiffViewMode::Split,
            document_layout: DocumentLayout::default(),
            document_ratio: default_document_ratio(),
            scm_graph_expanded: false,
            scm_changes_tree: false,
            editor_soft_wrap: false,
            editor_markdown_preview: false,
            editor_lsp: true,
            editor_git_gutter: true,
            helix_mode: false,
            notify_on_command_finish: NotifyMode::Unfocused,
            check_for_updates: true,
            update_channel: UpdateChannel::default(),
            auto_download_updates: true,
            gui_language: default_gui_language(),
            notify_threshold_secs: default_notify_threshold_secs(),
            restore_session: true,
            confirm_close: ConfirmClose::WhenBusy,
            show_tray_icon: true,
            bell: BellMode::Visual,
            prompt_editor: true,
            tab_completion: true,
            history_search: true,
            cursor_style: CursorStyle::Block,
            prompt_cursor_style: PromptCursorStyle::Follow,
            macos_option_as_alt: false,
            mouse_hide_while_typing: true,
            focus_follows_mouse: false,
            mouse_scroll_multiplier: 1.0,
            smooth_scroll: true,
            mouse_reporting: true,
            mouse_zoom_modifier: MouseZoomModifier::default(),
            clipboard_trim_trailing_spaces: false,
            copy_join_wrapped: false,
            copy_on_select: false,
            http_proxy: None,
            smart_select: true,
            word_separators: default_word_separators(),
            startup_mode: StartupMode::Normal,
            remember_window_size: true,
            working_directory: WorkingDirectory::default(),
            env: HashMap::new(),
            ssh_profiles: Vec::new(),
            verify_host_keys: true,
            ssh_warn_on_close: false,
            ssh_profile_frecency: HashMap::new(),
            shell_frecency: HashMap::new(),
            command_frecency: HashMap::new(),
            agent_commands: HashMap::new(),
            agent_launch: HashMap::new(),
            agent_frecency: HashMap::new(),
            worktree_setup_trust: HashMap::new(),
            restore_agent_sessions: true,
            per_pane_history: false,
            quarantined: false,
            servers_unreadable: false,
        }
    }
}

fn default_preset() -> String {
    "default".to_string()
}

fn default_true() -> bool {
    true
}

pub(super) fn default_gui_language() -> String {
    "en".to_string()
}

fn default_word_separators() -> String {
    ",│`|:\"' ()[]{}<>\t".to_string()
}

fn default_notify_threshold_secs() -> u64 {
    10
}

fn default_prefix() -> String {
    "ctrl-b".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RightPanelTab {
    #[default]
    Info,
    /// The source control panel. Renamed from `Changes` in place rather than
    /// added alongside it: `rename` works in both directions, so a config
    /// written by this version still says `"changes"` and an older build reads
    /// it back unchanged.
    #[serde(rename = "changes", alias = "scm", alias = "git")]
    Scm,
    Files,
    /// Project-wide content search. Added as a tab of its own, which costs a
    /// rolled-back build one thing: it does not know `"search"`, falls through
    /// `de_lenient` to `Info`, and opens there. Nothing else is lost.
    Search,
}

/// What opens when a file link in the grid is clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkFileOpen {
    /// anterm's own editor. The default, and the only one of the three that can
    /// honour a `:line:column` suffix or open a file on a remote host.
    #[default]
    Internal,
    /// Whatever the OS has the file associated with.
    System,
    /// [`Config::link_file_command`], for people who want their own editor.
    Command,
}

pub use anterm_protocol::diff::DiffViewMode;

/// The detail panel's resting width: the v4 design's 280px column, which is
/// what its insets (12px lists, 20px text) and the tab row were drawn against.
pub(super) fn default_right_panel_width() -> f32 {
    280.
}

/// Where the code / diff surface is drawn.
///
/// It used to be one thing — a full-workspace overlay — so there was nothing to
/// name. Docking it beside the terminal is the default now: opening a file to
/// read it while an agent talks underneath was the reason the built-in editor
/// exists, and an overlay covers the agent. `Fill` is that overlay, kept for
/// anyone who wants the whole window for the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentLayout {
    #[default]
    Dock,
    Fill,
}

pub(super) fn default_document_ratio() -> f32 {
    0.5
}

/// The band `document_ratio` is held to — in the file *and* at the divider.
///
/// One shared pair rather than two, for the reason `FONT_SIZE_MIN` and its
/// stepper are one pair (#550): a GUI that clamps somewhere the file does not
/// writes a value `sanitize` then moves, and the user finds the thing they
/// dropped somewhere else on the next launch. The divider clamps in *pixels*
/// against the terminal's floor as well, which is the tighter limit on a narrow
/// window; on a wide one this band is, and both ends of it have to be reachable
/// and keepable.
pub const DOCUMENT_RATIO_MIN: f32 = 0.2;
pub const DOCUMENT_RATIO_MAX: f32 = 0.8;

/// The named shares of the terminal column a document column can be snapped to,
/// in the order the segmented control and the divider's double-click cycle use.
pub const DOCUMENT_RATIO_THIRD: f32 = 1. / 3.;
pub const DOCUMENT_RATIO_HALF: f32 = 0.5;
pub const DOCUMENT_RATIO_TWO_THIRDS: f32 = 2. / 3.;
pub const DOCUMENT_RATIO_STOPS: [f32; 3] = [
    DOCUMENT_RATIO_THIRD,
    DOCUMENT_RATIO_HALF,
    DOCUMENT_RATIO_TWO_THIRDS,
];

/// The rem the chrome has always been laid out against — gpui's own default,
/// which is what `text_sm()` and `text_xs()` resolve 14px and 12px from. Left
/// alone, the interface looks exactly as it did before the size was settable.
pub const UI_FONT_SIZE_DEFAULT: f32 = 16.0;
pub const UI_FONT_SIZE_MIN: f32 = 12.0;
pub const UI_FONT_SIZE_MAX: f32 = 24.0;

/// The terminal's font-size and line-height bounds, shared by `sanitize` and
/// the GUI's steppers. The GUI used to keep its own, narrower pair (6–48,
/// 1.0–2.0), so a value inside the config range but outside the GUI's got
/// pushed the *wrong way* by a single step — `font_size: 50` shrank to 48 on
/// "+" — and written back to the file, permanently (#550). One range, defined
/// where the value is validated, is the `ui_font_size` precedent.
pub const FONT_SIZE_MIN: f32 = 4.0;
pub const FONT_SIZE_MAX: f32 = 256.0;
pub const LINE_HEIGHT_MIN: f32 = 0.5;
pub const LINE_HEIGHT_MAX: f32 = 4.0;

pub(super) fn default_ui_font_size() -> f32 {
    UI_FONT_SIZE_DEFAULT
}

pub(super) fn default_sidebar_width() -> f32 {
    260.0
}

pub const MAX_SCROLLBACK: usize = 100_000;

/// `keybindings`, read one line at a time.
///
/// [`de_lenient`] is all-or-nothing per field, which for a map means one bad
/// line throwing away every good one. Here each entry stands on its own: the
/// ones that name a shortcut or a list of shortcuts bind, the ones that do not
/// are logged and dropped. A `keybindings` that is not an object at all falls
/// back to an empty map rather than failing the file.
fn de_keybindings<'de, D>(deserializer: D) -> Result<HashMap<String, KeybindingOverride>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Object(entries) = value else {
        log::warn!("ignoring `keybindings` {value}: expected an object of action name to shortcut");
        return Ok(HashMap::new());
    };
    let mut bindings = HashMap::with_capacity(entries.len());
    for (action, raw) in entries {
        match KeybindingOverride::deserialize(&raw) {
            Ok(binding) => {
                bindings.insert(action, binding);
            }
            Err(e) => log::warn!(
                "ignoring keybinding for {action:?}: {raw} is not a shortcut, \
                 a list of shortcuts, or \"\" to unbind ({e})"
            ),
        }
    }
    Ok(bindings)
}

pub(crate) use anterm_protocol::util::de_lenient;

/// `shell`, read the way Settings writes it: no program means the login
/// shell. A hand-edited `"shell": { "args": ["-l"] }` — or any other shape
/// that does not name a program — used to fail the whole file, and every other
/// setting went with it into quarantine.
fn de_shell<'de, D>(deserializer: D) -> Result<Option<ShellConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    match ShellConfig::deserialize(&value) {
        Ok(shell) if !shell.program.trim().is_empty() => Ok(Some(shell)),
        Ok(_) => Ok(None),
        Err(e) => {
            log::warn!("ignoring invalid shell {value}: {e}; using the login shell");
            Ok(None)
        }
    }
}
