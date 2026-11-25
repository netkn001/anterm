use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    BASH_INTEGRATION, FISH_INTEGRATION, NUSHELL_INTEGRATION, POWERSHELL_INTEGRATION,
    ZSH_INTEGRATION,
};

pub(super) fn zsh_redirectors() -> [(&'static str, String); 4] {
    let redirect = |name: &str, tail: &str| {
        format!(
            "__anterm_ztmp=$ZDOTDIR\n\
             if [[ -n \"$ANTERM_USER_ZDOTDIR\" ]]; then ZDOTDIR=$ANTERM_USER_ZDOTDIR; else unset ZDOTDIR; fi\n\
             [[ \"$HISTFILE\" == \"$__anterm_ztmp/\"* ]] && HISTFILE=\"${{ZDOTDIR:-$HOME}}/${{HISTFILE#$__anterm_ztmp/}}\"\n\
             [[ -f \"${{ZDOTDIR:-$HOME}}/{name}\" ]] && source \"${{ZDOTDIR:-$HOME}}/{name}\"\n\
             {tail}ZDOTDIR=$__anterm_ztmp\n\
             unset __anterm_ztmp\n"
        )
    };
    [
        (
            ".zshenv",
            redirect(".zshenv", "export ANTERM_USER_ZDOTDIR=${ZDOTDIR:-$HOME}\n"),
        ),
        (".zprofile", redirect(".zprofile", "")),
        (
            ".zshrc",
            format!("{}{ZSH_INTEGRATION}", redirect(".zshrc", "")),
        ),
        (".zlogin", redirect(".zlogin", "")),
    ]
}

pub struct Injection {
    pub env: HashMap<String, String>,
    pub args: Vec<String>,
    pub replaces_argv: bool,
    pub dir: Option<PathBuf>,
}

const ZDOTDIR_PREFIX: &str = "anterm-zdotdir-";

pub(crate) fn is_our_zdotdir(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(ZDOTDIR_PREFIX))
}

fn real_user_zdotdir() -> Option<String> {
    if let Ok(z) = std::env::var("ANTERM_USER_ZDOTDIR") {
        if !z.is_empty() {
            return Some(z);
        }
    }
    std::env::var("ZDOTDIR")
        .ok()
        .filter(|z| !z.is_empty() && !is_our_zdotdir(z))
}

pub(super) enum ShellKind {
    Zsh,
    Bash,
    Fish,
    PowerShell,
    Nushell,
    Wsl,
}

pub(super) fn shell_kind(program: Option<&str>) -> Option<ShellKind> {
    let owned = match program {
        Some(p) => p.to_string(),
        None => crate::shells::login_shell(),
    };
    let base = Path::new(&owned)
        .file_name()?
        .to_str()?
        .to_ascii_lowercase();
    match base.strip_suffix(".exe").unwrap_or(&base) {
        "zsh" => Some(ShellKind::Zsh),
        "bash" if cfg!(windows) && !is_msys_bash(&owned) => None,
        "bash" => Some(ShellKind::Bash),
        "fish" => Some(ShellKind::Fish),
        "powershell" | "pwsh" => Some(ShellKind::PowerShell),
        "nu" => Some(ShellKind::Nushell),
        "wsl" => Some(ShellKind::Wsl),
        _ => None,
    }
}

pub(crate) fn wsl_distro(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--distribution" || a == "-d" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--distribution=") {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn wslenv_with(existing: Option<&str>, additions: &[&str]) -> String {
    let mut out: Vec<String> = existing
        .unwrap_or("")
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    for add in additions {
        let name = add.split('/').next().unwrap_or(add);
        if !out.iter().any(|e| e.split('/').next().unwrap_or(e) == name) {
            out.push((*add).to_string());
        }
    }
    out.join(":")
}

fn is_msys_bash(program: &str) -> bool {
    if !cfg!(windows) {
        return true;
    }
    let normalized = program.replace('/', "\\").to_ascii_lowercase();
    let Some((dir, _)) = normalized.rsplit_once('\\') else {
        return false;
    };
    let system_root = std::env::var("SystemRoot")
        .unwrap_or_else(|_| r"C:\Windows".to_string())
        .replace('/', "\\")
        .to_ascii_lowercase();
    let system_root = system_root.trim_end_matches('\\');
    !(dir == system_root || dir.starts_with(&format!("{system_root}\\")))
}

pub(super) fn throwaway_dir(prefix: &str) -> Option<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

pub(super) fn setup_zsh() -> Option<Injection> {
    let dir = throwaway_dir(ZDOTDIR_PREFIX)?;
    for (name, contents) in zsh_redirectors() {
        std::fs::write(dir.join(name), contents).ok()?;
    }

    let mut env = HashMap::new();
    if let Some(user_zdotdir) = real_user_zdotdir() {
        env.insert("ANTERM_USER_ZDOTDIR".to_string(), user_zdotdir);
    }
    env.insert("ZDOTDIR".to_string(), dir.to_string_lossy().into_owned());

    Some(Injection {
        env,
        args: Vec::new(),
        replaces_argv: false,
        dir: Some(dir),
    })
}

pub(super) fn setup_fish() -> Option<Injection> {
    Some(Injection {
        env: HashMap::new(),
        args: vec!["-C".to_string(), FISH_INTEGRATION.to_string()],
        replaces_argv: false,
        dir: None,
    })
}

pub(super) fn setup_powershell() -> Option<Injection> {
    Some(Injection {
        env: HashMap::new(),
        args: vec![
            "-NoLogo".to_string(),
            "-NoExit".to_string(),
            "-EncodedCommand".to_string(),
            powershell_encoded_command(POWERSHELL_INTEGRATION),
        ],
        replaces_argv: false,
        dir: None,
    })
}

/// Nushell has no environment variable that redirects its config file the way
/// ZDOTDIR does for zsh, so the injection rides `--config` instead: a
/// throwaway config.nu that sources the user's real one and then appends the
/// OSC hooks. The `dir` is what makes the file disappear when the pane closes.
///
/// Trade-off, inherent to `--config`: `$nu.config-path` points at the wrapper,
/// so `config nu` inside the pane edits a file that vanishes with it. The zsh
/// path dodges the analogue by restoring ZDOTDIR after startup; Nushell's
/// `$nu.*` paths are immutable, so this is accepted rather than fought.
pub(super) fn setup_nushell() -> Option<Injection> {
    setup_nushell_with(nushell_user_config_path().as_deref())
}

/// Split from `setup_nushell` so a test can drive the whole wrapper against a
/// config.nu of its own without writing into the real one.
pub(super) fn setup_nushell_with(user_config: Option<&Path>) -> Option<Injection> {
    let dir = throwaway_dir("anterm-nu-")?;
    let config = dir.join("config.nu");
    std::fs::write(&config, nushell_config_script_with(user_config)).ok()?;

    Some(Injection {
        env: HashMap::new(),
        args: vec![
            "--config".to_string(),
            config.to_string_lossy().into_owned(),
        ],
        replaces_argv: false,
        dir: Some(dir),
    })
}

/// The wrapper config.nu: the user's real config.nu sourced back in when it
/// exists, then the OSC hooks. `source` is a parse-time construct in Nushell —
/// it cannot be guarded at runtime or name a missing file — so the path is
/// resolved and substituted here, and a machine without a config.nu gets a
/// no-op line instead.
pub(super) fn nushell_config_script_with(user_config: Option<&Path>) -> String {
    let source = match user_config {
        Some(path) => format!("source {}", nu_string_literal(&path.to_string_lossy())),
        None => "# no user config.nu to restore".to_string(),
    };
    NUSHELL_INTEGRATION.replace("__ANTERM_SOURCE_USER_CONFIG__", &source)
}

/// Where nu would load its config.nu from. Only a file that exists comes
/// back — the wrapper's `source` cannot name one that is not there.
pub(super) fn nushell_user_config_path() -> Option<PathBuf> {
    let config = nushell_config_dir()?.join("config.nu");
    config.is_file().then_some(config)
}

/// nu's own config directory, resolved the way nu resolves it. Getting this
/// wrong is silent and total: the wrapper reports "no config.nu to restore"
/// for a user who has one, and `nu --config` then replaces their config with
/// one that never sources it.
///
/// nu-path's `configurable_dir_path` consults `$XDG_CONFIG_HOME` on *every*
/// platform, Windows included, and only when it is non-empty **and** absolute;
/// anything else falls through to `dirs::config_dir()`.
fn nushell_config_dir() -> Option<PathBuf> {
    nushell_config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        platform_config_dir(),
    )
}

pub(super) fn nushell_config_dir_from(
    xdg_config_home: Option<&std::ffi::OsStr>,
    platform_default: Option<PathBuf>,
) -> Option<PathBuf> {
    let base = match xdg_config_home {
        Some(xdg) if !xdg.is_empty() && Path::new(xdg).is_absolute() => PathBuf::from(xdg),
        _ => platform_default?,
    };
    Some(base.join("nushell"))
}

/// `dirs::config_dir()` — what nu falls back to. Note this is **not** where
/// anterm keeps its own config (`~/.config/anterm` on macOS too); nu follows the
/// platform convention, and the two are only the same directory on Linux.
#[cfg(windows)]
pub(super) fn platform_config_dir() -> Option<PathBuf> {
    // `dirs` asks Windows for FOLDERID_RoamingAppData; `%APPDATA%` is the
    // same directory in every environment anterm can spawn a pane in.
    std::env::var_os("APPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
pub(super) fn platform_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn platform_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join(".config"))
}

/// A Nushell string literal for `path`. Single quotes are fully literal in
/// Nushell, so they are the default; a path that contains an apostrophe (legal
/// on Windows) falls back to double quotes with backslash escaping.
pub(super) fn nu_string_literal(path: &str) -> String {
    if !path.contains('\'') {
        return format!("'{path}'");
    }
    let escaped = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

pub(super) fn powershell_encoded_command(script: &str) -> String {
    use base64::Engine as _;
    let utf16le: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(&utf16le)
}

pub(super) fn bash_rcfile() -> String {
    format!(
        r#"
# Replays what a real *login* shell would have sourced, in the same order —
# necessary because anterm spawns bash non-login (see shell_integration.rs) so
# that --rcfile is honored at all; bash silently ignores it for login shells.
#
# ~/.bashrc needs both halves of a rule that cannot be written as a static
# order, because the two shapes in the wild want opposite things:
#
#   * a ~/.bash_profile that forwards to ~/.bashrc (the common case, and what
#     every "put this in your .bash_profile" guide produces). Sourcing ~/.bashrc
#     again afterwards runs the user's whole file twice: banners print twice,
#     completions get sourced twice, anything appending to PROMPT_COMMAND stacks.
#   * a ~/.bash_profile that does *not* forward — written by someone whose
#     terminal spawns non-login shells, so ~/.bashrc always arrived on its own
#     and the profile only ever had to hold login-time settings. Dropping
#     ~/.bashrc from the chain would take that user's aliases, prompt and
#     completions away entirely.
#
# So watch instead of guess: wrap `source`/`.` for the length of the chain, note
# whether ~/.bashrc came through, and fill it in afterwards only if it did not.
# `~` is already expanded to $HOME by the time the wrapper sees $1, so matching
# on the trailing path component covers `. ~/.bashrc`, `source "$HOME/.bashrc"`
# and a spelled-out absolute path alike.
__anterm_bashrc=0
source() {{ case "${{1-}}" in */.bashrc|.bashrc) __anterm_bashrc=1;; esac; builtin source "$@"; }}
.() {{ case "${{1-}}" in */.bashrc|.bashrc) __anterm_bashrc=1;; esac; builtin . "$@"; }}
if [[ -f /etc/profile ]]; then source /etc/profile; fi
if [[ -f ~/.bash_profile ]]; then
  source ~/.bash_profile
elif [[ -f ~/.bash_login ]]; then
  source ~/.bash_login
elif [[ -f ~/.profile ]]; then
  source ~/.profile
fi
unset -f source .
if [[ $__anterm_bashrc == 0 && -f ~/.bashrc ]]; then source ~/.bashrc; fi
unset __anterm_bashrc
{BASH_INTEGRATION}"#
    )
}

pub(super) fn bash_path(path: &Path) -> String {
    let s = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

pub(super) fn setup_bash() -> Option<Injection> {
    let dir = throwaway_dir("anterm-bashrc-")?;
    let rcfile = dir.join("bashrc");
    std::fs::write(&rcfile, bash_rcfile()).ok()?;

    Some(Injection {
        env: HashMap::new(),
        args: vec!["--rcfile".to_string(), bash_path(&rcfile), "-i".to_string()],
        replaces_argv: true,
        dir: Some(dir),
    })
}

pub(super) const WSL_RCFILE_ENV: &str = "ANTERM_RC";

/// Where the zsh redirectors landed, in the Windows spelling `WSLENV`'s `/p`
/// translates for the distro.
///
/// Deliberately not `ZDOTDIR`: everything in this map is handed to *every* WSL
/// pane, and a bash or fish distro that found a `ZDOTDIR` in its environment
/// would carry it into any `zsh` started inside it. The bootstrap decides
/// whether this becomes `ZDOTDIR`, and it only decides that for a zsh distro.
pub(super) const WSL_ZDOTDIR_ENV: &str = "ANTERM_ZDOTDIR";

/// POSIX single-quoting, for a body some other shell has to re-parse.
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The bootstrap `sh` runs inside the distro. `$SHELL` is the only place the
/// user's real shell is named, so every arm dispatches on it: bash re-execs
/// through the rcfile written on the Windows side, zsh points `ZDOTDIR` at the
/// redirectors written beside it, fish carries its integration inline the way
/// `remote::bootstrap_command` does over SSH, and anything else falls back to a
/// plain login shell.
///
/// Both file-backed arms ask whether what they were handed is really there
/// before trusting it. The files live on the Windows side and reach the distro
/// over `/mnt`, which is not a given: automount can be off, `/etc/wsl.conf` can
/// move the root, and a distro can have no drvfs at all. Unguarded, bash is
/// handed a `--rcfile` it cannot read — which it ignores in silence, taking the
/// user's own `.bashrc` down with it, because anterm starts it non-login so that
/// `--rcfile` is honoured at all — and zsh is pointed at a `ZDOTDIR` that does
/// not exist, which takes every startup file the user wrote with it. Guarded,
/// such a distro gets the plain login shell it had before any of this: no
/// integration, but the startup files a bare `wsl.exe` pane reads, which is the
/// failure worth having.
///
/// `sh` parses this script, so the fish body is POSIX-quoted here — unlike the
/// SSH path, where the far end's own login shell parses the bootstrap and
/// `remote::fish_bootstrap` has to quote it the way fish reads quotes.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn wsl_exec_script() -> String {
    format!(
        concat!(
            r#"case "${{SHELL:-}}" in "#,
            r#"*/bash) [ -r "${{ANTERM_RC:-}}" ] && exec "$SHELL" --rcfile "$ANTERM_RC" -i; "#,
            r#"exec "$SHELL" -l ;; "#,
            r#"*/zsh) [ -n "${{ANTERM_ZDOTDIR:-}}" ] && [ -r "$ANTERM_ZDOTDIR/.zshrc" ] "#,
            r#"&& export ZDOTDIR="$ANTERM_ZDOTDIR"; exec "$SHELL" -l ;; "#,
            r#"*/fish) exec "$SHELL" -C {} -l ;; "#,
            r#"*) exec "${{SHELL:-/bin/sh}}" -l ;; "#,
            "esac"
        ),
        shell_quote(FISH_INTEGRATION)
    )
}

/// The files a WSL pane needs on the Windows side, and the environment that
/// tells the distro where they are.
///
/// Split out of `setup_wsl` so that everything deciding anything is compiled
/// and tested on every platform; what stays behind the `#[cfg(windows)]` is the
/// argv and a call to this.
///
/// `ANTERM_USER_ZDOTDIR` is conspicuously absent, and that is the point.
/// `real_user_zdotdir` answers from this process's own environment, and this
/// process is on the Windows side of the boundary — the `ZDOTDIR` the user
/// actually set lives inside the distro and is a Linux path nothing out here
/// can name. A Windows path sent across would aim every redirector at a
/// directory that is not there, and the user's `.zshrc` would go missing
/// silently. Left unset, `zsh_redirectors` falls through to `${ZDOTDIR:-$HOME}`
/// — the distro's own home — and its `.zshenv` arm recaptures a relocated
/// `ZDOTDIR` from in there, which is the only side that ever knew it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn wsl_integration_env(dir: &Path, wslenv: Option<&str>) -> Option<HashMap<String, String>> {
    let rcfile = dir.join("bashrc");
    std::fs::write(&rcfile, bash_rcfile()).ok()?;

    let mut env = HashMap::new();
    let mut names = vec![format!("{WSL_RCFILE_ENV}/p")];
    env.insert(
        WSL_RCFILE_ENV.to_string(),
        rcfile.to_string_lossy().into_owned(),
    );

    // The zsh half is best-effort on purpose. It is the bash rcfile that a WSL
    // pane has depended on since this path existed, and failing the whole setup
    // because a second set of files could not be written would take a working
    // bash distro down with it. Nothing names the directory unless all of it
    // landed, so a half-written one is never advertised to the distro.
    if let Some(zdotdir) = wsl_zdotdir(dir) {
        env.insert(WSL_ZDOTDIR_ENV.to_string(), zdotdir);
        names.push(format!("{WSL_ZDOTDIR_ENV}/p"));
    }

    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    env.insert("WSLENV".to_string(), wslenv_with(wslenv, &names));
    Some(env)
}

/// The zsh redirectors, under the pane's own throwaway directory so that the
/// `remove_dir_all` closing the pane already takes them.
#[cfg_attr(not(windows), allow(dead_code))]
fn wsl_zdotdir(dir: &Path) -> Option<String> {
    let zdotdir = dir.join(format!("{ZDOTDIR_PREFIX}wsl"));
    std::fs::create_dir_all(&zdotdir).ok()?;
    for (name, contents) in zsh_redirectors() {
        std::fs::write(zdotdir.join(name), contents).ok()?;
    }
    Some(zdotdir.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn setup_wsl(args: &[String]) -> Option<Injection> {
    let distro = wsl_distro(args);
    let dir = throwaway_dir("anterm-wslrc-")?;
    let env = wsl_integration_env(&dir, std::env::var("WSLENV").ok().as_deref())?;

    let mut argv: Vec<String> = Vec::new();
    if let Some(d) = &distro {
        argv.push("--distribution".to_string());
        argv.push(d.clone());
    }
    if let Some(cd) = wsl_cd(args) {
        argv.push("--cd".to_string());
        argv.push(cd);
    }
    // `--` forwards the command through the distro's default shell, which makes
    // shells such as fish parse the POSIX bootstrap before `sh` can receive it.
    // `--exec` bypasses that shell and executes the bootstrap interpreter itself.
    argv.push("--exec".to_string());
    argv.push("sh".to_string());
    argv.push("-c".to_string());
    argv.push(wsl_exec_script());

    Some(Injection {
        env,
        args: argv,
        replaces_argv: true,
        dir: Some(dir),
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn wsl_cd(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--cd" {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix("--cd=") {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg_attr(not(windows), allow(unused_variables))]
pub fn setup(program: Option<&str>, args: &[String], has_custom_args: bool) -> Option<Injection> {
    // Every shell defers to user-authored args, so the gate sits ahead of the
    // dispatch rather than once per arm — a shell added below inherits it
    // instead of having to remember it. Argv injection would collide with those
    // args outright, and even zsh's env-only ZDOTDIR swap changes which startup
    // files run. Arguments anterm's own detection supplied (Git Bash's `-i -l`,
    // a WSL row's `--distribution`) are not user-authored and never land here;
    // `daemon::pane::has_custom_args` is where that line is drawn.
    if has_custom_args {
        return None;
    }
    let mut injection = match shell_kind(program)? {
        ShellKind::Zsh => setup_zsh(),
        ShellKind::Fish => setup_fish(),
        ShellKind::Bash => setup_bash(),
        ShellKind::PowerShell => setup_powershell(),
        ShellKind::Nushell => setup_nushell(),
        #[cfg(windows)]
        ShellKind::Wsl => setup_wsl(args),
        #[cfg(not(windows))]
        ShellKind::Wsl => None,
    }?;

    injection
        .env
        .insert("ANTERM_SHELL_INTEGRATION".to_string(), String::new());

    Some(injection)
}
