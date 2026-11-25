use std::io;
use std::path::{Path, PathBuf};

use crate::cli_agent::CLIAgent;
use crate::host::Host;

use super::strategies::{
    CLAUDE_HOOK_EVENTS, CODEBUDDY_HOOK_EVENTS, CODEX_HOOK_EVENTS, CRUSH_HOOK_EVENTS,
    CURSOR_HOOK_EVENTS, DROID_HOOK_EVENTS, EMPRYO_HOOK_EVENTS, GEMINI_HOOK_EVENTS,
    JCODE_HOOK_EVENTS, KIMI_HOOK_EVENTS, OWNED_FILE_STEM_JS, OWNED_FILE_STEM_JSON,
    QODER_HOOK_EVENTS, QWEN_HOOK_EVENTS, TRAE_CODE_HOOK_EVENTS, antigravity_hook_set,
    enable_codex_hooks_feature, hook_map_install, hook_map_state, hook_map_uninstall,
    jcode_hooks_install, jcode_hooks_state, jcode_hooks_uninstall, muse_hooks_install,
    muse_hooks_state, muse_hooks_uninstall, named_hook_set_install, named_hook_set_state,
    named_hook_set_uninstall, owned_file_content, owned_file_install, owned_file_state,
    owned_file_uninstall, toml_hooks_install, toml_hooks_state, toml_hooks_uninstall,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HookAgent {
    Claude,
    Codex,
    TraeCode,
    Copilot,
    OpenCode,
    Pi,
    Grok,
    OhMyPi,
    Gemini,
    Droid,
    Qwen,
    Goose,
    Kimi,
    QoderCLI,
    Crush,
    CodeBuddy,
    Cursor,
    PrimeAgent,
    Antigravity,
    QoderCLICn,
    Empryo,
    Jcode,
    Muse,
}

impl HookAgent {
    pub const ALL: [HookAgent; 23] = [
        HookAgent::Claude,
        HookAgent::Codex,
        HookAgent::TraeCode,
        HookAgent::Copilot,
        HookAgent::OpenCode,
        HookAgent::Pi,
        HookAgent::Grok,
        HookAgent::OhMyPi,
        HookAgent::Gemini,
        HookAgent::Droid,
        HookAgent::Qwen,
        HookAgent::Goose,
        HookAgent::Kimi,
        HookAgent::QoderCLI,
        HookAgent::QoderCLICn,
        HookAgent::Crush,
        HookAgent::CodeBuddy,
        HookAgent::Cursor,
        HookAgent::PrimeAgent,
        HookAgent::Antigravity,
        HookAgent::Empryo,
        HookAgent::Jcode,
        HookAgent::Muse,
    ];

    /// The hooks behind a detected agent process, if it has any.
    ///
    /// Process detection knows far more agents than hooks do — the ones with no
    /// arm here report status some other way, or not at all. The match is
    /// exhaustive on purpose: a newly detected agent has to say which it is.
    pub fn of_detected(agent: CLIAgent) -> Option<HookAgent> {
        match agent {
            CLIAgent::Claude => Some(HookAgent::Claude),
            CLIAgent::Codex => Some(HookAgent::Codex),
            CLIAgent::TraeCode => Some(HookAgent::TraeCode),
            CLIAgent::Copilot => Some(HookAgent::Copilot),
            CLIAgent::OpenCode => Some(HookAgent::OpenCode),
            CLIAgent::Pi => Some(HookAgent::Pi),
            CLIAgent::Grok => Some(HookAgent::Grok),
            CLIAgent::OhMyPi => Some(HookAgent::OhMyPi),
            CLIAgent::PrimeAgent => Some(HookAgent::PrimeAgent),
            CLIAgent::Antigravity => Some(HookAgent::Antigravity),
            CLIAgent::Empryo => Some(HookAgent::Empryo),
            CLIAgent::Jcode => Some(HookAgent::Jcode),
            CLIAgent::Muse => Some(HookAgent::Muse),
            CLIAgent::Gemini => Some(HookAgent::Gemini),
            CLIAgent::Droid => Some(HookAgent::Droid),
            CLIAgent::Qwen => Some(HookAgent::Qwen),
            CLIAgent::Goose => Some(HookAgent::Goose),
            CLIAgent::Kimi => Some(HookAgent::Kimi),
            CLIAgent::QoderCLI => Some(HookAgent::QoderCLI),
            CLIAgent::QoderCLICn => Some(HookAgent::QoderCLICn),
            CLIAgent::Crush => Some(HookAgent::Crush),
            CLIAgent::CodeBuddy => Some(HookAgent::CodeBuddy),
            CLIAgent::Cursor => Some(HookAgent::Cursor),
            CLIAgent::Aider
            | CLIAgent::Amp
            | CLIAgent::Auggie
            | CLIAgent::Hermes
            | CLIAgent::Vibe => None,
        }
    }

    /// The events this agent's hooks merge into a shared JSON config, if that
    /// is how it takes them. `None` means the agent owns a generated file
    /// instead — see [`owned_file_content`].
    pub(super) fn hook_map_events(self) -> Option<&'static [(&'static str, &'static str)]> {
        match self {
            HookAgent::Claude => Some(CLAUDE_HOOK_EVENTS),
            HookAgent::Codex => Some(CODEX_HOOK_EVENTS),
            HookAgent::TraeCode => Some(TRAE_CODE_HOOK_EVENTS),
            HookAgent::Gemini => Some(GEMINI_HOOK_EVENTS),
            HookAgent::Droid => Some(DROID_HOOK_EVENTS),
            HookAgent::Qwen => Some(QWEN_HOOK_EVENTS),
            HookAgent::QoderCLI | HookAgent::QoderCLICn => Some(QODER_HOOK_EVENTS),
            HookAgent::Empryo => Some(EMPRYO_HOOK_EVENTS),
            HookAgent::Jcode => Some(JCODE_HOOK_EVENTS),
            HookAgent::Crush => Some(CRUSH_HOOK_EVENTS),
            HookAgent::CodeBuddy => Some(CODEBUDDY_HOOK_EVENTS),
            HookAgent::Cursor => Some(CURSOR_HOOK_EVENTS),
            HookAgent::Copilot
            | HookAgent::OpenCode
            | HookAgent::Pi
            | HookAgent::Grok
            | HookAgent::OhMyPi
            | HookAgent::PrimeAgent
            | HookAgent::Antigravity
            | HookAgent::Goose
            | HookAgent::Kimi
            | HookAgent::Muse => None,
        }
    }

    /// The events this agent's hooks merge into a shared TOML config, if that
    /// is how it takes them — the third strategy, for the agents whose hooks
    /// live as `[[hooks]]` entries in a config file the user also hand-edits.
    fn toml_hook_events(self) -> Option<&'static [(&'static str, &'static str)]> {
        match self {
            HookAgent::Kimi => Some(KIMI_HOOK_EVENTS),
            _ => None,
        }
    }

    /// Whether this agent's hook-map entries carry `command` and `matcher` at
    /// the top level rather than nesting a `hooks` array of `{type, command}`
    /// objects inside the matcher. Claude's shape is the latter; Crush flattened
    /// it, and still calls itself Claude-Code-compatible on the wire. Cursor's
    /// `hooks.json` was flat from the start.
    pub(super) fn flat_hook_map(self) -> bool {
        matches!(self, HookAgent::Crush | HookAgent::Cursor)
    }

    /// The `name` anterm's hook-map entries carry, for an agent that shows one.
    /// Gemini CLI prints `Executing Hook: <name>` above its input while a hook
    /// runs, and falls back to the whole command — the quoted path of the
    /// anterm binary and its arguments — for a hook without a name.
    pub(super) fn hook_entry_name(self) -> Option<&'static str> {
        match self {
            HookAgent::Gemini => Some("anterm"),
            _ => None,
        }
    }

    /// The schema version a hook map has to declare at its root, if the agent
    /// demands one. Cursor ignores a `hooks.json` without `"version": 1`, so a
    /// file anterm creates — or finds without one — gets it; one the user already
    /// versioned is left alone.
    pub(super) fn hook_map_version(self) -> Option<u64> {
        match self {
            HookAgent::Cursor => Some(1),
            _ => None,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            HookAgent::Claude => "claude",
            HookAgent::Codex => "codex",
            HookAgent::TraeCode => "traecli",
            HookAgent::Copilot => "copilot",
            HookAgent::OpenCode => "opencode",
            HookAgent::Pi => "pi",
            HookAgent::Grok => "grok",
            HookAgent::OhMyPi => "omp",
            HookAgent::PrimeAgent => "prime-agent",
            HookAgent::Antigravity => "antigravity",
            HookAgent::Empryo => "empryo",
            HookAgent::Jcode => "jcode",
            HookAgent::Muse => "muse",
            HookAgent::Gemini => "gemini",
            HookAgent::Droid => "droid",
            HookAgent::Qwen => "qwen",
            HookAgent::Goose => "goose",
            HookAgent::Kimi => "kimi",
            HookAgent::QoderCLI => "qodercli",
            HookAgent::Crush => "crush",
            HookAgent::CodeBuddy => "codebuddy",
            HookAgent::Cursor => "cursor",
            HookAgent::QoderCLICn => "qoderclicn",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            HookAgent::Claude => "Claude Code",
            HookAgent::Codex => "Codex",
            HookAgent::TraeCode => "TraeCode",
            HookAgent::Copilot => "Copilot CLI",
            HookAgent::OpenCode => "OpenCode",
            HookAgent::Pi => "Pi",
            HookAgent::Grok => "Grok Build",
            HookAgent::OhMyPi => "Oh My Pi",
            HookAgent::PrimeAgent => "Prime Agent",
            HookAgent::Antigravity => "Antigravity",
            HookAgent::Empryo => "Empryo",
            HookAgent::Jcode => "jcode",
            HookAgent::Muse => "Muse Code",
            HookAgent::Gemini => "Gemini",
            HookAgent::Droid => "Droid",
            HookAgent::Qwen => "Qwen Code",
            HookAgent::Goose => "Goose",
            HookAgent::Kimi => "Kimi Code",
            HookAgent::QoderCLI => "Qoder CLI",
            HookAgent::Crush => "Crush",
            HookAgent::CodeBuddy => "CodeBuddy",
            HookAgent::Cursor => "Cursor CLI",
            HookAgent::QoderCLICn => "Qoder CN CLI",
        }
    }

    pub fn target_display(self, target: &HookTarget) -> String {
        target.abbreviate_home(&self.target_path(target))
    }

    pub(super) fn target_path(self, target: &HookTarget) -> PathBuf {
        match self {
            HookAgent::Claude => target.claude_settings_path(),
            HookAgent::Codex => target.codex_hooks_path(),
            HookAgent::TraeCode => target.traecli_hooks_path(),
            HookAgent::Copilot => target.under_home(&[".copilot", "hooks", OWNED_FILE_STEM_JSON]),
            HookAgent::OpenCode => target.under(
                &target.xdg_config_dir(),
                &["opencode", "plugins", OWNED_FILE_STEM_JS],
            ),
            HookAgent::Pi => target.under_home(&[".pi", "agent", "extensions", "anterm", "index.ts"]),
            HookAgent::PrimeAgent => {
                target.under_home(&[".prime", "agent", "extensions", "anterm", "index.ts"])
            }
            HookAgent::Antigravity => target.under_home(&[".gemini", "config", "hooks.json"]),
            HookAgent::Empryo => target.under_home(&[".empryo", "hooks.json"]),
            HookAgent::Jcode => target.jcode_config_path(),
            HookAgent::Muse => target.under_home(&[
                ".local",
                "share",
                "anterm",
                "agent-hooks",
                "muse",
                ".muse-plugin",
                "plugin.json",
            ]),
            HookAgent::Grok => target.under_home(&[".grok", "hooks", OWNED_FILE_STEM_JSON]),
            HookAgent::OhMyPi => {
                target.under_home(&[".omp", "agent", "extensions", "anterm", "index.ts"])
            }
            HookAgent::Gemini => target.gemini_settings_path(),
            HookAgent::Droid => target.under_home(&[".factory", "settings.json"]),
            HookAgent::Qwen => target.under_home(&[".qwen", "settings.json"]),
            // The Open Plugins layout, which Goose implements rather than
            // inventing its own: any `.agents/plugins/<name>/hooks/hooks.json`
            // is picked up at startup.
            HookAgent::Goose => {
                target.under_home(&[".agents", "plugins", "anterm", "hooks", "hooks.json"])
            }
            HookAgent::Kimi => target.kimi_config_path(),
            HookAgent::QoderCLI => target.qoder_settings_path(),
            HookAgent::QoderCLICn => target.qoder_cn_settings_path(),
            HookAgent::Crush => target.crush_settings_path(),
            HookAgent::CodeBuddy => target.codebuddy_settings_path(),
            // The user-level file; Cursor merges it under any project one.
            HookAgent::Cursor => target.under_home(&[".cursor", "hooks.json"]),
        }
    }

    pub(super) fn marker(self) -> String {
        format!("agent-hook {}", self.slug())
    }
}

pub(super) const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

pub struct HookTarget<'a> {
    pub(super) host: &'a dyn Host,
    pub(super) home: PathBuf,
    pub(super) exe: PathBuf,
}

impl<'a> HookTarget<'a> {
    pub fn local(host: &'a dyn Host) -> Option<HookTarget<'a>> {
        Self::local_for_exe(host, std::env::current_exe().ok()?)
    }

    /// Build a local target for a known anterm hook runner.
    ///
    /// Most callers use [`Self::local`]. The standalone CLI is the exception:
    /// it diagnoses hooks but never executes them, so it supplies the
    /// `anterm-app` it would launch instead of comparing configs to `anterm`.
    pub fn local_for_exe(host: &'a dyn Host, exe: PathBuf) -> Option<HookTarget<'a>> {
        Some(HookTarget {
            host,
            home: home_dir()?,
            exe,
        })
    }

    pub fn remote(host: &'a dyn Host, home: PathBuf) -> HookTarget<'a> {
        use crate::daemon::install::asset;
        let dialect = crate::daemon::install::RemoteProtocol::of_this_build();
        let home_str = home.to_string_lossy();
        // A Windows server reports its home natively (`C:\Users\me`), and the
        // hook command runs there, so the binary is named the way that
        // machine's installer put it — and spelled natively again.
        let binary = match asset::sftp_path_from_windows(&home_str) {
            Some(sftp_home) => {
                let installed = asset::remote_paths_on(
                    asset::RemotePlatform::Windows,
                    &sftp_home,
                    dialect.control,
                    dialect.protocol,
                )
                .binary;
                asset::windows_native_path(&installed).unwrap_or(installed)
            }
            None => asset::remote_paths(&home_str, dialect.control, dialect.protocol).binary,
        };
        HookTarget {
            host,
            home,
            exe: PathBuf::from(binary),
        }
    }

    pub(super) fn is_local(&self) -> bool {
        self.host.id().is_local()
    }

    pub(super) fn under(&self, base: &Path, parts: &[&str]) -> PathBuf {
        let mut p = base.to_path_buf();
        for part in parts {
            p = self.host.join(&p, part);
        }
        p
    }

    fn under_home(&self, parts: &[&str]) -> PathBuf {
        self.under(&self.home, parts)
    }

    fn claude_settings_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("settings.json");
        }
        self.under_home(&[".claude", "settings.json"])
    }

    /// Codex keeps its whole home, `hooks.json` included, in `CODEX_HOME`
    /// when that is set. Local-only, like the other overrides.
    fn codex_hooks_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("hooks.json");
        }
        self.under_home(&[".codex", "hooks.json"])
    }

    /// Gemini CLI roots its user-level `.gemini` directory, settings included,
    /// at `GEMINI_CLI_HOME` in place of the user's home when that is set.
    /// Local-only, like the other overrides.
    fn gemini_settings_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("GEMINI_CLI_HOME").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join(".gemini").join("settings.json");
        }
        self.under_home(&[".gemini", "settings.json"])
    }

    fn xdg_config_dir(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir);
        }
        self.under_home(&[".config"])
    }

    fn jcode_config_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("JCODE_HOME").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("config.toml");
        }
        self.under_home(&[".jcode", "config.toml"])
    }

    fn kimi_config_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("KIMI_CODE_HOME").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("config.toml");
        }
        self.under_home(&[".kimi-code", "config.toml"])
    }

    fn qoder_settings_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("QODER_CONFIG_DIR").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("settings.json");
        }
        self.under_home(&[".qoder", "settings.json"])
    }

    /// The China build keeps its settings in `~/.qoder-cn`. It resolves the
    /// directory the same way the global build does, under a `-cn` name:
    /// `QODERCN_CONFIG_DIR` wins, otherwise the home falls back through
    /// `QODERCN_CLI_HOME` and `GEMINI_CLI_HOME` before the user's own, with
    /// `.qoder-cn` as the directory name. Only the first is honoured here —
    /// local-only, like every other override, so a local env var must not
    /// redirect a remote machine's hooks.
    fn qoder_cn_settings_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("QODERCN_CONFIG_DIR").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("settings.json");
        }
        self.under_home(&[".qoder-cn", "settings.json"])
    }

    /// The global `crush.json`. Crush resolves it through `CRUSH_GLOBAL_CONFIG`
    /// when set, otherwise `$XDG_CONFIG_HOME/crush/crush.json` (and `~/.config`
    /// when that is unset) — which is exactly what [`Self::xdg_config_dir`]
    /// answers. The override is local-only: a local env var must not redirect a
    /// remote machine's hooks.
    pub(super) fn crush_settings_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("CRUSH_GLOBAL_CONFIG").filter(|d| !d.is_empty())
        {
            return PathBuf::from(dir).join("crush.json");
        }
        self.under(&self.xdg_config_dir(), &["crush", "crush.json"])
    }

    /// CodeBuddy moves its whole home, `settings.json` included, to
    /// `CODEBUDDY_CONFIG_DIR` when that is set and not blank. Local-only, like
    /// the other overrides.
    fn codebuddy_settings_path(&self) -> PathBuf {
        if self.is_local()
            && let Some(dir) = std::env::var_os("CODEBUDDY_CONFIG_DIR")
                .filter(|d| !d.to_string_lossy().trim().is_empty())
        {
            return PathBuf::from(dir).join("settings.json");
        }
        self.under_home(&[".codebuddy", "settings.json"])
    }

    fn traecli_hooks_path(&self) -> PathBuf {
        if self.is_local() {
            if let Some(dir) = std::env::var_os("TRAECLI_HOME").filter(|d| !d.is_empty()) {
                return PathBuf::from(dir).join("hooks.json");
            }
            if let Some(dir) = std::env::var_os("TRAE_HOME").filter(|d| !d.is_empty()) {
                return PathBuf::from(dir).join("cli").join("hooks.json");
            }
        }
        self.under_home(&[".trae", "cli", "hooks.json"])
    }

    pub(super) fn hook_command(&self, agent: HookAgent, event: &str) -> String {
        if let Some(exe) = self.hook_command_exe() {
            return format!("{exe} agent-hook {} {event}", agent.slug());
        }
        format!(
            "\"{}\" agent-hook {} {event}",
            self.exe.display(),
            agent.slug()
        )
    }

    /// A shell-safe executable path for generated hook commands.
    ///
    /// On Windows, Codex runs hook commands through the session's shell, which
    /// is frequently PowerShell. `pwsh -Command` drops the quotes around a
    /// path containing spaces, so the usual `"C:\Program Files\..."` form is
    /// parsed as `C:\Program` and fails — and even a quoted path without
    /// spaces is a syntax error in PowerShell (invoking a quoted path requires
    /// the `&` call operator). When the executable resolves by its bare file
    /// name from PATH, we can emit the name without any quoting, which every
    /// shell (`cmd.exe`, PowerShell, bash) executes correctly.
    ///
    /// Returns `None` when the executable is not resolvable by name from PATH,
    /// or when the first PATH match is a different binary; callers then fall
    /// back to the quoted full path.
    pub(super) fn hook_command_exe(&self) -> Option<String> {
        #[cfg(windows)]
        if self.is_local() {
            if let Some(name) = path_resolvable_name(&self.exe) {
                return Some(name);
            }
        }
        None
    }

    pub(super) fn read(&self, p: &Path) -> io::Result<String> {
        let bytes = self.host.read_file(p, MAX_CONFIG_BYTES)?;
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub(super) fn write(&self, p: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(parent) = p.parent() {
            self.host.create_dir(parent, true)?;
        }
        if self.is_local() {
            crate::config::write_atomic(p, bytes)?;
        } else {
            self.host.write_file(p, bytes)?;
        }
        Ok(())
    }

    fn abbreviate_home(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HooksState {
    NotInstalled,
    Installed,
    Outdated,
}

pub fn hooks_state(target: &HookTarget, agent: HookAgent) -> HooksState {
    let path = agent.target_path(target);
    if agent == HookAgent::Muse {
        return muse_hooks_state(target);
    }
    if agent == HookAgent::Antigravity {
        return named_hook_set_state(target, &path, &antigravity_hook_set(target));
    }
    if agent == HookAgent::Jcode {
        return jcode_hooks_state(target, &path);
    }
    if let Some(events) = agent.toml_hook_events() {
        return toml_hooks_state(target, &path, agent, events);
    }
    if let Some(events) = agent.hook_map_events() {
        return hook_map_state(target, &path, agent, events);
    }
    let Some(expected) = owned_file_content(target, agent) else {
        return HooksState::NotInstalled;
    };
    owned_file_state(target, &path, &expected, &agent.marker())
}

/// What an install or uninstall actually did.
///
/// These land in a note the user reads in Settings, and this crate cannot
/// reach `src/ui/i18n`, which owns every user-visible string. Returning a
/// sentence from here meant a Chinese or Japanese UI reported "Installed" in
/// English; returning the outcome lets the caller word it in the user's
/// language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Installed,
    /// Installed on a remote, where `codex features enable hooks` still has to
    /// be run once by hand.
    InstalledEnableCodexThere,
    /// Bundle prepared on a remote; its own Muse CLI must install and approve it.
    MuseInstallManually(String),
    /// Installed, but running `codex features enable hooks` here failed.
    InstalledCodexEnableFailed(String),
    Removed,
    /// The agent has no config file at all.
    NothingInstalled,
    /// The config is there, but holds no anterm hooks.
    NoAntermHooks,
}

pub fn install_hooks(target: &HookTarget, agent: HookAgent) -> anyhow::Result<HookOutcome> {
    let path = agent.target_path(target);
    if agent == HookAgent::Muse {
        return muse_hooks_install(target, &path);
    }
    if agent == HookAgent::Antigravity {
        named_hook_set_install(target, &path, antigravity_hook_set(target))?;
        return Ok(HookOutcome::Installed);
    }
    if agent == HookAgent::Jcode {
        jcode_hooks_install(target, &path)?;
        return Ok(HookOutcome::Installed);
    }
    if let Some(events) = agent.toml_hook_events() {
        toml_hooks_install(target, &path, agent, events)?;
        return Ok(HookOutcome::Installed);
    }
    if let Some(events) = agent.hook_map_events() {
        hook_map_install(target, &path, agent, events)?;
        if agent != HookAgent::Codex {
            return Ok(HookOutcome::Installed);
        }
        if !target.is_local() {
            return Ok(HookOutcome::InstalledEnableCodexThere);
        }
        return Ok(match enable_codex_hooks_feature() {
            Ok(()) => HookOutcome::Installed,
            Err(e) => HookOutcome::InstalledCodexEnableFailed(e.to_string()),
        });
    }
    let content = owned_file_content(target, agent)
        .ok_or_else(|| anyhow::anyhow!("{agent:?} has no owned file"))?;
    owned_file_install(target, &path, &content, &agent.marker())?;
    Ok(HookOutcome::Installed)
}

pub fn uninstall_hooks(target: &HookTarget, agent: HookAgent) -> anyhow::Result<HookOutcome> {
    let path = agent.target_path(target);
    if agent == HookAgent::Muse {
        return muse_hooks_uninstall(target, &path);
    }
    if agent == HookAgent::Antigravity {
        return named_hook_set_uninstall(target, &path);
    }
    if agent == HookAgent::Jcode {
        return jcode_hooks_uninstall(target, &path);
    }
    if agent.toml_hook_events().is_some() {
        return toml_hooks_uninstall(target, &path, agent);
    }
    match agent.hook_map_events() {
        Some(_) => hook_map_uninstall(target, &path, agent),
        None => owned_file_uninstall(target, &path, &agent.marker()),
    }
}

pub fn refresh_hooks(target: &HookTarget) -> usize {
    let mut refreshed = 0;
    for agent in HookAgent::ALL {
        if hooks_state(target, agent) != HooksState::Outdated {
            continue;
        }
        match install_hooks(target, agent) {
            Ok(summary) => {
                refreshed += 1;
                log::info!(
                    "refreshed stale {} hooks at {}: {summary:?}",
                    agent.display_name(),
                    agent.target_display(target)
                );
            }
            Err(e) => log::warn!(
                "could not refresh stale {} hooks: {e}",
                agent.display_name()
            ),
        }
    }
    refreshed
}

pub fn refresh_remote_hooks(host: &dyn Host, home: PathBuf) -> usize {
    if home_dir().is_some_and(|ours| ours == home) {
        return 0;
    }
    refresh_hooks(&HookTarget::remote(host, home))
}

pub fn refresh_hooks_at_launch() -> usize {
    if cfg!(debug_assertions) {
        return 0;
    }
    let host = crate::host::local::LocalHost::new();
    let Some(target) = HookTarget::local(&*host) else {
        return 0;
    };
    refresh_hooks(&target)
}

pub(super) fn home_dir() -> Option<PathBuf> {
    crate::util::home_dir()
}

/// Returns the bare file name of `exe` when resolving that name from PATH
/// yields the same binary. Returns `None` when the name does not resolve, or
/// when an earlier PATH entry contains a different file with the same name
/// (a bare-name command would then invoke the wrong binary).
#[cfg(windows)]
fn path_resolvable_name(exe: &Path) -> Option<String> {
    let name = exe.file_name()?.to_str()?;
    let path_var = std::env::var_os("PATH")?;
    let exe_canonical = std::fs::canonicalize(exe).ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        let Ok(candidate_canonical) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        if same_windows_path(&candidate_canonical, &exe_canonical) {
            return Some(name.to_string());
        }
        return None;
    }
    None
}

#[cfg(windows)]
fn same_windows_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}
