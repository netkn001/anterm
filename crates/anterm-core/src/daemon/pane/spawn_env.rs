use std::path::PathBuf;

use portable_pty::CommandBuilder;

use crate::daemon::protocol::{RemoteContext, RemoteKind, ShellSpec};
use crate::daemon::shell_integration;

#[cfg(windows)]
fn default_prog() -> CommandBuilder {
    CommandBuilder::new(crate::shells::windows_default_shell())
}

#[cfg(not(windows))]
fn default_prog() -> CommandBuilder {
    default_prog_with_override(detected_shell_override())
}

#[cfg(not(windows))]
pub(super) fn default_prog_with_override(shell_override: Option<String>) -> CommandBuilder {
    let cmd = CommandBuilder::new_default_prog();
    let portable_shell = cmd.get_shell();
    if let Some(shell) = shell_override.filter(|shell| shell != &portable_shell) {
        return CommandBuilder::new(&shell);
    }
    cmd
}

#[cfg(not(windows))]
fn detected_shell_override() -> Option<String> {
    let path = std::env::var_os(crate::daemon::DETECTED_SHELL_ENV)?;
    usable_shell_path(path)
}

#[cfg(not(windows))]
fn usable_shell_path(path: std::ffi::OsString) -> Option<String> {
    let path = PathBuf::from(path);
    if path.as_os_str().is_empty() || !path.is_file() {
        return None;
    }
    path.into_os_string().into_string().ok()
}

#[cfg(windows)]
pub(super) fn default_shell_name(_cmd: &CommandBuilder) -> String {
    crate::shells::windows_default_shell().to_string()
}

/// The shell `cmd` will actually run. `get_shell` names the login shell
/// whatever `cmd` holds, and the detected-shell override swaps the program
/// for another one: anterm launched from bash with zsh as the login shell ran
/// bash with zsh's integration, which is none at all.
#[cfg(not(windows))]
pub(super) fn default_shell_name(cmd: &CommandBuilder) -> String {
    match cmd.get_argv().first() {
        Some(program) if !cmd.is_default_prog() => program.to_string_lossy().into_owned(),
        _ => cmd.get_shell(),
    }
}

#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(super) struct ChosenShell {
    pub(super) program: String,
    pub(super) args: Vec<String>,
    pub(super) args_are_anterm_defaults: bool,
}

pub(super) fn has_custom_args(chosen: Option<&ChosenShell>) -> bool {
    chosen.is_some_and(|c| !c.args.is_empty() && !c.args_are_anterm_defaults)
}

pub(super) fn choose_shell(
    spawn_override: Option<ShellSpec>,
    configured: Option<(String, Vec<String>)>,
) -> Option<ChosenShell> {
    spawn_override
        .map(|s| ChosenShell {
            program: s.program,
            args: s.args,
            args_are_anterm_defaults: s.args_are_anterm_defaults,
        })
        .or_else(|| {
            configured.map(|(program, args)| ChosenShell {
                program,
                args,
                args_are_anterm_defaults: false,
            })
        })
}

pub(super) fn apply_shell_integration(
    cmd: &mut CommandBuilder,
    resolved_program: &str,
    integration: &shell_integration::Injection,
) {
    if integration.replaces_argv || (cmd.is_default_prog() && !integration.args.is_empty()) {
        *cmd = CommandBuilder::new(resolved_program);
    }
    cmd.args(&integration.args);
    for (k, v) in &integration.env {
        cmd.env(k, v);
    }
}

pub(super) struct SpawnConfig {
    pub(super) cmd: CommandBuilder,
    pub(super) initial_cwd: Option<PathBuf>,
    pub(super) integration_dir: Option<PathBuf>,
    pub(super) remote: Option<RemoteContext>,
    /// The shell this pane actually got, after the override and the config have
    /// been resolved against each other. Recorded so the machine tree can name
    /// it — see [`crate::machine::PaneRecord::shell`].
    pub(super) shell: Option<ShellSpec>,
}

/// Why a configured shell cannot be run, in one sentence, before anything
/// tries. Without it a missing program arrives at the window wrapped four
/// deep — "daemon refused Spawn: spawn failed: Unable to spawn … (ENOENT: No
/// such file or directory)" — and the one fact that matters is buried in it.
///
/// Only a program given as a path can be checked here; a bare name is resolved
/// through PATH by the OS, and guessing at that would be worse than silence.
pub(super) fn shell_program_problem(program: &str) -> Option<String> {
    let path = std::path::Path::new(program);
    if path.components().count() < 2 {
        return None;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return Some(format!("no such shell on this machine: {program}"));
    };
    if meta.is_dir() {
        return Some(format!("the configured shell is a directory: {program}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Some(format!("the configured shell is not executable: {program}"));
        }
    }
    None
}

pub(super) fn build_spawn_config(
    pane: u64,
    cwd: Option<PathBuf>,
    shell: Option<ShellSpec>,
) -> anyhow::Result<SpawnConfig> {
    let initial_cwd = initial_working_directory(cwd);
    let configured = choose_shell(shell, crate::config::shell_command());
    if let Some(problem) = configured
        .as_ref()
        .and_then(|c| shell_program_problem(&c.program))
    {
        anyhow::bail!(problem);
    }
    let remote = wsl_remote_context(configured.as_ref());
    // Taken before the chosen shell is consumed by the command builder: what
    // goes in the tree is what was resolved here, not the possibly-empty
    // override the caller sent.
    let shell = configured.as_ref().map(|c| ShellSpec {
        program: c.program.clone(),
        args: c.args.clone(),
        args_are_anterm_defaults: c.args_are_anterm_defaults,
    });
    let (cmd, integration_dir) = build_shell_command(configured, &initial_cwd, pane)?;
    Ok(SpawnConfig {
        cmd,
        initial_cwd,
        integration_dir,
        remote,
        shell,
    })
}

pub(super) fn wsl_remote_context(shell: Option<&ChosenShell>) -> Option<RemoteContext> {
    if !cfg!(windows) {
        return None;
    }
    let chosen = shell?;
    let base = std::path::Path::new(&chosen.program)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    if base.strip_suffix(".exe").unwrap_or(&base) != "wsl" {
        return None;
    }
    Some(RemoteContext {
        kind: RemoteKind::Wsl,
        argv: Vec::new(),
        // No `--distribution` means wsl.exe launches the default distro —
        // name it here, so every consumer of the context (the `\\wsl$`
        // completion route, paste-path rewriting, host labels) gets a real
        // distro instead of an empty placeholder.
        target: shell_integration::wsl_distro(&chosen.args)
            .or_else(crate::shells::default_wsl_distro)
            .unwrap_or_default(),
    })
}

pub(super) fn build_shell_command(
    configured: Option<ChosenShell>,
    initial_cwd: &Option<PathBuf>,
    pane: u64,
) -> anyhow::Result<(CommandBuilder, Option<PathBuf>)> {
    let mut cmd = match &configured {
        Some(chosen) => {
            let mut c = CommandBuilder::new(&chosen.program);
            c.args(&chosen.args);
            c
        }
        None => default_prog(),
    };
    let resolved_program = match &configured {
        Some(chosen) => chosen.program.clone(),
        None => default_shell_name(&cmd),
    };

    let integration = shell_integration::setup(
        Some(&resolved_program),
        configured.as_ref().map_or(&[][..], |c| c.args.as_slice()),
        has_custom_args(configured.as_ref()),
    );
    if let Some(integration) = &integration {
        apply_shell_integration(&mut cmd, &resolved_program, integration);
    }
    let integration_dir = integration.as_ref().and_then(|i| i.dir.clone());
    let launched = launched_shell_program(&cmd, &resolved_program);
    apply_common_command_setup(&mut cmd, initial_cwd, pane, &launched);
    Ok((cmd, integration_dir))
}

/// The program the pane is really about to exec.
///
/// `resolved_program` is what anterm *decided* to run before shell integration
/// had its say, and for a default-prog builder that decision is
/// `CommandBuilder::get_shell()` — the passwd entry — even when
/// `default_prog()` swapped the builder for the shell that launched the daemon.
/// argv survives every rewrite on the way here, including the one an
/// argv-replacing injection performs, so it is the honest answer. An empty argv
/// means the builder is still the untouched default prog, which is precisely
/// the case `resolved_program` already describes.
pub(super) fn launched_shell_program(cmd: &CommandBuilder, resolved_program: &str) -> String {
    cmd.get_argv()
        .first()
        .map(|argv0| argv0.to_string_lossy().into_owned())
        .unwrap_or_else(|| resolved_program.to_string())
}

pub(super) fn initial_working_directory(cwd: Option<PathBuf>) -> Option<PathBuf> {
    let fallback = std::env::current_dir()
        .ok()
        .filter(|d| d != std::path::Path::new("/"))
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from));
    let forced = crate::config::working_directory_base();
    // A configured "Start in" path that is not a directory can never win the
    // pick below, so every new pane silently lands on the fallback and the
    // mistyped path reads as a anterm bug (#601). Settings refuses to save one
    // now, but a hand-edited config.json can still hold one — name it, with
    // the reason, at the moment it actually costs something (an explicit cwd
    // that resolves never consults the config, so that case stays quiet).
    if let Some(dir) = &forced {
        if !dir.is_dir() && cwd.as_ref().is_none_or(|d| !d.is_dir()) {
            log::warn!(
                "configured working directory {} is not a directory; new panes fall back",
                dir.display()
            );
        }
    }
    [cwd, forced, fallback]
        .into_iter()
        .flatten()
        .find(|d| d.is_dir())
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn locale_fallback_is_needed(
    extra_env: &std::collections::HashMap<String, String>,
    mut inherited: impl FnMut(&str) -> Option<String>,
) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"].iter().all(|key| {
        !extra_env.contains_key(*key) && inherited(key).as_deref().is_none_or(str::is_empty)
    })
}

#[cfg(target_os = "macos")]
pub(super) const LOCALE_DEFINITION_DIR: &str = "/usr/share/locale";

#[cfg(any(target_os = "macos", test))]
const FALLBACK_CHARACTER_LOCALES: [&str; 2] = ["C.UTF-8", "en_US.UTF-8"];

/// The variable the locale fallback sets.
///
/// It has to be one a shell consults for *every* category. `LC_CTYPE` alone
/// fixes character handling and leaves collation, time and numbers at `C`; a
/// shell that then asks `setlocale(LC_COLLATE, "")` finds no variable to read
/// and bash warns `setlocale: LC_COLLATE: cannot change locale ()` once per
/// category. (zsh and fish swallow the failure, so they look fine while being
/// just as half-configured.) `LC_ALL` would also cover everything, but it wins
/// over every `LC_*` the user's own rc files set afterwards — `LANG` loses to
/// them, which is what a fallback should do.
#[cfg(any(target_os = "macos", test))]
pub(super) const LOCALE_FALLBACK_KEY: &str = "LANG";

#[cfg(any(target_os = "macos", test))]
pub(super) fn character_locale(identifier: Option<&str>, exists: impl Fn(&str) -> bool) -> Option<String> {
    identifier
        .and_then(posix_locale_stem)
        .map(|stem| format!("{stem}.UTF-8"))
        .into_iter()
        .chain(FALLBACK_CHARACTER_LOCALES.iter().map(|s| (*s).to_string()))
        .find(|candidate| exists(candidate))
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn posix_locale_stem(identifier: &str) -> Option<String> {
    let base = identifier.split('@').next()?;
    let mut parts = base.split(['_', '-']).filter(|s| !s.is_empty());
    let language = parts
        .next()
        .filter(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphabetic()))?;
    let region = parts.next_back().filter(|r| {
        (r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic()))
            || (r.len() == 3 && r.chars().all(|c| c.is_ascii_digit()))
    })?;
    Some(format!(
        "{}_{}",
        language.to_ascii_lowercase(),
        region.to_ascii_uppercase()
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn system_locale_identifier() -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use std::os::raw::c_void;

    type CFLocaleRef = *const c_void;
    unsafe extern "C" {
        fn CFLocaleCopyCurrent() -> CFLocaleRef;
        fn CFLocaleGetIdentifier(locale: CFLocaleRef) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    unsafe {
        let locale = CFLocaleCopyCurrent();
        if locale.is_null() {
            return None;
        }
        let identifier = CFLocaleGetIdentifier(locale);
        let out =
            (!identifier.is_null()).then(|| CFString::wrap_under_get_rule(identifier).to_string());
        CFRelease(locale);
        out
    }
}

const TERM_PROGRAM_NAME: &str = "anterm";

/// The config dir this server runs on, not a socket path.
///
/// A client needs *two* endpoints — control and pane — and they are derived
/// from the config dir by rules that already live in this crate
/// (`control_socket_path`, `transport::socket_path_for`), including a hashed
/// fallback when the directory is too long for `sun_path`. Publishing one
/// socket path and letting the client reconstruct the other from it means a
/// second, independent derivation that can and did disagree with the first.
/// Publishing the directory instead means a CLI in this shell resolves both
/// endpoints with the very same functions the server used to open them.
pub(super) const ANTERM_CONFIG_DIR_ENV: &str = "ANTERM_CONFIG_DIR";
pub(crate) const ANTERM_PANE_ENV: &str = "ANTERM_PANE";

pub(super) fn config_dir_env() -> Option<String> {
    crate::config::config_dir_path().map(|p| p.display().to_string())
}

/// The shell the pane is running, not the one the passwd entry names.
///
/// Everything that spawns "the user's shell" — tmux's `default-shell`, `sudo
/// -s`, an editor's `!` escape, a coding agent picking a quoting dialect —
/// reads `$SHELL`. A GUI launch inherits the login session's snapshot of it,
/// so a pane told to run fish still advertised `/bin/zsh` and every one of
/// those consumers walked off into the wrong shell (#342). Inside a pane, the
/// shell anterm was told to run *is* the user's shell, so that is what we name.
///
/// Note this is the opposite direction from `shells::login_shell()`, which
/// deliberately ignores `$SHELL` in favour of passwd: that answers "which
/// shell did the user log in with", which is still the login shell. This
/// answers "which shell is this pane", which is not the same question.
pub(super) const SHELL_ENV: &str = "SHELL";

/// The absolute path to name in `$SHELL`, or `None` when there is none to
/// promise.
///
/// `$SHELL` is a POSIX contract, so the path rules here are POSIX ones rather
/// than the host's — this only ever describes a pane on a unix daemon (see the
/// caller), and hard-coding them keeps the resolution testable on any host.
///
/// A configured command may be bare (`fish`), because a bare command is what
/// the shell inventory keeps when it wants PATH to decide which install wins.
/// Consumers of `$SHELL` exec it directly, and many of them do so with a PATH
/// of their own, so a bare name there is not the same binary the pane runs.
/// Resolve it the way exec would, against the PATH the pane will inherit, and
/// leave `$SHELL` untouched when that finds nothing — a stale but honest login
/// shell beats a name that resolves somewhere else.
pub(super) fn shell_env_path(
    program: &str,
    path_var: impl FnOnce() -> Option<String>,
    is_file: impl Fn(&str) -> bool,
) -> Option<String> {
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    if program.starts_with('/') {
        return Some(program.to_string());
    }
    // Anything else with a separator is relative to a working directory, and
    // the reader's is not necessarily the pane's. Only a bare name is worth
    // resolving.
    if program.contains('/') {
        return None;
    }
    path_var()?
        .split(':')
        .filter(|dir| dir.starts_with('/'))
        .map(|dir| format!("{}/{program}", dir.trim_end_matches('/')))
        .find(|candidate| is_file(candidate))
}

const CAPABILITY_ENV: [&str; 2] = ["TERM", "COLORTERM"];

fn names_capability_env(key: &str) -> bool {
    CAPABILITY_ENV.iter().any(|cap| {
        if cfg!(windows) {
            key.eq_ignore_ascii_case(cap)
        } else {
            key == *cap
        }
    })
}

pub(super) fn pane_environment(
    extra_env: &std::collections::HashMap<String, String>,
    // Only Windows has a use for it — see the `COLORFGBG` block below.
    #[cfg_attr(not(windows), allow(unused_variables))] dark: bool,
    pane: u64,
    shell: &str,
) -> Vec<(String, String)> {
    let version = env!("CARGO_PKG_VERSION");
    let mut env = vec![
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
        (
            crate::agent_hooks::ANTERM_ENV_MARKER.to_string(),
            version.to_string(),
        ),
        ("TERM_PROGRAM".to_string(), TERM_PROGRAM_NAME.to_string()),
        ("TERM_PROGRAM_VERSION".to_string(), version.to_string()),
        // anterm renders OSC 8 links, but `supports-hyperlinks` (Claude Code,
        // most Node CLIs) only trusts a fixed TERM_PROGRAM list and strips
        // them otherwise; this is the override it honours. It is checked
        // before the isTTY test, so such a tool now writes OSC 8 into
        // `> file` and `| less` too. `FORCE_HYPERLINK=0` in the `env` config
        // or the shell's rc turns it back off.
        ("FORCE_HYPERLINK".to_string(), "1".to_string()),
        (ANTERM_PANE_ENV.to_string(), pane.to_string()),
    ];
    #[cfg(windows)]
    if !extra_env
        .keys()
        .any(|key| key.eq_ignore_ascii_case("COLORFGBG"))
    {
        // The bundled ConPTY forwards OSC 11 queries, so this is no longer the
        // only answer a Windows pane gets — but the in-box conhost still
        // swallows them, and that is what a build without `conpty.dll` beside
        // it runs on. Keep the conventional fallback hint for that case, and
        // for applications that never learned to ask. An explicit user override
        // still wins, below.
        let colorfgbg = if dark { "15;0" } else { "0;15" };
        env.push(("COLORFGBG".to_string(), colorfgbg.to_string()));
    }
    if let Some(dir) = config_dir_env() {
        env.push((ANTERM_CONFIG_DIR_ENV.to_string(), dir));
    }
    // Names the pane's own history file; the integration snippet is what
    // decides to use it, once the user's rc has said where their history was.
    if let Some((key, value)) = crate::daemon::history::env_for(pane, shell) {
        env.push((key, value));
    }
    // Windows is deliberately left out. `SHELL` is not a native concept there —
    // neither cmd nor PowerShell reads it — and the tools that do read it are
    // the POSIX emulations: MSYS/Git Bash, Cygwin, and WSL, which take `WSLENV`
    // exports as Linux paths. All of them want a POSIX path, and every shell
    // this branch could name is a Windows one, so setting it would point them
    // at something they cannot exec. That also covers the WSL pane, whose
    // program is `wsl.exe` rather than a shell at all: the distro's own login
    // shell is the right answer inside it, and leaving `SHELL` alone is how
    // that answer survives.
    //
    // Resolution runs against the PATH the pane will actually inherit, so a
    // user who redirects PATH in their `env` block gets the binary that PATH
    // resolves rather than the daemon's.
    if !cfg!(windows)
        && let Some(path) = shell_env_path(
            shell,
            || {
                extra_env
                    .get("PATH")
                    .cloned()
                    .or_else(|| std::env::var("PATH").ok())
            },
            |candidate| std::path::Path::new(candidate).is_file(),
        )
    {
        env.push((SHELL_ENV.to_string(), path));
    }
    env.extend(
        extra_env
            .iter()
            .filter(|(k, _)| !names_capability_env(k))
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    env
}

/// zsh integration works by pointing `ZDOTDIR` at anterm's redirector dir, which
/// then sources the user's own startup files from `ANTERM_USER_ZDOTDIR`. A
/// `ZDOTDIR` in the configured `env` block is applied after that and used to
/// win outright: zsh read the user's files directly and the pane came up with
/// no prompt marks, no editor, no suggestions. When the redirector is in
/// place, the configured value is where the user's files live, so it goes
/// where the redirector looks for them instead.
pub(super) fn reroute_configured_zdotdir(
    env: Vec<(String, String)>,
    zsh_integration: bool,
) -> Vec<(String, String)> {
    if !zsh_integration {
        return env;
    }
    env.into_iter()
        .map(|(k, v)| {
            if k == "ZDOTDIR" {
                ("ANTERM_USER_ZDOTDIR".to_string(), v)
            } else {
                (k, v)
            }
        })
        .collect()
}

pub(super) fn apply_common_command_setup(
    cmd: &mut CommandBuilder,
    initial_cwd: &Option<PathBuf>,
    pane: u64,
    shell: &str,
) {
    if let Some(dir) = initial_cwd {
        cmd.cwd(dir);
        // A new pane inherits the directory the last one *reported*, which is
        // the logical path its shell was showing — `/tmp/x`, not the
        // `/private/tmp/x` the kernel resolves it to. Handing that to `cwd()`
        // alone loses the distinction: the child starts in the resolved
        // directory and its shell falls back to `getcwd()`, so one tab read
        // `/tmp/x` while every tab opened from it read `/private/tmp/x` — the
        // same directory under two names, side by side in the sidebar.
        //
        // `PWD` is how a shell is told which of a directory's names it arrived
        // by; it is what `cd` sets, and what Terminal.app and iTerm2 pass for
        // this reason. POSIX has the shell verify it — a `PWD` that does not
        // name the directory the process is really in is discarded, confirmed
        // here against zsh, bash, sh and fish — so this can correct the name
        // and cannot invent one.
        //
        // Unix only: a Windows shell that reads `PWD` at all wants it in its
        // own idiom (`/c/x` under Git Bash), so a native path would just be
        // discarded by the same check.
        #[cfg(unix)]
        cmd.env("PWD", dir);
    }
    // The AppImage runtime exports `ARGV0` (the path the image was launched
    // by) into anterm's own environment, and the pane would inherit it. zsh
    // treats an exported `ARGV0` as the argv[0] of every external command it
    // runs, so each program in the pane is told it is `anterm-….AppImage` — and
    // multiplexers like Volta's shims, which pick the tool from argv[0], fail
    // with "Could not find executable" (#1100). Nothing outside a single
    // command line ever wants it set, so the pane starts without it; the
    // configured `env` block, applied below, can still put it back.
    cmd.env_remove("ARGV0");

    let extra_env = crate::config::extra_env();

    // Windows hands every process a private copy of the environment at spawn
    // time and never updates it, so a daemon that has been running since before
    // an installer touched `HKCU\Environment` would give a brand-new pane its
    // startup `PATH` (#333). Re-read both hives here and pin the merge onto the
    // command; the pane-specific variables below (and the configured overrides
    // they carry) are applied afterwards and still win.
    #[cfg(windows)]
    for (k, v) in crate::daemon::windows_env::refreshed_pane_environment(&extra_env) {
        cmd.env(k, v);
    }

    let dark = crate::machine::appearance().dark;
    let injected_zdotdir = cmd
        .get_env("ZDOTDIR")
        .and_then(|z| z.to_str())
        .filter(|z| shell_integration::is_our_zdotdir(z))
        .map(str::to_owned);
    for (k, v) in reroute_configured_zdotdir(
        pane_environment(&extra_env, dark, pane, shell),
        injected_zdotdir.is_some(),
    ) {
        cmd.env(k, v);
    }

    #[cfg(target_os = "macos")]
    if locale_fallback_is_needed(&extra_env, |key| std::env::var(key).ok())
        && let Some(locale) = character_locale(system_locale_identifier().as_deref(), |name| {
            std::path::Path::new(LOCALE_DEFINITION_DIR)
                .join(name)
                .is_dir()
        })
    {
        cmd.env(LOCALE_FALLBACK_KEY, locale);
    }
}
