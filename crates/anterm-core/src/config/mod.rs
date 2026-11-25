mod paths;
mod schema;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use crate::util::{strip_bom, unix_now};

pub use paths::{
    PORTABLE_DATA_DIR, PORTABLE_MARKER, config_dir_path, config_path, default_config_dir,
    machine_config_dir, portable_data_dir, set_config_dir, write_atomic, write_atomic_private,
};
pub(crate) use paths::{quarantine, quarantine_by_rename};

pub use schema::{
    BellMode, Config, ConfirmClose, CursorStyle, CustomShell, DOCUMENT_RATIO_HALF,
    DOCUMENT_RATIO_MAX, DOCUMENT_RATIO_MIN, DOCUMENT_RATIO_STOPS, DOCUMENT_RATIO_THIRD,
    DOCUMENT_RATIO_TWO_THIRDS, DiffViewMode, DocumentLayout, FONT_SIZE_MAX, FONT_SIZE_MIN,
    FontFeatures, KeybindingOverride, LINE_HEIGHT_MAX, LINE_HEIGHT_MIN, LinkFileOpen,
    MAX_SCROLLBACK, MouseZoomModifier, NewTabPosition, NotifyMode, ProfileUsage,
    PromptCursorStyle, RightPanelTab, SUPPORTED_GUI_LANGUAGES, ShellConfig, StartupMode,
    TabBarPosition, UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX, UI_FONT_SIZE_MIN, UpdateChannel,
    WdStrategy, WindowBackdrop, WorkingDirectory, default_font_fallbacks,
    platform_last_resort_fallbacks,
};

use schema::{
    default_document_ratio, default_gui_language, default_right_panel_width, default_sidebar_width,
    default_ui_font_size,
};

/// Where the saved SSH hosts live, beside `config.json` (#911).
pub const SERVERS_FILE: &str = "servers.json";

/// What [`SERVERS_FILE`] holds. The keys are the ones `config.json` used to
/// carry, so a hand-moved block pastes straight across.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
struct ServersFile {
    ssh_profiles: Vec<crate::ssh_profile::SshProfile>,
    ssh_profile_frecency: HashMap<uuid::Uuid, ProfileUsage>,
}

#[derive(Serialize)]
struct ServersFileRef<'a> {
    ssh_profiles: &'a [crate::ssh_profile::SshProfile],
    ssh_profile_frecency: &'a HashMap<uuid::Uuid, ProfileUsage>,
}

/// Whether a `config.json` still carries the keys that moved to
/// [`SERVERS_FILE`] — the cue for the one-time move.
#[derive(Deserialize)]
struct LegacyServerKeys {
    #[serde(default)]
    ssh_profiles: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    ssh_profile_frecency: Option<serde::de::IgnoredAny>,
}

fn has_legacy_server_keys(text: &str) -> bool {
    serde_json::from_str::<LegacyServerKeys>(strip_bom(text))
        .is_ok_and(|keys| keys.ssh_profiles.is_some() || keys.ssh_profile_frecency.is_some())
}

/// How the file behind a [`Config::load_with_outcome`] went — the answer a
/// hot-reload watcher needs before it swaps a running app onto the result,
/// because "no file yet" and "a broken file" must not do the same thing
/// (#537).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// The file parsed (after the usual field-level leniency).
    Parsed,
    /// There is no file (or no config dir yet): the defaults simply are the
    /// config, and saving them is fine.
    Absent,
    /// The file existed but did not parse. A copy was kept beside it, and the
    /// returned config is the defaults with writes suppressed — saving over
    /// the broken file would make one typo permanent.
    Quarantined,
    /// The file is there but could not be read at all. Writes are suppressed
    /// the same way, but nothing was parked beside it: there was nothing
    /// readable to copy. Distinct from [`LoadOutcome::Quarantined`] because
    /// telling the user to look in `config.json.corrupt` for contents that
    /// were never written there sends them after a file that is not there.
    Unreadable,
}

impl LoadOutcome {
    /// Whether the file is standing between the user and their settings: the
    /// values handed back are defaults with writes suppressed, not anything
    /// the user wrote.
    pub fn failed(self) -> bool {
        matches!(self, Self::Quarantined | Self::Unreadable)
    }
}

impl Config {
    pub fn load() -> Self {
        Self::load_with_outcome().0
    }

    /// [`Config::load`] with the verdict the file earned. Most callers want
    /// the values either way and use `load`; the watcher that swaps a running
    /// app onto the result needs the outcome to keep a broken file from
    /// evicting the settings the app is running on.
    pub fn load_with_outcome() -> (Self, LoadOutcome) {
        match Self::path() {
            Some(path) => Self::load_from(&path),
            None => (Config::default(), LoadOutcome::Absent),
        }
    }

    /// [`Config::load_with_outcome`] for the `config.json` at `path`, with the
    /// saved hosts read from the [`SERVERS_FILE`] beside it.
    ///
    /// A `config.json` from before the split still holds the hosts itself.
    /// When there is no [`SERVERS_FILE`] yet they are moved into one, once:
    /// the new file is written first and `config.json` rewritten without them
    /// only after that has landed, so a failure anywhere leaves them where
    /// they were. When both files are there the new one wins and the copy in
    /// `config.json` is not read — it is what an older anterm wrote after the
    /// move, or a synced file brought in, and it falls out of `config.json`
    /// at its next save.
    pub(crate) fn load_from(path: &Path) -> (Self, LoadOutcome) {
        let (mut cfg, outcome, text) = Self::load_config_file(path);
        let servers = path.with_file_name(SERVERS_FILE);
        let found = cfg.load_servers(&servers);
        if !found
            && let Some(text) = text
            && has_legacy_server_keys(&text)
        {
            cfg.move_servers_out(&text, path, &servers);
        }
        (cfg, outcome)
    }

    /// `config.json` alone, with the text it parsed from.
    fn load_config_file(path: &Path) -> (Self, LoadOutcome, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Config::default(), LoadOutcome::Absent, None);
            }
            Err(e) => {
                // Unreadable is not unparseable, but the rule is the same:
                // what cannot be read must not be overwritten. There may be
                // nothing readable to keep a copy of, so nothing is parked —
                // the file itself is still where the user left it.
                log::warn!(
                    "failed to read config at {}: {e}; using defaults, writes suppressed",
                    path.display()
                );
                let mut cfg = Config::default();
                cfg.quarantined = true;
                return (cfg, LoadOutcome::Unreadable, None);
            }
        };
        match serde_json::from_str::<Config>(strip_bom(&text)) {
            Ok(mut cfg) => {
                cfg.sanitize();
                (cfg, LoadOutcome::Parsed, Some(text))
            }
            Err(e) => {
                // The next `save` overwrites this file wholesale, so handing
                // back defaults with no trace quietly discards whatever the
                // file held the moment anything — a dragged sidebar divider —
                // writes. Park a copy first, the way `WindowViews::load`
                // does, and mark the stand-in so `save` refuses to run for it.
                log::warn!(
                    "failed to parse config at {}: {e}; keeping it aside and using defaults",
                    path.display()
                );
                quarantine(path);
                let mut cfg = Config::default();
                cfg.quarantined = true;
                (cfg, LoadOutcome::Quarantined, None)
            }
        }
    }

    /// Takes the hosts from the [`SERVERS_FILE`] at `path`, if there is one.
    /// Returns whether there was — a file that is there but broken counts, so
    /// that nothing is moved over the top of it.
    fn load_servers(&mut self, path: &Path) -> bool {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
            Err(e) => {
                log::warn!(
                    "failed to read {}: {e}; saved SSH hosts unavailable, writes suppressed",
                    path.display()
                );
                self.servers_unreadable = true;
                return true;
            }
        };
        match serde_json::from_str::<ServersFile>(strip_bom(&text)) {
            Ok(servers) => {
                self.ssh_profiles = servers.ssh_profiles;
                self.ssh_profile_frecency = servers.ssh_profile_frecency;
            }
            Err(e) => {
                log::warn!(
                    "failed to parse {}: {e}; keeping it aside, writes suppressed",
                    path.display()
                );
                quarantine(path);
                self.servers_unreadable = true;
            }
        }
        true
    }

    /// The one-time move of the hosts out of `config.json` (#911). New file
    /// first: until it has landed, `config.json` is the only copy.
    ///
    /// `config.json` is then rewritten from its own `text` with just those
    /// two keys taken out, not from `self` the way a save writes it. Nobody
    /// asked for this write, so it should change nothing else: a hand-kept
    /// file stays the handful of keys it was rather than growing every
    /// default, and a key this build does not know — one a newer anterm on a
    /// synced machine wrote — survives.
    fn move_servers_out(&self, text: &str, config: &Path, servers: &Path) {
        if let Err(e) = self.write_servers(servers) {
            log::warn!(
                "could not move SSH hosts to {}: {e}; leaving them in config.json",
                servers.display()
            );
            return;
        }
        let rewrite = || -> std::io::Result<()> {
            let mut value: serde_json::Value =
                serde_json::from_str(strip_bom(text)).map_err(std::io::Error::other)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("ssh_profiles");
                object.remove("ssh_profile_frecency");
            }
            let text = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
            write_atomic(config, text.as_bytes())
        };
        match rewrite() {
            Ok(()) => log::info!("moved SSH hosts from config.json to {}", servers.display()),
            Err(e) => log::warn!(
                "moved SSH hosts to {}, but could not rewrite config.json: {e}",
                servers.display()
            ),
        }
    }

    /// What a clicked file link should open in, with the pre-setting default
    /// filled in for a `Config` that never went through [`Self::sanitize`].
    pub fn file_open_mode(&self) -> LinkFileOpen {
        self.link_file_open.unwrap_or_default()
    }

    fn sanitize(&mut self) {
        if !self.font_size.is_finite() || self.font_size <= 0.0 {
            self.font_size = Config::default().font_size;
        }
        self.font_size = self.font_size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        if !self.line_height.is_finite() || self.line_height <= 0.0 {
            self.line_height = Config::default().line_height;
        }
        self.line_height = self.line_height.clamp(LINE_HEIGHT_MIN, LINE_HEIGHT_MAX);
        if !self.ui_font_size.is_finite() || self.ui_font_size <= 0.0 {
            self.ui_font_size = default_ui_font_size();
        }
        // The whole chrome is a multiple of this, so a wild value does not
        // shrink one label — it makes the window unusable. Keep the range to
        // sizes the layout still holds together at.
        self.ui_font_size = self.ui_font_size.clamp(UI_FONT_SIZE_MIN, UI_FONT_SIZE_MAX);
        if let Some(family) = &self.ui_font_family
            && family.trim().is_empty()
        {
            self.ui_font_family = None;
        }
        self.scrollback_limit = self.scrollback_limit.clamp(100, MAX_SCROLLBACK);
        if !self.mouse_scroll_multiplier.is_finite() || self.mouse_scroll_multiplier <= 0.0 {
            self.mouse_scroll_multiplier = Config::default().mouse_scroll_multiplier;
        }
        self.mouse_scroll_multiplier = self.mouse_scroll_multiplier.clamp(0.1, 10.0);
        self.notify_threshold_secs = self.notify_threshold_secs.clamp(1, 3600);
        self.window_opacity = self
            .window_opacity
            .filter(|o| o.is_finite())
            .map(|o| o.clamp(0.2, 1.0));
        if !self.sidebar_width.is_finite() || self.sidebar_width <= 0.0 {
            self.sidebar_width = default_sidebar_width();
        }
        self.sidebar_width = self.sidebar_width.clamp(100.0, 2000.0);
        if !self.right_panel_width.is_finite() || self.right_panel_width <= 0.0 {
            self.right_panel_width = default_right_panel_width();
        }
        self.right_panel_width = self.right_panel_width.clamp(100.0, 2000.0);
        if !self.document_ratio.is_finite() || self.document_ratio <= 0.0 {
            self.document_ratio = default_document_ratio();
        }
        self.document_ratio = self
            .document_ratio
            .clamp(DOCUMENT_RATIO_MIN, DOCUMENT_RATIO_MAX);
        if let Some(command) = &self.link_file_command
            && command.trim().is_empty()
        {
            self.link_file_command = None;
        }
        // Absent means a config written before file links could open in the
        // built-in editor. Anyone who had already pointed `link_file_command`
        // at their own editor asked for that and keeps it; everyone else, who
        // was silently getting the OS file association, gets the editor. A
        // fresh `link_file_open` in a `Default` config would have taken the
        // first group's editor away on upgrade.
        self.link_file_open
            .get_or_insert(match self.link_file_command {
                Some(_) => LinkFileOpen::Command,
                None => LinkFileOpen::Internal,
            });
        // Normalise here so every reader can take the field as-is instead of
        // repeating a trim-and-drop-if-blank dance.
        self.http_proxy = self
            .http_proxy
            .take()
            .map(|proxy| proxy.trim().to_string())
            .filter(|proxy| !proxy.is_empty());
        if !SUPPORTED_GUI_LANGUAGES.contains(&self.gui_language.as_str()) {
            self.gui_language = default_gui_language();
        }
    }

    pub fn save(&self) {
        if let Err(error) = self.try_save() {
            log::warn!("failed to save config: {error}");
        }
    }

    /// Persist without hiding a failure from an interactive settings editor.
    pub fn try_save(&self) -> std::io::Result<()> {
        let path = Self::path()
            .ok_or_else(|| std::io::Error::other("configuration directory is unavailable"))?;
        self.try_save_to(&path)
    }

    /// [`Config::try_save`] for the `config.json` at `path`, and the
    /// [`SERVERS_FILE`] beside it.
    pub(crate) fn try_save_to(&self, path: &Path) -> std::io::Result<()> {
        if self.quarantined {
            return Err(std::io::Error::other(
                "the existing configuration could not be read; repair it before saving",
            ));
        }
        if self.servers_unreadable {
            return Err(std::io::Error::other(format!(
                "{SERVERS_FILE} could not be read; repair it before saving"
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Hosts first, for the same reason the move out of `config.json`
        // writes them first: a `config.json` that lands without them while
        // their own file failed to would have dropped them.
        self.write_servers(&path.with_file_name(SERVERS_FILE))?;
        self.write_config(path)
    }

    fn write_config(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(path, text.as_bytes())
    }

    /// Writes the hosts to their own file — unless it already says exactly
    /// this, which is nearly every save: a dragged divider has nothing to do
    /// with the hosts, and rewriting the file anyway would wake the watcher a
    /// second time. No hosts and no file stays no file.
    fn write_servers(&self, path: &Path) -> std::io::Result<()> {
        let on_disk = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<ServersFile>(strip_bom(&text)).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(ServersFile::default()),
            Err(_) => None,
        };
        if on_disk.as_ref().is_some_and(|held| {
            held.ssh_profiles == self.ssh_profiles
                && held.ssh_profile_frecency == self.ssh_profile_frecency
        }) {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&ServersFileRef {
            ssh_profiles: &self.ssh_profiles,
            ssh_profile_frecency: &self.ssh_profile_frecency,
        })
        .map_err(std::io::Error::other)?;
        // Host names, users and key paths: nobody else's business.
        write_atomic_private(path, text.as_bytes())
    }

    fn path() -> Option<PathBuf> {
        config_path("config.json")
    }
}

pub fn shell_command() -> Option<(String, Vec<String>)> {
    Config::load().shell.map(|s| (s.program, s.args))
}

pub fn working_directory_base() -> Option<PathBuf> {
    let wd = Config::load().working_directory;
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match wd.strategy {
        WdStrategy::Inherit => None,
        WdStrategy::Home => home(),
        WdStrategy::Custom => {
            let p = wd.path.trim();
            if p.is_empty() {
                home()
            } else {
                Some(PathBuf::from(p))
            }
        }
    }
}

pub fn extra_env() -> HashMap<String, String> {
    Config::load().env
}

/// Every wrapper name the daemon should read as an agent: `agent_commands`,
/// plus the programs `agent_launch` entries run (see
/// [`crate::cli_agent::launch_aliases`]). An explicit `agent_commands`
/// entry wins over an implied one.
///
/// Read again whenever `config.json` changes rather than once per process: the
/// daemon outlives any number of edits, and a launch override added while it
/// runs has to be recognised by the very next launch that uses it.
pub fn agent_detection_aliases() -> std::sync::Arc<HashMap<String, String>> {
    type Stamp = Option<(std::time::SystemTime, u64)>;
    static CACHE: std::sync::Mutex<Option<(Stamp, std::sync::Arc<HashMap<String, String>>)>> =
        std::sync::Mutex::new(None);
    let stamp: Stamp = Config::path()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| Some((m.modified().ok()?, m.len())));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((seen, aliases)) = cache.as_ref()
        && *seen == stamp
    {
        return aliases.clone();
    }
    let cfg = Config::load();
    let aliases = std::sync::Arc::new(detection_aliases(&cfg.agent_commands, &cfg.agent_launch));
    *cache = Some((stamp, aliases.clone()));
    aliases
}

/// [`agent_detection_aliases`] over the two maps it reads, for a caller that
/// already holds them.
pub fn detection_aliases(
    commands: &HashMap<String, String>,
    launch: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut aliases = crate::cli_agent::launch_aliases(launch);
    for (name, slug) in commands {
        aliases.insert(name.to_ascii_lowercase(), slug.clone());
    }
    aliases
}
