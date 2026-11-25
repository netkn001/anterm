use super::local::{bash_rcfile, powershell_encoded_command, shell_quote, zsh_redirectors};
use super::{FISH_INTEGRATION, POWERSHELL_INTEGRATION};

/// Asks the far host which shell it runs, and is written to be read by
/// every shell sshd might hand it to — the exec request runs through the
/// account's default shell, and on Windows OpenSSH that is `cmd.exe /c`,
/// `powershell.exe -c` or `pwsh -c`, not a POSIX `-c`. So it sticks to
/// words all of them read without complaint, and each answers differently:
///
/// - a POSIX shell (and fish) prints the marker, `$SHELL`, and the two
///   `__anterm_ps*=` keys with nothing after the `=` (fish drops the word
///   outright and prints an empty line);
/// - PowerShell has no `$SHELL` variable, so that `echo` prints nothing,
///   and fills the keys from its own automatic variables — the edition
///   (`Desktop` is Windows PowerShell 5.1, `Core` is pwsh) and the
///   directory its executable lives in;
/// - cmd.exe has no `;` separator: it echoes the whole line back verbatim,
///   marker and all, on one line.
///
/// The shape of each word is load-bearing. No quotes: Windows OpenSSH
/// hands the command to the shell through a Win32 command line, and `"`s
/// there are argv syntax, gone before PowerShell reads the text. No
/// parentheses or pipes: a POSIX shell parses the whole line before
/// running any of it, so one construct it rejects would cost the answer
/// it does give. `key=$VAR` expands in every dialect that expands at all
/// (PowerShell's argument mode included), the `=` ending the key.
pub const PROBE_COMMAND: &str = "echo __anterm_shell; echo $SHELL; \
     echo __anterm_psedition=$PSEdition; echo __anterm_pshome=$PSHOME";

const PROBE_MARKER: &str = "__anterm_shell";

const PS_EDITION_KEY: &str = "__anterm_psedition=";

const PS_HOME_KEY: &str = "__anterm_pshome=";

const HEREDOC: &str = "__ANTERM_RC_EOF__";

/// The longest bootstrap sent to a PowerShell host. Windows caps a
/// process command line at 32,767 UTF-16 units, and the bootstrap rides
/// inside two of them: sshd's `<shell> -c <bootstrap>`, then that shell's
/// launch of the interactive PowerShell, which is a little shorter. The
/// headroom is for sshd's part — the shell's path and switch — with room
/// to spare for any quoting it adds. cmd.exe's far smaller 8,191 never
/// applies: nothing is sent through cmd.
const POWERSHELL_COMMAND_LINE_BUDGET: usize = 30_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RemoteShell {
    Zsh,
    Bash,
    Fish,
    /// Windows PowerShell or pwsh as a Windows account's default shell.
    /// The path the probe pairs with it is the executable, `C:\…\pwsh.exe`.
    PowerShell,
}

impl RemoteShell {
    fn from_path(path: &str) -> Option<Self> {
        match path.rsplit('/').next()? {
            "zsh" => Some(Self::Zsh),
            "bash" => Some(Self::Bash),
            "fish" => Some(Self::Fish),
            _ => None,
        }
    }
}

pub fn parse_probe(output: &str) -> Option<(RemoteShell, String)> {
    let mut lines = output
        .lines()
        .map(|l| l.trim_end_matches('\r').trim())
        .skip_while(|l| *l != PROBE_MARKER);
    lines.next()?;
    let lines: Vec<&str> = lines.collect();
    if let Some(exe) = powershell_from_probe(&lines) {
        return Some((RemoteShell::PowerShell, exe));
    }
    let path = *lines.iter().find(|l| !l.is_empty())?;
    if !path.starts_with('/') {
        return None;
    }
    RemoteShell::from_path(path).map(|shell| (shell, path.to_string()))
}

/// Whether the probe was answered by cmd.exe — Windows OpenSSH's default
/// shell out of the box. Such a host is left a plain shell, on purpose:
/// cmd has no hook that could carry the integration, and starting
/// PowerShell where the account asked for cmd would change the user's
/// shell rather than instrument it. A host whose `DefaultShell` names
/// PowerShell gets it. Only for the log, so the absence explains itself.
pub fn probe_answered_by_cmd(output: &str) -> bool {
    let echoed = format!("{PROBE_MARKER};");
    output.lines().any(|l| l.trim().starts_with(&echoed))
}

/// The PowerShell executable a probe describes, if PowerShell answered.
///
/// The edition names the binary — `powershell.exe` is only ever Windows
/// PowerShell, `pwsh` only ever Core — and `$PSHOME` is the directory it
/// sits in, so the path is exactly the shell that ran the probe: the one
/// the account is configured with, whatever other edition is installed
/// beside it. A Windows host with pwsh installed but Windows PowerShell as
/// its default stays on Windows PowerShell, just as a plain shell request
/// would have left it.
///
/// Windows hosts only. pwsh as a Unix login shell answers the probe too,
/// but there the binary in `$PSHOME` is not always runnable on its own —
/// a package manager's install launches it through a wrapper that sets up
/// the .NET runtime first, and the wrapper is what `$SHELL` names, where
/// the probe cannot read it — so such a host keeps its plain shell.
fn powershell_from_probe(lines: &[&str]) -> Option<String> {
    let field = |key: &str| lines.iter().find_map(|l| l.strip_prefix(key));
    let edition = field(PS_EDITION_KEY)?;
    let home = field(PS_HOME_KEY)?.trim_end_matches(['/', '\\']);
    if !is_windows_path(home) || home.contains('\0') {
        return None;
    }
    match edition {
        "Desktop" => Some(format!(r"{home}\powershell.exe")),
        "Core" => Some(format!(r"{home}\pwsh.exe")),
        _ => None,
    }
}

/// A drive (`C:\…`, `C:/…`) or UNC (`\\server\…`) path.
fn is_windows_path(path: &str) -> bool {
    let b = path.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || path.starts_with(r"\\")
}

/// The script a native SSH session execs in place of a bare shell request.
///
/// `start_dir` is a directory *on the remote host* — where the pane was the
/// last time the far shell reported its cwd — and the session moves there
/// before the user's shell is exec'd, so a redialled pane comes back where
/// it was instead of in the login directory. It rides in the bootstrap
/// rather than being typed at the prompt so it never shows on screen or
/// lands in history, and a directory that has gone away since fails the
/// `cd` quietly and leaves the session in the login directory, exactly as
/// if nothing had been asked.
///
/// The `cd` is the script's first line, ahead of the rc staging: a
/// relative `$TMPDIR` then resolves against one directory for every line
/// that follows, not one before the `cd` and another after it.
pub fn bootstrap_command(
    shell: RemoteShell,
    shell_path: &str,
    start_dir: Option<&str>,
) -> String {
    if shell == RemoteShell::PowerShell {
        return powershell_bootstrap(shell_path, start_dir);
    }
    let mut out = start_dir
        .and_then(usable_start_dir)
        .map(|dir| cd_line(shell, dir))
        .unwrap_or_default();
    out.push_str(&match shell {
        RemoteShell::Zsh => zsh_bootstrap(shell_path),
        RemoteShell::Bash => bash_bootstrap(shell_path),
        RemoteShell::Fish => fish_bootstrap(shell_path),
        RemoteShell::PowerShell => unreachable!("returned above"),
    });
    out
}

/// `dir`, if it can only mean one place on the far host.
///
/// Absolute paths only: a relative one would resolve against the login
/// directory and through `CDPATH`, and a `~` would reach the shell quoted
/// and never expand. Neither is anything OSC 7 reports, so honouring one
/// would be a guess. A NUL cannot travel in an exec request at all.
fn usable_start_dir(dir: &str) -> Option<&str> {
    (dir.starts_with('/') && !dir.contains('\0')).then_some(dir)
}

/// `builtin`, so a `cd` wrapper defined in a file the `-c` shell already
/// read (`.zshenv`, fish's `config.fish`) cannot print, prompt or refuse;
/// `--`, so no directory name is ever taken for an option.
fn cd_line(shell: RemoteShell, dir: &str) -> String {
    let quoted = match shell {
        RemoteShell::Zsh | RemoteShell::Bash => shell_quote(dir),
        RemoteShell::Fish => fish_quote(dir),
        RemoteShell::PowerShell => unreachable!("PowerShell moves with Set-Location"),
    };
    format!("builtin cd -- {quoted} 2>/dev/null\n")
}

/// The command the host's default PowerShell runs: start a second,
/// interactive PowerShell — the same executable — carrying the
/// integration.
///
/// A second process because Windows has no `exec`: the shell sshd started
/// was told `-c`, which runs one command and leaves, and nothing can turn
/// it interactive after the fact. It waits on the child and hands back its
/// exit code, so the session ends when the user's shell does.
///
/// The child is started the way a local PowerShell pane is (`-NoLogo
/// -NoExit -EncodedCommand`), so the user's profile loads first and the
/// integration wraps whatever prompt it settled on. The start directory
/// rides inside the encoded script too — base64 carries any path through
/// the Win32 command line untouched, where a literal would be at the
/// mercy of sshd's quoting — which puts it after the profile: a profile
/// that moves somewhere is overruled for a redialled pane, which is the
/// point of asking for a directory at all. One that has gone away fails
/// quietly into wherever the profile left the session.
fn powershell_bootstrap(exe: &str, start_dir: Option<&str>) -> String {
    let build = |dir: Option<&str>| {
        let mut script = dir.map(set_location_line).unwrap_or_default();
        script.push_str(&compact_powershell(POWERSHELL_INTEGRATION));
        format!(
            "& {} -NoLogo -NoExit -EncodedCommand {}; exit $LASTEXITCODE",
            ps_quote(exe),
            powershell_encoded_command(&script)
        )
    };
    let dir = start_dir.and_then(powershell_start_dir);
    let with_dir = build(dir.as_deref());
    // A start directory long enough to push the command line past what
    // Windows will run is dropped rather than costing the session: the
    // pane opens where the login leaves it, still integrated.
    if dir.is_some() && with_dir.chars().count() > POWERSHELL_COMMAND_LINE_BUDGET {
        return build(None);
    }
    with_dir
}

/// `dir` as a path the far Windows host can only read as one place.
///
/// The integration reports its cwd the way a `file://` URI spells a drive
/// path — `/C:/Users/ann` — and the local side may have kept it that way
/// (a Unix anterm) or already unwrapped it to `C:/Users/ann` (a Windows
/// one); both mean drive C. A UNC share comes back as `//server/share/…`.
/// Anything else is refused as ambiguous: `/Users/ann` is relative to
/// whichever drive is current, and a bare `C:` means wherever drive C
/// last was.
fn powershell_start_dir(dir: &str) -> Option<String> {
    if dir.contains('\0') {
        return None;
    }
    let drive = |b: &[u8]| {
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\')
    };
    if drive(dir.as_bytes()) {
        return Some(dir.to_string());
    }
    if let Some(rest) = dir.strip_prefix('/')
        && drive(rest.as_bytes())
    {
        return Some(rest.to_string());
    }
    let unc = dir.replace('/', r"\");
    let (server, rest) = unc.strip_prefix(r"\\")?.split_once('\\')?;
    let share = rest.split('\\').next().unwrap_or_default();
    // `\\?\` and `\\.\` are device namespaces, not a server's shares.
    let named = |s: &str| !s.is_empty() && s != "?" && s != ".";
    (named(server) && named(share)).then_some(unc)
}

/// `-LiteralPath`, so brackets and `*` in a directory name are only
/// characters; a directory that is gone is caught rather than reported,
/// and `Stop` makes sure it is catchable — without it, a missing path is a
/// non-terminating error that prints straight past the `try`.
fn set_location_line(dir: &str) -> String {
    format!(
        "try {{ Set-Location -LiteralPath {} -ErrorAction Stop }} catch {{ }}\n",
        ps_quote(dir)
    )
}

/// PowerShell single-quoting. PowerShell takes the typographic single
/// quotes for quote marks as well, so they are doubled along with `'`.
fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// The integration with its comments, blank lines and indentation
/// dropped. It travels as base64 of UTF-16 — each character costs 8/3 of
/// one on the command line — and the commentary is half of it. The script
/// has no here-strings and no string that breaks onto a line starting
/// with `#`, so such a line is always a comment.
fn compact_powershell(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    for line in script.lines().map(str::trim) {
        if !line.is_empty() && !line.starts_with('#') {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn fish_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', r"\\").replace('\'', r"\'"))
}

fn write_file(out: &mut String, name: &str, body: &str) {
    out.push_str(&format!(
        "command cat > \"$__anterm_d/{name}\" <<'{HEREDOC}'\n{}\n{HEREDOC}\n",
        body.trim_end_matches('\n')
    ));
}

fn zsh_bootstrap(shell_path: &str) -> String {
    let mut out = String::new();
    out.push_str("__anterm_d=${TMPDIR:-/tmp}/anterm-zdotdir-$$\n");
    out.push_str("command mkdir -p \"$__anterm_d\" 2>/dev/null\n");

    let mut guard = String::new();
    for (name, contents) in zsh_redirectors() {
        let body = if name == ".zshrc" {
            format!("{contents}{ZSH_CLEANUP_HOOK}")
        } else {
            contents
        };
        write_file(&mut out, name, &body);
        guard.push_str(&format!("[ -s \"$__anterm_d/{name}\" ] && "));
    }
    out.push_str(&format!(
        "{guard}export ZDOTDIR=\"$__anterm_d\" ANTERM_RM_DIR=\"$__anterm_d\"\n"
    ));
    out.push_str(&format!("exec {} -l\n", shell_quote(shell_path)));
    out
}

fn bash_bootstrap(shell_path: &str) -> String {
    let quoted = shell_quote(shell_path);
    let mut out = String::new();
    out.push_str("__anterm_d=${TMPDIR:-/tmp}/anterm-bashrc-$$\n");
    out.push_str("command mkdir -p \"$__anterm_d\" 2>/dev/null\n");
    write_file(
        &mut out,
        "bashrc",
        &format!("{}{BASH_CLEANUP_HOOK}", bash_rcfile()),
    );
    out.push_str("if [ -s \"$__anterm_d/bashrc\" ]; then\n");
    out.push_str("export ANTERM_RM_DIR=\"$__anterm_d\"\n");
    out.push_str(&format!("exec {quoted} --rcfile \"$__anterm_d/bashrc\" -i\n"));
    out.push_str("fi\n");
    out.push_str(&format!("exec {quoted} -l\n"));
    out
}

fn fish_bootstrap(shell_path: &str) -> String {
    format!(
        "exec {} -C {} -l\n",
        fish_quote(shell_path),
        fish_quote(FISH_INTEGRATION)
    )
}

const ZSH_CLEANUP_HOOK: &str = r#"
# --- anterm remote cleanup (zsh) ---
if [[ -n "$ANTERM_RM_DIR" ]]; then
  typeset -g __anterm_rm_dir=$ANTERM_RM_DIR
  unset ANTERM_RM_DIR
  __anterm_rm_rcdir() {
    [[ -n "$__anterm_rm_dir" ]] || return 0
    command rm -rf -- "$__anterm_rm_dir"
    unset __anterm_rm_dir
  }
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __anterm_rm_rcdir
fi
# --- end anterm remote cleanup ---
"#;

const BASH_CLEANUP_HOOK: &str = r#"
# --- anterm remote cleanup (bash) ---
if [[ -n "$ANTERM_RM_DIR" ]]; then
  __anterm_rm_dir=$ANTERM_RM_DIR
  unset ANTERM_RM_DIR
  __anterm_rm_rcdir() {
    [[ -n "$__anterm_rm_dir" ]] || return 0
    command rm -rf -- "$__anterm_rm_dir"
    unset __anterm_rm_dir
  }
  precmd_functions+=(__anterm_rm_rcdir)
fi
# --- end anterm remote cleanup ---
"#;

#[cfg(test)]
mod tests {
    use super::super::{BASH_INTEGRATION, ZSH_INTEGRATION};
    use super::*;

    #[test]
    fn probe_reads_the_line_after_the_marker() {
        assert_eq!(
            parse_probe("__anterm_shell\n/bin/zsh\n"),
            Some((RemoteShell::Zsh, "/bin/zsh".to_string()))
        );
        assert_eq!(
            parse_probe("Welcome to prod!\n__anterm_shell\n/usr/local/bin/fish\n"),
            Some((RemoteShell::Fish, "/usr/local/bin/fish".to_string()))
        );
        assert_eq!(
            parse_probe("__anterm_shell\r\n/bin/bash\r\n"),
            Some((RemoteShell::Bash, "/bin/bash".to_string()))
        );
    }

    #[test]
    fn probe_declines_anything_that_isnt_a_shell_we_know() {
        assert_eq!(parse_probe("__anterm_shell\n$SHELL\n"), None);
        assert_eq!(parse_probe("__anterm_shell\n\n"), None);
        assert_eq!(parse_probe("__anterm_shell\n/bin/ksh\n"), None);
        assert_eq!(parse_probe("/bin/zsh\n"), None);
    }

    #[test]
    fn zsh_bootstrap_gates_zdotdir_on_every_redirector_landing() {
        let script = bootstrap_command(RemoteShell::Zsh, "/bin/zsh", None);
        for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            assert!(
                script.contains(&format!("[ -s \"$__anterm_d/{name}\" ] &&")),
                "missing landing check for {name}"
            );
        }
        let export = script.find("export ZDOTDIR=").expect("exports ZDOTDIR");
        let exec = script.find("exec '/bin/zsh' -l").expect("execs zsh");
        assert!(export < exec);
        assert!(script.contains("__anterm_report_cwd"));
    }

    #[test]
    fn file_writing_bootstraps_end_in_a_bare_exec_of_the_users_shell() {
        for (shell, path) in [
            (RemoteShell::Zsh, "/bin/zsh"),
            (RemoteShell::Bash, "/bin/bash"),
        ] {
            let script = bootstrap_command(shell, path, None);
            let last = script.trim_end().lines().last().unwrap();
            assert_eq!(
                last,
                format!("exec '{path}' -l"),
                "{shell:?} bootstrap must end by exec'ing {path} bare"
            );
        }
    }

    #[test]
    fn bash_bootstrap_forces_a_non_login_shell_through_the_rcfile() {
        let script = bootstrap_command(RemoteShell::Bash, "/bin/bash", None);
        assert!(script.contains("exec '/bin/bash' --rcfile \"$__anterm_d/bashrc\" -i"));
        assert!(script.contains("source /etc/profile"));
    }

    #[test]
    fn fish_bootstrap_is_one_exec_carrying_the_escaped_body() {
        let script = bootstrap_command(RemoteShell::Fish, "/usr/bin/fish", None);
        assert!(script.starts_with("exec '/usr/bin/fish' -C '"));
        assert!(script.trim_end().ends_with("' -l"));
        assert!(!script.contains("mkdir"));

        assert!(script.contains(r"printf \'\\e]%s\\a\' $argv[1]"));
    }

    #[test]
    fn quoting_survives_paths_and_bodies_that_fight_back() {
        assert_eq!(shell_quote("/o'dd/zsh"), r"'/o'\''dd/zsh'");
        assert_eq!(fish_quote(r"a'b\c"), r"'a\'b\\c'");
    }

    #[test]
    fn heredoc_delimiter_cannot_appear_in_a_body_it_delimits() {
        for body in [ZSH_INTEGRATION, BASH_INTEGRATION, FISH_INTEGRATION] {
            assert!(!body.contains(HEREDOC));
        }
        assert!(!bash_rcfile().contains(HEREDOC));
    }

    #[cfg(unix)]
    fn parse_check(
        shell: &str,
        syntax_only_flag: &str,
        script: &str,
    ) -> Option<(bool, String)> {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let mut child = Command::new(shell)
            .arg(syntax_only_flag)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(script.as_bytes())
            .expect("write script");
        let out = child.wait_with_output().expect("wait for parse check");
        Some((
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_scripts_parse_under_their_real_shells() {
        let cases = [
            (RemoteShell::Zsh, "zsh", "-n", "/bin/zsh"),
            (RemoteShell::Bash, "bash", "-n", "/bin/bash"),
            (RemoteShell::Fish, "fish", "--no-execute", "/usr/bin/fish"),
        ];
        for (shell, bin, flag, path) in cases {
            for start_dir in [None, Some(AWKWARD_DIR)] {
                let script = bootstrap_command(shell, path, start_dir);
                if let Some((ok, stderr)) = parse_check(bin, flag, &script) {
                    assert!(
                        ok,
                        "{bin} rejected its bootstrap script (start dir {start_dir:?}):\n{stderr}"
                    );
                }
            }
        }
    }

    /// A remote directory with everything in it that quoting has to get
    /// right: a space, a single quote, a backslash and a `$`.
    const AWKWARD_DIR: &str = r"/srv/my service/it's \n $HOME";

    #[test]
    fn a_start_dir_opens_every_bootstrap_with_one_quiet_cd() {
        for (shell, path, expected) in [
            (
                RemoteShell::Zsh,
                "/bin/zsh",
                r"builtin cd -- '/srv/my service/it'\''s \n $HOME' 2>/dev/null",
            ),
            (
                RemoteShell::Bash,
                "/bin/bash",
                r"builtin cd -- '/srv/my service/it'\''s \n $HOME' 2>/dev/null",
            ),
            (
                RemoteShell::Fish,
                "/usr/bin/fish",
                r"builtin cd -- '/srv/my service/it\'s \\n $HOME' 2>/dev/null",
            ),
        ] {
            let script = bootstrap_command(shell, path, Some(AWKWARD_DIR));
            let (first, rest) = script.split_once('\n').expect("more than one line");
            assert_eq!(first, expected, "{shell:?}");
            assert_eq!(
                rest,
                bootstrap_command(shell, path, None),
                "{shell:?}: the cd is added in front, and nothing else changes"
            );
        }
    }

    #[test]
    fn without_a_start_dir_the_bootstrap_has_no_cd_at_all() {
        for (shell, path) in [
            (RemoteShell::Zsh, "/bin/zsh"),
            (RemoteShell::Bash, "/bin/bash"),
            (RemoteShell::Fish, "/usr/bin/fish"),
        ] {
            assert!(!bootstrap_command(shell, path, None).contains("builtin cd"));
        }
    }

    #[test]
    fn a_start_dir_that_is_not_an_absolute_path_is_ignored() {
        for dir in ["", "~/my_service", "my_service", "./x", "/a\0b"] {
            let script = bootstrap_command(RemoteShell::Bash, "/bin/bash", Some(dir));
            assert_eq!(
                script,
                bootstrap_command(RemoteShell::Bash, "/bin/bash", None),
                "{dir:?} must not be turned into a cd"
            );
        }
    }

    /// Runs the bootstrap's own `cd` line, then `pwd`, in `bin` the way
    /// sshd runs an exec request: `<shell> -c <script>`, started in the
    /// login directory. `None` when `bin` is not installed here.
    #[cfg(unix)]
    fn land(
        shell: RemoteShell,
        bin: &str,
        home: &std::path::Path,
        start_dir: &str,
    ) -> Option<std::process::Output> {
        use std::process::Command;
        let script = format!("{}pwd\n", cd_line(shell, start_dir));
        Command::new(bin)
            .arg("-c")
            .arg(script)
            .current_dir(home)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .output()
            .ok()
    }

    #[cfg(unix)]
    #[test]
    fn a_real_shell_lands_in_the_start_dir_and_falls_back_home_without_a_word() {
        let root = tempfile::tempdir().expect("tempdir");
        // Canonical, so `pwd` has no symlinked `/var` to disagree about.
        let root = root.path().canonicalize().expect("canonical tempdir");
        let home = root.join("home");
        let target = root.join(r"my service/it's \n $HOME");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&target).expect("target");
        let target_str = target.to_str().expect("utf-8 temp path");
        let missing = root.join("gone since");
        let missing_str = missing.to_str().expect("utf-8 temp path");

        for (shell, bin) in [
            (RemoteShell::Bash, "bash"),
            (RemoteShell::Zsh, "zsh"),
            (RemoteShell::Fish, "fish"),
        ] {
            let Some(out) = land(shell, bin, &home, target_str) else {
                continue;
            };
            assert!(out.status.success(), "{bin}: {out:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim_end(),
                target_str,
                "{bin} did not land in the start dir"
            );

            let out = land(shell, bin, &home, missing_str).expect("ran once already");
            assert!(out.status.success(), "{bin}: {out:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim_end(),
                home.to_str().unwrap(),
                "{bin} left the login directory for a directory that is gone"
            );
            assert!(
                out.stderr.is_empty(),
                "{bin} complained about the missing directory: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn heredoc_bodies_parse_under_their_real_shells() {
        for (name, contents) in zsh_redirectors() {
            let body = if name == ".zshrc" {
                format!("{contents}{ZSH_CLEANUP_HOOK}")
            } else {
                contents
            };
            if let Some((ok, stderr)) = parse_check("zsh", "-n", &body) {
                assert!(ok, "zsh rejected the remote {name}:\n{stderr}");
            }
        }
        let rcfile = format!("{}{BASH_CLEANUP_HOOK}", bash_rcfile());
        if let Some((ok, stderr)) = parse_check("bash", "-n", &rcfile) {
            assert!(ok, "bash rejected the remote rcfile:\n{stderr}");
        }
    }

    // --- Windows OpenSSH hosts ---

    const WIN_PS51: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    const WIN_PWSH: &str = r"C:\Program Files\PowerShell\7\pwsh.exe";

    #[test]
    fn probe_names_the_powershell_that_answered_it() {
        // What each edition prints for PROBE_COMMAND: no line at all for
        // `echo $SHELL`, then the two keys. Windows output is CRLF.
        assert_eq!(
            parse_probe(
                "__anterm_shell\r\n__anterm_psedition=Desktop\r\n\
                 __anterm_pshome=C:\\Windows\\System32\\WindowsPowerShell\\v1.0\r\n"
            ),
            Some((RemoteShell::PowerShell, WIN_PS51.to_string()))
        );
        assert_eq!(
            parse_probe(
                "__anterm_shell\r\n__anterm_psedition=Core\r\n\
                 __anterm_pshome=C:\\Program Files\\PowerShell\\7\\\r\n"
            ),
            Some((RemoteShell::PowerShell, WIN_PWSH.to_string()))
        );
        // A profile that talks on the way in.
        assert_eq!(
            parse_probe(
                "Loading personal and system profiles took 412ms.\r\n__anterm_shell\r\n\
                 __anterm_psedition=Core\r\n__anterm_pshome=D:\\tools\\pwsh\r\n"
            ),
            Some((
                RemoteShell::PowerShell,
                r"D:\tools\pwsh\pwsh.exe".to_string()
            ))
        );
    }

    #[test]
    fn probe_declines_powershell_answers_it_cannot_place() {
        for out in [
            // pwsh as a Unix login shell keeps its plain shell.
            "__anterm_shell\n__anterm_psedition=Core\n__anterm_pshome=/opt/microsoft/powershell/7\n",
            "__anterm_shell\n__anterm_psedition=Desktop\n__anterm_pshome=/opt/ps\n",
            "__anterm_shell\n__anterm_psedition=Core\n__anterm_pshome=\n",
            "__anterm_shell\n__anterm_psedition=Classic\n__anterm_pshome=C:\\ps\n",
            "__anterm_shell\n__anterm_psedition=Core\n__anterm_pshome=relative\\ps\n",
            // Keys before the marker are someone's banner, not an answer.
            "__anterm_psedition=Core\n__anterm_pshome=C:\\ps\n__anterm_shell\n",
        ] {
            assert_eq!(parse_probe(out), None, "{out:?}");
        }
    }

    #[test]
    fn cmd_answers_the_probe_with_its_own_echo_and_is_left_alone() {
        // cmd has no `;`: the first `echo` prints the rest of the line.
        let echoed = PROBE_COMMAND.strip_prefix("echo ").unwrap();
        let out = format!("{echoed}\r\n");
        assert_eq!(parse_probe(&out), None);
        assert!(probe_answered_by_cmd(&out));
        // sshd may hand cmd the command in quotes; cmd echoes the tail one.
        assert!(probe_answered_by_cmd(&format!("{echoed}\"\r\n")));

        assert!(!probe_answered_by_cmd("__anterm_shell\n/bin/zsh\n"));
        assert!(!probe_answered_by_cmd(
            "__anterm_shell\n__anterm_psedition=Core\n__anterm_pshome=C:\\ps\n"
        ));
    }

    #[test]
    fn posix_answers_are_read_as_before_with_the_empty_keys_trailing() {
        assert_eq!(
            parse_probe("__anterm_shell\n/bin/zsh\n__anterm_psedition=\n__anterm_pshome=\n"),
            Some((RemoteShell::Zsh, "/bin/zsh".to_string()))
        );
        // fish drops a word whose variable is unset: two empty lines.
        assert_eq!(
            parse_probe("__anterm_shell\n/usr/bin/fish\n\n\n"),
            Some((RemoteShell::Fish, "/usr/bin/fish".to_string()))
        );
        // An unset $SHELL must not promote a key line to the shell path.
        assert_eq!(
            parse_probe("__anterm_shell\n\n__anterm_psedition=\n__anterm_pshome=\n"),
            None
        );
    }

    /// Runs PROBE_COMMAND through every shell installed here the way sshd
    /// would (`<shell> -c <command>`), and checks each is recognised as
    /// itself. A Unix pwsh is declined like any Unix PowerShell, but what
    /// it prints shows the probe's words expand in PowerShell's argument
    /// mode — the same parser Windows PowerShell and a Windows pwsh use.
    #[cfg(unix)]
    #[test]
    fn real_shells_answer_the_probe_as_themselves() {
        use std::process::Command;
        let run = |bin: &str, args: &[&str]| {
            Command::new(bin)
                .args(args)
                .arg("-c")
                .arg(PROBE_COMMAND)
                .env("SHELL", format!("/usr/bin/{bin}"))
                .output()
                .ok()
        };
        for (bin, shell) in [
            ("sh", None),
            ("dash", None),
            ("bash", Some(RemoteShell::Bash)),
            ("zsh", Some(RemoteShell::Zsh)),
            ("fish", Some(RemoteShell::Fish)),
        ] {
            let Some(out) = run(bin, &[]) else { continue };
            let parsed = parse_probe(&String::from_utf8_lossy(&out.stdout));
            assert_eq!(parsed.map(|(s, _)| s), shell, "{bin}: {out:?}");
        }
        if let Some(out) = run("pwsh", &["-NoProfile", "-NonInteractive"]) {
            let text = String::from_utf8_lossy(&out.stdout);
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.first(), Some(&PROBE_MARKER), "{text:?}");
            assert_eq!(lines.get(1), Some(&"__anterm_psedition=Core"), "{text:?}");
            let home = lines
                .get(2)
                .and_then(|l| l.strip_prefix(PS_HOME_KEY))
                .unwrap_or_default();
            assert!(
                std::path::Path::new(home).join("pwsh").is_file(),
                "{home:?} is not pwsh's home ({text:?})"
            );
            assert_eq!(parse_probe(&text), None);
            assert!(!probe_answered_by_cmd(&text));
        }
    }

    /// The script inside `-EncodedCommand`, and everything around it.
    fn split_powershell_bootstrap(script: &str) -> (&str, String, &str) {
        let (head, rest) = script
            .split_once("-EncodedCommand ")
            .expect("carries an encoded command");
        let (b64, tail) = rest.split_once(';').expect("ends with the exit");
        (head, super::super::tests::decode_utf16le_base64(b64), tail)
    }

    #[test]
    fn powershell_bootstrap_starts_the_same_powershell_with_the_integration() {
        let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None);
        assert!(
            !script.contains('\n'),
            "one line through the Win32 command line"
        );
        assert!(!script.contains('"'), "no argv quoting for sshd to eat");
        let (head, body, tail) = split_powershell_bootstrap(&script);
        assert_eq!(
            head,
            r"& 'C:\Program Files\PowerShell\7\pwsh.exe' -NoLogo -NoExit "
        );
        assert_eq!(tail, " exit $LASTEXITCODE");
        assert_eq!(body, compact_powershell(POWERSHELL_INTEGRATION));
        for needle in ["]133;A", "]133;B", "]133;C", "]133;D", "]7;file://", "]0;"] {
            assert!(body.contains(needle), "compacted away {needle}");
        }
        assert!(!body.lines().any(|l| l.starts_with('#')));

        let ps51 = bootstrap_command(RemoteShell::PowerShell, WIN_PS51, None);
        assert!(ps51.starts_with(&format!("& '{WIN_PS51}' -NoLogo -NoExit ")));
    }

    #[test]
    fn windows_start_dirs_are_taken_only_when_they_name_one_place() {
        let cases = [
            ("/C:/Users/ann/my dir", Some("C:/Users/ann/my dir")),
            ("C:/Users/ann", Some("C:/Users/ann")),
            (r"d:\work", Some(r"d:\work")),
            ("/C:/", Some("C:/")),
            ("//fs01/share/team", Some(r"\\fs01\share\team")),
            (r"\\fs01\share", Some(r"\\fs01\share")),
            // Relative to whichever drive is current.
            ("/Users/ann", None),
            // Wherever drive C last was.
            ("C:", None),
            ("/C:", None),
            ("C:Users", None),
            ("~/x", None),
            ("x", None),
            ("", None),
            ("//fs01", None),
            ("//fs01/", None),
            (r"\\?\C:\x", None),
            (r"\\.\pipe\x", None),
            ("C:/a\0b", None),
        ];
        for (dir, want) in cases {
            assert_eq!(powershell_start_dir(dir).as_deref(), want, "{dir:?}");
        }
    }

    #[test]
    fn a_start_dir_opens_the_encoded_script_with_one_quiet_set_location() {
        let dir = "/C:/Users/ann/it's ‘quoted’ $HOME";
        let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, Some(dir));
        let (_, body, _) = split_powershell_bootstrap(&script);
        let (first, rest) = body.split_once('\n').unwrap();
        assert_eq!(
            first,
            "try { Set-Location -LiteralPath 'C:/Users/ann/it''s ‘‘quoted’’ $HOME' \
             -ErrorAction Stop } catch { }"
        );
        assert_eq!(rest, compact_powershell(POWERSHELL_INTEGRATION));

        for refused in ["/Users/ann", "C:", "relative"] {
            assert_eq!(
                bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, Some(refused)),
                bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None),
                "{refused:?} must not become a Set-Location"
            );
        }
    }

    #[test]
    fn powershell_bootstrap_fits_a_windows_command_line() {
        // sshd's own part: the shell's path and its `-c`.
        let sshd = WIN_PWSH.len() + " -c ".len() + 2;
        let longest_classic = format!("C:/{}", "d".repeat(257));
        for dir in [None, Some(longest_classic.as_str())] {
            let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, dir);
            let len = script.chars().count();
            assert!(
                len <= POWERSHELL_COMMAND_LINE_BUDGET && len + sshd < 32_767,
                "{len} chars with start dir {dir:?}"
            );
        }
        // A long-path-aware host can report a far longer cwd. It costs the
        // start dir, not the session.
        let huge = format!("C:/{}", "d/".repeat(8_000));
        assert_eq!(
            bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, Some(&huge)),
            bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None),
        );
    }

    #[test]
    fn ps_quote_doubles_every_single_quote_powershell_honours() {
        assert_eq!(ps_quote("a'b"), "'a''b'");
        assert_eq!(ps_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        assert_eq!(ps_quote(r"C:\x $y"), r"'C:\x $y'");
    }

    /// A throwaway home for a pwsh run, so no real profile is read.
    #[cfg(unix)]
    fn pwsh(home: &std::path::Path) -> std::process::Command {
        let mut cmd = std::process::Command::new("pwsh");
        cmd.current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env_remove("ANTERM_SHELL_INTEGRATION");
        cmd
    }

    /// Runs the bootstrap in a real pwsh the way a PowerShell default
    /// shell runs an exec request (`<shell> -c <bootstrap>`), with the
    /// Windows executable swapped for the local one and commands on stdin
    /// in place of a user, and checks the child decodes and runs the
    /// integration and hands its exit code back out through the parent.
    #[cfg(unix)]
    #[test]
    fn a_real_pwsh_runs_the_bootstrap_end_to_end() {
        use std::io::Write as _;
        use std::process::Stdio;

        let home = tempfile::tempdir().expect("tempdir");
        let script = bootstrap_command(RemoteShell::PowerShell, WIN_PWSH, None).replacen(
            &ps_quote(WIN_PWSH),
            "'pwsh'",
            1,
        );
        let Ok(mut child) = pwsh(home.path())
            .args(["-NoProfile", "-c", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return;
        };
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                b"Write-Output \"integrated=$env:ANTERM_SHELL_INTEGRATION\"\n\
                  Write-Output \"prompt=$((Get-Command prompt).ScriptBlock -match '133;A')\"\n\
                  exit 7\n",
            )
            .unwrap();
        let out = child.wait_with_output().expect("pwsh ran");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stdout.contains("integrated=1"), "{stdout}\n{stderr}");
        assert!(stdout.contains("prompt=True"), "{stdout}\n{stderr}");
        assert_eq!(out.status.code(), Some(7), "{stdout}\n{stderr}");
    }

    /// The start directory line in a real pwsh: it lands in a directory
    /// that is there, and says nothing and stays put for one that is gone.
    #[cfg(unix)]
    #[test]
    fn a_real_pwsh_lands_in_the_start_dir_and_stays_quiet_without_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let root = root.path().canonicalize().expect("canonical tempdir");
        let home = root.join("home");
        let target = root.join("it's [a] ‘dir’");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&target).expect("target");
        let missing = root.join("gone since");

        for (dir, want) in [(&target, &target), (&missing, &home)] {
            let script = format!(
                "{}(Get-Location).Path",
                set_location_line(dir.to_str().unwrap())
            );
            let Ok(out) = pwsh(&home)
                .args(["-NoProfile", "-NonInteractive", "-c", &script])
                .output()
            else {
                return;
            };
            assert!(out.status.success(), "{out:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim_end(),
                want.to_str().unwrap()
            );
            assert!(
                out.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
