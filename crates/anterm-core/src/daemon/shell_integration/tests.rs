use std::path::{Path, PathBuf};

use super::local::*;
use super::{
    BASH_INTEGRATION, FISH_INTEGRATION, NUSHELL_INTEGRATION, POWERSHELL_INTEGRATION,
    ZSH_INTEGRATION,
};

#[test]
fn is_our_zdotdir_matches_only_our_prefix() {
    assert!(is_our_zdotdir("/tmp/anterm-zdotdir-1234-0"));
    assert!(is_our_zdotdir("anterm-zdotdir-x"));
    assert!(!is_our_zdotdir("/home/alice/.config/zsh"));
    assert!(!is_our_zdotdir("/tmp/other-zdotdir"));
    assert!(!is_our_zdotdir(""));
    assert!(!is_our_zdotdir("/tmp/not-anterm-zdotdir-1"));
}

/// `\e]133;D;1\a` — what a shell reports for the `false` these tests type.
///
/// The trailing BEL is the point. `contains("133;D;1")` is a prefix match on
/// the exit status, so it also accepts `133;D;127` (the bootstrap exec'd a
/// shell that could not find `false`) and `133;D;130` (interrupted) — a
/// bootstrap that never ran the command at all would pass every assertion
/// below. Matching through the terminator pins the status whole.
const FAILED_COMMAND_MARK: &str = "133;D;1\u{7}";

/// Type `keys` at a freshly spawned shell and return everything it wrote back.
///
/// `keys` carries its own Enter because the two families disagree about which
/// byte that is. A line-discipline shell gets `\n` (the pty's ICRNL would
/// accept `\r` too); PSReadLine puts the tty in raw mode and reads keys
/// itself, so only a real `\r` submits — `\n` just sits in the buffer and the
/// command never runs.
fn prompt_cycle_over_pty(
    program: &str,
    injection: &Injection,
    keys: &[u8],
    cwd: Option<&Path>,
) -> String {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::io::{Read, Write};

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new(program);
    cmd.args(&injection.args);
    for (k, v) in &injection.env {
        cmd.env(k, v);
    }
    if let Some(dir) = cwd {
        cmd.cwd(dir);
    }
    let mut child = pty.slave.spawn_command(cmd).expect("spawn shell");

    let mut writer = pty.master.take_writer().expect("writer");
    let mut reader = pty.master.try_clone_reader().expect("reader");

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut out = Vec::new();
    let mut answered = 0usize;
    let mut typed = false;
    let mut seen_fail = None;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(chunk) => out.extend_from_slice(&chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let text = String::from_utf8_lossy(&out);

        // Play terminal for the one query that blocks: PSReadLine asks where
        // the cursor is (`CSI 6n`) before it will draw anything, and waits for
        // the answer. Nothing is behind this pty to give one, so an unanswered
        // report means pwsh never echoes a keystroke and the whole test times
        // out. The position itself does not matter here — only that a reply
        // arrives. A line-discipline shell never asks, so this is inert for
        // the Git Bash and WSL cases.
        let asked = text.matches("\u{1b}[6n").count();
        if asked > answered {
            for _ in answered..asked {
                let _ = writer.write_all(b"\x1b[1;1R");
            }
            let _ = writer.flush();
            answered = asked;
        }

        // Type only once the first prompt is marked (A or B — they arrive
        // in the same prompt cycle for every integrated shell, and a
        // Nushell without a config-defined prompt_indicator emits only A).
        // Writing at spawn time instead puts the keystrokes ahead of the
        // cursor-position reply in the same input stream, and pwsh — still
        // waiting on that reply — eats `false\r` as the answer to its own
        // query. The command then never runs, and the failure reads as a
        // missing C mark rather than as the race it is.
        if !typed && (text.contains("133;A") || text.contains("133;B")) {
            writer.write_all(keys).expect("write");
            writer.flush().expect("flush");
            typed = true;
        }

        // Stop only once the typed command's own D report lands. A shell can
        // emit an unpaired D while drawing its first prompt, and breaking on
        // that would sample the transcript before the prompt cycle under test
        // has run at all.
        if typed && text.contains(FAILED_COMMAND_MARK) && seen_fail.is_none() {
            seen_fail = Some(std::time::Instant::now());
        }
        // The D mark and the rest of the same prompt cycle (the cwd report,
        // the A mark) are separate writes: breaking the moment the mark
        // arrives can sample the transcript before the report does. Hold
        // the pty open briefly so the cycle's tail lands.
        if let Some(at) = seen_fail {
            if at.elapsed() >= std::time::Duration::from_millis(500) {
                break;
            }
        }
    }
    // Reap before asserting: a panic here would otherwise leave the shell
    // holding the pty open for the rest of the test run.
    let _ = child.kill();
    let _ = child.wait();
    drop(pty.master);
    assert!(
        typed,
        "no prompt-start mark ever arrived, so nothing was typed; got:\n{}",
        String::from_utf8_lossy(&out)
    );
    String::from_utf8_lossy(&out).into_owned()
}

fn reported_cwd(text: &str) -> PathBuf {
    let payload = text
        .split("\u{1b}]")
        .find(|s| s.starts_with("7;file://"))
        .and_then(|s| s.split(['\u{7}', '\u{1b}']).next())
        .unwrap_or_else(|| panic!("expected OSC 7; got:\n{text}"));
    crate::daemon::pane::parse_osc7(payload.as_bytes())
        .unwrap_or_else(|| panic!("daemon could not parse OSC 7 payload {payload:?}"))
}

/// The last OSC 7 cwd report in a transcript — the one emitted after the
/// typed commands ran, which is what proves `cd` moved the pane's cwd.
fn last_osc7(text: &str) -> PathBuf {
    let payload = text
        .split("\u{1b}]")
        .filter(|s| s.starts_with("7;file://"))
        .last()
        .and_then(|s| s.split(['\u{7}', '\u{1b}']).next())
        .unwrap_or_else(|| panic!("expected OSC 7; got:\n{text}"));
    crate::daemon::pane::parse_osc7(payload.as_bytes())
        .unwrap_or_else(|| panic!("daemon could not parse OSC 7 payload {payload:?}"))
}

#[cfg(windows)]
#[test]
fn git_bash_reports_the_full_prompt_cycle_over_a_real_pty() {
    let Some(bash) = crate::shells::git_bash_path() else {
        eprintln!("skipping: Git for Windows not installed");
        return;
    };
    let bash = bash.to_string_lossy().into_owned();
    let injection = setup(Some(&bash), &[], false).expect("bash integration");
    let text = prompt_cycle_over_pty(&bash, &injection, b"false\n", None);

    for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
        assert!(
            text.contains(mark),
            "Git Bash must report {mark:?}; got:\n{text}"
        );
    }
    let cwd = reported_cwd(&text);
    assert!(
        cwd.exists(),
        "Git Bash reported a cwd the Windows side cannot resolve: {cwd:?} \
         — a drive-relative msys path, so `pwd -W` translation regressed"
    );
}

#[cfg(windows)]
#[test]
fn wsl_reports_the_full_prompt_cycle_over_a_real_pty() {
    let Some(distro) = crate::shells::wsl_distros().into_iter().next() else {
        eprintln!("skipping: no WSL distributions installed");
        return;
    };
    let args: Vec<String> = vec![
        "--distribution".into(),
        distro.clone(),
        "--cd".into(),
        "~".into(),
    ];
    let injection = setup(Some("wsl.exe"), &args, false).expect("wsl integration");
    let text = prompt_cycle_over_pty("wsl.exe", &injection, b"false\n", None);

    for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
        assert!(
            text.contains(mark),
            "WSL ({distro}) must report {mark:?}; got:\n{text}"
        );
    }
    let cwd = reported_cwd(&text);
    assert!(
        cwd.to_string_lossy().starts_with('/'),
        "expected the distro's own absolute path, got {cwd:?}"
    );
}

/// The OSC 0 title the shell settled on, as `user@host:dir`.
#[cfg(unix)]
fn reported_title(text: &str) -> String {
    text.split("\u{1b}]")
        .filter_map(|s| s.strip_prefix("0;"))
        .filter_map(|s| s.split(['\u{7}', '\u{1b}']).next())
        // pwsh emits an empty OSC 0 of its own just before ours; the one
        // under test is whatever is left after dropping those.
        .filter(|t| !t.is_empty())
        .last()
        .unwrap_or_else(|| panic!("expected a non-empty OSC 0 title; got:\n{text}"))
        .to_string()
}

#[cfg(unix)]
fn pwsh_on_path() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("pwsh"))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Guards the Unix half of the PowerShell integration, which had no coverage
/// at all until #583 — every pty round-trip here used to be `#[cfg(windows)]`,
/// so a script written against `$env:USERNAME` / `$env:COMPUTERNAME` /
/// `$env:USERPROFILE` shipped for two platforms where all three are empty.
#[cfg(unix)]
#[test]
fn pwsh_reports_the_full_prompt_cycle_over_a_real_pty_on_unix() {
    let Some(pwsh) = pwsh_on_path() else {
        eprintln!("skipping: pwsh not installed");
        return;
    };
    let injection = setup(Some(&pwsh), &[], false).expect("powershell integration");
    // From $HOME, so the title's `~` abbreviation is exercised rather than
    // assumed — it is the half that silently did nothing on Unix.
    let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
    let text = prompt_cycle_over_pty(&pwsh, &injection, b"false\r", Some(&home));

    for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
        assert!(
            text.contains(mark),
            "pwsh must report {mark:?}; got:\n{text}"
        );
    }

    let cwd = reported_cwd(&text);
    assert_eq!(
        cwd,
        home.canonicalize().unwrap_or(home.clone()),
        "OSC 7 must name the pane's real cwd"
    );

    // Asserted by shape, not against $USER: a CI runner does not reliably
    // export it, and the bug being guarded is emptiness — `@:` — not which
    // name lands there. When the environment does name the user, pin it.
    let title = reported_title(&text);
    let (who, path) = title.split_once(':').expect("title is `user@host:dir`");
    let (user, host) = who.split_once('@').expect("title is `user@host:dir`");
    assert!(
        !user.is_empty(),
        "the title must name the user — `$env:USERNAME` is empty on Unix, \
         which is what left it starting with a bare `@`; got {title:?}"
    );
    assert!(
        !host.is_empty(),
        "the title must name the host — `$env:COMPUTERNAME` is empty on Unix; \
         got {title:?}"
    );
    if let Ok(expected) = std::env::var("USER") {
        assert_eq!(user, expected, "the title named the wrong user");
    }
    assert_eq!(
        path, "~",
        "$HOME must abbreviate to `~` — the old check keyed off \
         `$env:USERPROFILE`, which does not exist on Unix; got {title:?}"
    );
}

#[test]
fn shell_kind_maps_known_basenames() {
    assert!(matches!(shell_kind(Some("/bin/zsh")), Some(ShellKind::Zsh)));
    assert!(matches!(shell_kind(Some("zsh")), Some(ShellKind::Zsh)));
    assert!(matches!(
        shell_kind(Some("/bin/bash")),
        Some(ShellKind::Bash)
    ));
    assert!(matches!(
        shell_kind(Some("/usr/local/bin/fish")),
        Some(ShellKind::Fish)
    ));
    for prog in [
        "powershell.exe",
        "powershell",
        "pwsh",
        "pwsh.exe",
        "C:/Program Files/PowerShell/7/pwsh.exe",
        "PowerShell.EXE",
    ] {
        assert!(
            matches!(shell_kind(Some(prog)), Some(ShellKind::PowerShell)),
            "{prog} should map to PowerShell"
        );
    }
    for prog in ["nu", "nu.exe", "C:/Tools/nu.exe"] {
        assert!(
            matches!(shell_kind(Some(prog)), Some(ShellKind::Nushell)),
            "{prog} should map to Nushell"
        );
    }
    assert!(shell_kind(Some("/bin/sh")).is_none());
    assert!(shell_kind(Some("cmd.exe")).is_none());
    assert!(matches!(shell_kind(Some("wsl.exe")), Some(ShellKind::Wsl)));
    assert!(matches!(shell_kind(Some("wsl")), Some(ShellKind::Wsl)));
}

#[cfg(windows)]
#[test]
fn wsl_setup_never_contacts_the_distro() {
    let args: Vec<String> = vec![
        "--distribution".into(),
        "anterm-no-such-distro-exists".into(),
        "--cd".into(),
        "~".into(),
    ];
    let inj = setup(Some("wsl.exe"), &args, false)
        .expect("setup must not depend on reaching the distro");

    let sep = inj
        .args
        .iter()
        .position(|a| a == "--exec")
        .expect("`--exec`");
    assert_eq!(
        &inj.args[..sep],
        &[
            "--distribution".to_string(),
            "anterm-no-such-distro-exists".to_string(),
            "--cd".to_string(),
            "~".to_string()
        ]
    );
    assert_eq!(inj.args[sep + 1], "sh");
    assert_eq!(inj.args[sep + 2], "-c");
    assert!(inj.args[sep + 3].contains("$SHELL"));
    assert!(inj.args[sep + 3].contains("--rcfile"));
    assert!(inj.replaces_argv);
}

#[test]
fn the_wsl_bootstrap_carries_integration_for_fish_not_just_bash() {
    let script = wsl_exec_script();

    assert!(script.contains(
        r#"*/bash) [ -r "${ANTERM_RC:-}" ] && exec "$SHELL" --rcfile "$ANTERM_RC" -i; exec "$SHELL" -l ;;"#
    ));
    assert!(script.contains(&format!(
        r#"*/fish) exec "$SHELL" -C {} -l ;;"#,
        shell_quote(FISH_INTEGRATION)
    )));

    // `sh` parses this script, so the body is POSIX-quoted — backslashes
    // pass through untouched. `remote::fish_quote`, which the SSH path uses
    // because the far end's own fish parses the bootstrap there, would
    // double every one of them and hand fish `\\e]%s\\a`.
    assert!(script.contains(r"printf '\''\e]%s\a'\'' $argv[1]"));
}

#[test]
fn the_wsl_bootstrap_carries_integration_for_zsh_too() {
    let script = wsl_exec_script();

    assert!(
        script
            .contains(r#"*/zsh) [ -n "${ANTERM_ZDOTDIR:-}" ] && [ -r "$ANTERM_ZDOTDIR/.zshrc" ] "#)
    );
    assert!(script.contains(r#"&& export ZDOTDIR="$ANTERM_ZDOTDIR"; exec "$SHELL" -l ;;"#));

    // The redirectors read this to find the user's own startup files, and
    // only the distro can answer it. Naming it out here would aim them at a
    // Windows path no distro has.
    assert!(
        !script.contains("ANTERM_USER_ZDOTDIR"),
        "the bootstrap must not carry a ZDOTDIR this side made up"
    );
}

#[test]
fn wsl_integration_env_writes_what_each_shell_reads_and_names_it_for_the_distro() {
    let dir = throwaway_dir("anterm-wsltest-").expect("temp dir");
    let env = wsl_integration_env(&dir, None).expect("integration files");

    let rcfile = PathBuf::from(env.get(WSL_RCFILE_ENV).expect("ANTERM_RC"));
    assert_eq!(std::fs::read_to_string(&rcfile).unwrap(), bash_rcfile());

    let zdotdir = PathBuf::from(env.get(WSL_ZDOTDIR_ENV).expect("ANTERM_ZDOTDIR"));
    for (name, contents) in zsh_redirectors() {
        assert_eq!(
            std::fs::read_to_string(zdotdir.join(name)).unwrap(),
            contents
        );
    }

    // Both are Windows paths — `/p` is what turns them into the distro's
    // view of themselves, so neither may be pre-translated here.
    assert_eq!(env.get("WSLENV").unwrap(), "ANTERM_RC/p:ANTERM_ZDOTDIR/p");
    assert!(!env.contains_key("ANTERM_USER_ZDOTDIR"));
    assert!(!env.contains_key("ZDOTDIR"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The arms are strings until something runs them. This asks a real `sh`
/// which one it picks, with stub shells standing in for the distro's.
#[cfg(unix)]
#[test]
fn the_wsl_bootstrap_picks_the_arm_that_matches_the_distro_s_shell() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::Command;

    let dir = throwaway_dir("anterm-wslarm-").expect("temp dir");
    for name in ["bash", "zsh", "fish"] {
        let stub = dir.join(name);
        std::fs::write(
            &stub,
            format!("#!/bin/sh\necho {name} \"$@\"\necho ZDOTDIR=${{ZDOTDIR-unset}}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let zdotdir = dir.join("zdot");
    std::fs::create_dir_all(&zdotdir).unwrap();
    std::fs::write(zdotdir.join(".zshrc"), "").unwrap();

    let rcfile = dir.join("bashrc");
    std::fs::write(&rcfile, "").unwrap();

    let run = |shell: &str, rc: &str, zdot: &str| {
        let out = Command::new("sh")
            .arg("-c")
            .arg(wsl_exec_script())
            .env_clear()
            .env("SHELL", dir.join(shell).to_string_lossy().into_owned())
            .env("ANTERM_RC", rc)
            .env("ANTERM_ZDOTDIR", zdot)
            .output()
            .expect("run the bootstrap");
        assert!(out.status.success(), "the bootstrap exited non-zero");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    let rc = rcfile.to_string_lossy().into_owned();
    let zdot = zdotdir.to_string_lossy().into_owned();
    assert!(run("bash", &rc, &zdot).contains("bash --rcfile"));
    assert!(run("fish", &rc, &zdot).starts_with("fish -C"));

    let zsh = run("zsh", &rc, &zdot);
    assert!(zsh.contains("zsh -l"), "{zsh}");
    assert!(zsh.contains(&format!("ZDOTDIR={zdot}")), "{zsh}");

    // The distro that cannot see /mnt. Every arm that was handed a path has
    // to notice, and fall back to the login shell that reads the user's own
    // files rather than to one holding a path that is not there.
    let missing = dir.join("not-there").to_string_lossy().into_owned();

    let blind_zsh = run("zsh", &rc, &missing);
    assert!(blind_zsh.contains("zsh -l"), "{blind_zsh}");
    assert!(blind_zsh.contains("ZDOTDIR=unset"), "{blind_zsh}");

    let blind_bash = run("bash", &missing, &zdot);
    assert!(
        !blind_bash.contains("--rcfile"),
        "bash was handed an rcfile it cannot read: {blind_bash}"
    );
    assert!(blind_bash.contains("bash -l"), "{blind_bash}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The distro runs this through `sh`, so a quoting slip is a pane that
/// never opens. CI's Linux and macOS legs have a real `sh` to ask.
#[cfg(unix)]
#[test]
fn the_wsl_bootstrap_is_valid_posix_sh() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new("sh")
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sh -n");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(wsl_exec_script().as_bytes())
        .expect("write script");
    let out = child.wait_with_output().expect("wait for sh -n");
    assert!(
        out.status.success(),
        "sh rejected the WSL bootstrap:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn wsl_distro_and_cd_are_read_from_either_flag_spelling() {
    let long: Vec<String> = ["--distribution", "Ubuntu-24.04", "--cd", "~"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(wsl_distro(&long).as_deref(), Some("Ubuntu-24.04"));
    assert_eq!(wsl_cd(&long).as_deref(), Some("~"));

    let short: Vec<String> = ["-d", "Debian"].iter().map(|s| s.to_string()).collect();
    assert_eq!(wsl_distro(&short).as_deref(), Some("Debian"));
    assert_eq!(wsl_cd(&short), None);

    let eq: Vec<String> = ["--distribution=Arch", "--cd=/tmp"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(wsl_distro(&eq).as_deref(), Some("Arch"));
    assert_eq!(wsl_cd(&eq).as_deref(), Some("/tmp"));

    assert_eq!(wsl_distro(&[]), None);
    assert_eq!(wsl_distro(&["--distribution".to_string()]), None);
}

#[test]
fn wslenv_preserves_the_users_own_entries() {
    assert_eq!(
        wslenv_with(Some("MYVAR/p:OTHER"), &["ANTERM_RC/p"]),
        "MYVAR/p:OTHER:ANTERM_RC/p"
    );
    assert_eq!(wslenv_with(None, &["ANTERM_RC/p"]), "ANTERM_RC/p");
    assert_eq!(wslenv_with(Some(""), &["ANTERM_RC/p"]), "ANTERM_RC/p");
    assert_eq!(wslenv_with(Some("ANTERM_RC/l"), &["ANTERM_RC/p"]), "ANTERM_RC/l");
    assert_eq!(
        wslenv_with(None, &["ANTERM_RC/p", "ANTERM_ZDOTDIR/p"]),
        "ANTERM_RC/p:ANTERM_ZDOTDIR/p"
    );
    assert_eq!(
        wslenv_with(Some("ANTERM_ZDOTDIR/l"), &["ANTERM_RC/p", "ANTERM_ZDOTDIR/p"]),
        "ANTERM_ZDOTDIR/l:ANTERM_RC/p"
    );
}

#[test]
fn shell_kind_strips_exe_for_non_powershell_shells() {
    for prog in [
        "C:/Program Files/Git/bin/bash.exe",
        "C:/msys64/usr/bin/bash.exe",
    ] {
        assert!(
            matches!(shell_kind(Some(prog)), Some(ShellKind::Bash)),
            "{prog} should map to Bash"
        );
    }
    if !cfg!(windows) {
        for prog in ["bash.exe", "BASH.EXE", "bash"] {
            assert!(matches!(shell_kind(Some(prog)), Some(ShellKind::Bash)));
        }
    }
}

#[test]
#[cfg(windows)]
fn shell_kind_declines_the_wsl_bash_launcher() {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    for prog in [
        format!(r"{system_root}\System32\bash.exe"),
        format!(r"{system_root}/System32/bash.exe"),
        format!(r"{system_root}\SysWOW64\bash.exe"),
        format!(r"{system_root}\system32\BASH.EXE"),
    ] {
        assert!(
            shell_kind(Some(&prog)).is_none(),
            "{prog} is the WSL launcher and must not be treated as Bash"
        );
    }
    for prog in ["bash", "bash.exe", "BASH.EXE"] {
        assert!(
            shell_kind(Some(prog)).is_none(),
            "{prog} cannot be identified as msys bash and must be declined"
        );
    }
}

#[test]
fn bash_rcfile_path_uses_forward_slashes_on_windows() {
    let rendered = bash_path(Path::new(
        r"C:\Users\a\AppData\Local\Temp\anterm-bashrc-1-0\bashrc",
    ));
    if cfg!(windows) {
        assert_eq!(
            rendered,
            "C:/Users/a/AppData/Local/Temp/anterm-bashrc-1-0/bashrc"
        );
    }
    assert_eq!(
        bash_path(Path::new("/tmp/anterm-bashrc-1-0/bashrc")),
        "/tmp/anterm-bashrc-1-0/bashrc"
    );
}

#[test]
fn zsh_redirectors_source_user_files_and_append_integration() {
    let files = zsh_redirectors();
    assert_eq!(files.len(), 4);
    let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, [".zshenv", ".zprofile", ".zshrc", ".zlogin"]);
    for (name, body) in &files {
        assert!(
            body.contains("$ANTERM_USER_ZDOTDIR"),
            "{name} should reference the user's real ZDOTDIR"
        );
        assert!(body.contains(name), "{name} should source its own name");
        assert!(body.contains("source"), "{name} should source");
    }
    let zshrc = &files[2].1;
    assert!(zshrc.contains("__anterm_precmd"));
    assert!(zshrc.contains("133;A"));
    assert!(!files[0].1.contains("__anterm_precmd"));
}

#[test]
fn zsh_redirectors_point_zdotdir_at_the_real_dir_only_while_sourcing() {
    for (name, body) in zsh_redirectors() {
        let save = body.find("__anterm_ztmp=$ZDOTDIR").expect("stashes our dir");
        let aim = body
            .find("ZDOTDIR=$ANTERM_USER_ZDOTDIR")
            .expect("aims at the real dir");
        let source = body
            .find(&format!("source \"${{ZDOTDIR:-$HOME}}/{name}\""))
            .expect("sources the user's file");
        let restore = body
            .rfind("ZDOTDIR=$__anterm_ztmp")
            .expect("restores our dir");
        let unset = body
            .rfind("unset __anterm_ztmp")
            .expect("cleans up its scratch var");
        assert!(
            save < aim && aim < source && source < restore && restore < unset,
            "{name}: order must be stash → aim-at-real → source → restore-ours → unset"
        );
    }
}

#[test]
fn zshenv_recaptures_a_user_relocated_zdotdir() {
    let files = zsh_redirectors();
    let zshenv = &files[0].1;
    let source = zshenv.find("source \"${ZDOTDIR:-$HOME}/.zshenv\"").unwrap();
    let recapture = zshenv
        .find("export ANTERM_USER_ZDOTDIR=${ZDOTDIR:-$HOME}")
        .unwrap();
    let restore = zshenv.rfind("ZDOTDIR=$__anterm_ztmp").unwrap();
    assert!(
        source < recapture && recapture < restore,
        "recapture must run after sourcing the user's .zshenv, before we restore our dir"
    );
    for (name, body) in &files[1..] {
        assert!(
            !body.contains("export ANTERM_USER_ZDOTDIR"),
            "{name} must not re-export ANTERM_USER_ZDOTDIR"
        );
    }
}

#[test]
fn zsh_integration_restores_real_zdotdir_after_startup() {
    assert!(ZSH_INTEGRATION.contains("__anterm_restore_zdotdir"));
    assert!(ZSH_INTEGRATION.contains("ZDOTDIR=${ANTERM_USER_ZDOTDIR:-$HOME}"));
    assert!(
        ZSH_INTEGRATION.contains("add-zsh-hook -d precmd __anterm_restore_zdotdir"),
        "the restore hook must deregister itself so it runs exactly once"
    );
}

#[test]
fn zsh_redirectors_relocate_histfile_away_from_throwaway_zdotdir() {
    let files = zsh_redirectors();
    let zshrc = &files[2].1;
    let aim = zshrc.find("ZDOTDIR=$ANTERM_USER_ZDOTDIR").unwrap();
    let relocate = zshrc
        .find(r#"[[ "$HISTFILE" == "$__anterm_ztmp/"* ]] && HISTFILE="${ZDOTDIR:-$HOME}/${HISTFILE#$__anterm_ztmp/}""#)
        .expect("rewrites HISTFILE when /etc/zshrc pointed it at the throwaway ZDOTDIR");
    let source = zshrc.find("source \"${ZDOTDIR:-$HOME}/.zshrc\"").unwrap();
    assert!(
        aim < relocate && relocate < source,
        "HISTFILE must be relocated after ZDOTDIR is restored and before sourcing user .zshrc"
    );

    #[cfg(unix)]
    if let Some(home) = throwaway_dir("anterm-zsh-hist-home-") {
        let ztmp = home.join("anterm-zdotdir-123");
        std::fs::create_dir_all(&ztmp).unwrap();
        for (name, body) in &files {
            std::fs::write(ztmp.join(name), body).unwrap();
        }
        // Simulate Oh My Zsh's `[ -z "$HISTFILE" ] && HISTFILE="$HOME/.zsh_history"`
        // after macOS `/etc/zshrc` set `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history`.
        std::fs::write(
            home.join(".zshrc"),
            "[ -z \"$HISTFILE\" ] && HISTFILE=\"$HOME/.zsh_history\"\n",
        )
        .unwrap();
        let script = format!(
            "ZDOTDIR='{ztmp}'; HISTFILE=\"${{ZDOTDIR:-$HOME}}/.zsh_history\"; source '{ztmp}/.zshrc'; printf '%s\\n' \"$HISTFILE\"",
            ztmp = ztmp.display()
        );
        if let Ok(out) = std::process::Command::new("zsh")
            .args(["-f", "-c", &script])
            .env("HOME", &home)
            .env_remove("ANTERM_USER_ZDOTDIR")
            .output()
            && out.status.success()
        {
            let got = String::from_utf8_lossy(&out.stdout);
            assert_eq!(
                got.trim(),
                home.join(".zsh_history").to_string_lossy(),
                "HISTFILE must point at the user's real dir, not the throwaway ZDOTDIR"
            );
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}

#[test]
fn bash_rcfile_sources_user_config_then_appends_integration() {
    let rc = bash_rcfile();
    assert!(rc.contains("/etc/profile"));
    assert!(rc.contains("~/.bash_profile"));
    assert!(rc.contains("~/.bashrc"));
    assert!(rc.contains("__anterm"));
    assert!(rc.contains("133;"));
}

/// Runs `bash_rcfile()` against a throwaway $HOME whose ~/.bashrc appends a
/// line every time it is sourced, and reports how many lines it left.
///
/// `None` means there is no usable bash on this box, so there is nothing to
/// assert either way.
#[cfg(unix)]
fn bashrc_sourcings(profile: Option<(&str, &str)>) -> Option<usize> {
    let home = throwaway_dir("anterm-rcfile-home-")?;
    let ticks = home.join("ticks");
    if let Some((name, body)) = profile {
        std::fs::write(home.join(name), body).expect("write profile");
    }
    std::fs::write(
        home.join(".bashrc"),
        format!("printf 'tick\\n' >> '{}'\n", ticks.display()),
    )
    .expect("write .bashrc");
    let rcfile = home.join("rcfile");
    std::fs::write(&rcfile, bash_rcfile()).expect("write rcfile");

    let ran = std::process::Command::new("bash")
        .arg("--rcfile")
        .arg(&rcfile)
        .args(["-i", "-c", "true"])
        .env("HOME", &home)
        .output();
    let sourced = ran.ok().map(|_| {
        std::fs::read_to_string(&ticks)
            .unwrap_or_default()
            .lines()
            .count()
    });
    let _ = std::fs::remove_dir_all(&home);
    sourced
}

/// The profile chain is first-match-wins, and a ~/.bash_profile that exists
/// only to forward to ~/.bashrc is the common case — so sourcing ~/.bashrc
/// after the chain ran it a second time for almost everybody.
#[cfg(unix)]
#[test]
fn a_forwarding_bash_profile_does_not_pull_bashrc_in_twice() {
    for forward in [
        "if [ -f ~/.bashrc ]; then . ~/.bashrc; fi\n",
        "source \"$HOME/.bashrc\"\n",
    ] {
        let Some(sourced) = bashrc_sourcings(Some((".bash_profile", forward))) else {
            return;
        };
        assert_eq!(
            sourced, 1,
            "~/.bashrc must be sourced exactly once for {forward:?}, got {sourced}",
        );
    }
}

/// The other half of the rule: a profile that never forwards is why the
/// unconditional source existed in the first place. Dropping it outright
/// would leave this user with no aliases, prompt or completions at all.
#[cfg(unix)]
#[test]
fn a_profile_that_never_forwards_still_gets_bashrc_once() {
    for profile in [
        Some((".bash_profile", "export ANTERM_PROFILE=1\n")),
        Some((".profile", "export ANTERM_PROFILE=1\n")),
        // No profile at all — the plain macOS $HOME.
        None,
    ] {
        let Some(sourced) = bashrc_sourcings(profile) else {
            return;
        };
        assert_eq!(
            sourced, 1,
            "~/.bashrc must still arrive for {profile:?}, got {sourced}",
        );
    }
}

#[test]
fn every_integration_guards_install_on_empty_sentinel() {
    for (shell, body) in [
        ("zsh", ZSH_INTEGRATION),
        ("bash", BASH_INTEGRATION),
        ("fish", FISH_INTEGRATION),
    ] {
        assert!(
            body.contains(r#"-z "$ANTERM_SHELL_INTEGRATION""#),
            "{shell} integration must guard install on the sentinel being empty \
             (matching setup()'s empty-string reset), not on its mere definedness",
        );
    }
    assert!(
        !FISH_INTEGRATION.contains("set -q ANTERM_SHELL_INTEGRATION"),
        "fish must guard on emptiness (`test -z`), never `set -q`",
    );
}

#[test]
fn d_emitter_is_prepended_ahead_of_user_precmd_hooks() {
    assert!(
        ZSH_INTEGRATION.contains("precmd_functions=(__anterm_precmd_d $precmd_functions)"),
        "zsh must prepend the D emitter (add-zsh-hook can only append)"
    );
    assert!(
        BASH_INTEGRATION
            .contains(r#"precmd_functions=(__anterm_precmd_d "${precmd_functions[@]}")"#),
        "bash must prepend the D emitter"
    );
    for (shell, body) in [
        ("zsh", ZSH_INTEGRATION),
        ("bash", BASH_INTEGRATION),
        ("fish", FISH_INTEGRATION),
    ] {
        assert_eq!(
            body.matches("133;D").count(),
            1,
            "{shell} must emit D from exactly one place"
        );
    }
}

#[test]
fn every_cwd_report_escapes_literal_percent() {
    for (shell, body, escape) in [
        ("zsh", ZSH_INTEGRATION, r"${PWD//\%/%25}"),
        ("bash", BASH_INTEGRATION, r"${PWD//\%/%25}"),
        (
            "fish",
            FISH_INTEGRATION,
            "string replace --all '%' '%25' -- $PWD",
        ),
    ] {
        assert!(
            body.contains(escape),
            "{shell}'s OSC 7 reporter must %-escape the literal percent"
        );
        assert!(
            !body.contains(r#" "$PWD";"#),
            "{shell} must not emit the raw $PWD in its OSC 7 report"
        );
    }
    assert!(
        BASH_INTEGRATION.contains(r"${d//\%/%25}"),
        "bash's msys OSC 7 reporter must %-escape the literal percent too"
    );
}

#[test]
fn bash_reports_a_windows_path_under_msys() {
    let s = BASH_INTEGRATION;
    assert!(
        s.contains(r#"if [[ "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ]]"#),
        "bash must detect msys/cygwin to pick its cwd reporter"
    );
    assert!(
        s.contains("builtin pwd -W"),
        "bash's msys branch must translate the cwd with `pwd -W`"
    );
    assert!(
        s.contains(r#"file://%s/%s"#),
        "bash's msys branch must make the translated path URI-absolute"
    );
    assert!(
        s.contains(r#"[[ "$d" == ?:* ]] || return 0"#),
        "bash's msys branch must report nothing when `pwd -W` yields no drive"
    );
    assert!(
        !s.contains(r#"d="$PWD""#),
        "bash's msys branch must never fall back to the untranslated $PWD"
    );
}

#[test]
fn msys_payload_round_trips_through_parse_osc7() {
    let parse = |payload: &str| {
        crate::daemon::pane::parse_osc7(payload.as_bytes())
            .unwrap_or_else(|| panic!("{payload} should parse"))
    };
    for (translated, want) in [
        ("C:/Users/thoma/repo", "C:/Users/thoma/repo"),
        ("C:/", "C:/"),
        ("D:/work/a b", "D:/work/a b"),
        ("C:/tmp/a%25c", "C:/tmp/a%c"),
    ] {
        let got = parse(&format!("7;file://localhost/{translated}"));
        let want = if cfg!(windows) {
            PathBuf::from(want)
        } else {
            PathBuf::from(format!("/{want}"))
        };
        assert_eq!(got, want, "payload for {translated}");
    }

    if cfg!(windows) {
        let got = parse("7;file://localhost/c/Users/thoma");
        assert_ne!(got, PathBuf::from("C:/Users/thoma"));
        assert!(
            !got.is_absolute(),
            "{got:?} is drive-relative — Windows resolves it against the \
             current drive, which is why it must never be emitted"
        );
    }
}

#[test]
fn setup_fish_injects_startup_command_without_files() {
    let inj = setup_fish().expect("fish injection is infallible");
    assert_eq!(inj.args[0], "-C");
    assert!(inj.args[1].contains("__anterm"));
    assert!(inj.args[1].contains("133;"));
    assert!(inj.env.is_empty());
    assert!(!inj.replaces_argv);
    assert!(inj.dir.is_none());
}

#[test]
fn setup_zsh_writes_redirectors_and_points_zdotdir_at_them() {
    let inj = setup_zsh().expect("zsh setup should succeed");
    let dir = inj.dir.clone().expect("zsh needs a throwaway dir");
    assert_eq!(
        inj.env.get("ZDOTDIR").map(String::as_str),
        Some(dir.to_string_lossy().as_ref())
    );
    assert!(!inj.replaces_argv);
    assert!(inj.args.is_empty());
    for (name, body) in zsh_redirectors() {
        let written = std::fs::read_to_string(dir.join(name)).expect("redirector written");
        assert_eq!(written, body);
    }
    assert!(is_our_zdotdir(&dir.to_string_lossy()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn setup_bash_writes_rcfile_and_forces_non_login() {
    let inj = setup_bash().expect("bash setup should succeed");
    let dir = inj.dir.clone().expect("bash needs a throwaway dir");
    assert_eq!(inj.args[0], "--rcfile");
    assert_eq!(inj.args[2], "-i");
    assert!(inj.replaces_argv);
    let rc = std::fs::read_to_string(&inj.args[1]).expect("rcfile written");
    assert_eq!(rc, bash_rcfile());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn setup_dispatches_by_shell_and_sets_sentinel() {
    let inj = setup(Some("zsh"), &[], false).expect("zsh setup");
    assert_eq!(
        inj.env.get("ANTERM_SHELL_INTEGRATION").map(String::as_str),
        Some("")
    );
    if let Some(d) = inj.dir {
        let _ = std::fs::remove_dir_all(d);
    }

    assert!(setup(Some("zsh"), &[], true).is_none());

    let inj = setup(Some("fish"), &[], false).expect("fish setup");
    assert!(inj.env.contains_key("ANTERM_SHELL_INTEGRATION"));

    assert!(setup(Some("fish"), &[], true).is_none());

    let bash = if cfg!(windows) {
        "C:/Program Files/Git/bin/bash.exe"
    } else {
        "bash"
    };
    let inj = setup(Some(bash), &[], false).expect("bash setup");
    assert!(inj.replaces_argv);
    assert!(inj.env.contains_key("ANTERM_SHELL_INTEGRATION"));
    if let Some(d) = inj.dir {
        let _ = std::fs::remove_dir_all(d);
    }

    assert!(setup(Some(bash), &[], true).is_none());

    let inj = setup(Some("powershell.exe"), &[], false).expect("powershell setup");
    assert!(inj.env.contains_key("ANTERM_SHELL_INTEGRATION"));
    assert!(inj.dir.is_none());
    assert!(!inj.replaces_argv);

    assert!(setup(Some("pwsh"), &[], true).is_none());

    let inj = setup(Some("nu.exe"), &[], false).expect("nushell setup");
    assert!(inj.env.contains_key("ANTERM_SHELL_INTEGRATION"));
    assert_eq!(inj.args[0], "--config");
    assert!(!inj.replaces_argv);
    if let Some(d) = inj.dir {
        let _ = std::fs::remove_dir_all(d);
    }

    assert!(setup(Some("nu.exe"), &[], true).is_none());

    assert!(setup(Some("/bin/sh"), &[], false).is_none());
}

#[test]
fn setup_powershell_injects_encoded_command_without_files() {
    let inj = setup_powershell().expect("powershell injection is infallible");
    assert_eq!(inj.args[0], "-NoLogo");
    assert_eq!(inj.args[1], "-NoExit");
    assert_eq!(inj.args[2], "-EncodedCommand");
    assert_eq!(inj.args.len(), 4);
    let b64 = &inj.args[3];
    assert!(
        b64.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='),
        "encoded command must be pure base64"
    );
    assert_eq!(decode_utf16le_base64(b64), POWERSHELL_INTEGRATION);
    assert!(inj.env.is_empty());
    assert!(inj.dir.is_none());
    assert!(!inj.replaces_argv);
}

#[test]
fn powershell_integration_emits_every_osc_133_mark_and_cwd() {
    let s = POWERSHELL_INTEGRATION;
    assert!(s.contains("]133;A"));
    assert!(s.contains("]133;B"));
    assert!(s.contains("]133;C"));
    assert!(s.contains("]133;D;$code"));
    assert!(s.contains("]7;file://"));
    assert!(s.contains("if (-not $env:ANTERM_SHELL_INTEGRATION)"));
    let ok_at = s.find("$ok = $?").expect("captures $?");
    let exit_at = s.find("$lastExit = $LASTEXITCODE").expect("captures exit");
    assert!(ok_at < exit_at, "$? must be read before the exit code");
    assert!(s.contains("$global:__AntermOrigPrompt = $function:prompt"));
    assert!(s.contains("& $global:__AntermOrigPrompt"));
    assert!(s.contains(".Replace('%', '%25')"));
}

#[test]
fn powershell_integration_sets_an_osc_title() {
    let s = POWERSHELL_INTEGRATION;
    assert!(s.contains("]0;$($global:__AntermUser)@$($global:__AntermHost):"));
    assert!(s.contains("$titlePath = '~'"));
    assert!(s.contains("$titlePath = $fsPath.Replace('\\', '/')"));
}

/// `$env:USERNAME`, `$env:COMPUTERNAME` and `$env:USERPROFILE` are Windows-only
/// spellings that are simply empty on macOS and Linux — reaching for any of
/// them is what titled every Unix pwsh pane `@:` plus a raw absolute path
/// (#583). The script runs unchanged on all three platforms, so the cheapest
/// guard is to keep those names out of it entirely.
#[test]
fn powershell_integration_reads_identity_platform_neutrally() {
    let s = POWERSHELL_INTEGRATION;
    for windows_only in ["$env:USERNAME", "$env:COMPUTERNAME", "$env:USERPROFILE"] {
        assert!(
            !s.contains(windows_only),
            "{windows_only} is empty on macOS and Linux; use the .NET \
             equivalent so the one script is right on every platform"
        );
    }
    assert!(s.contains("[Environment]::UserName"));
    assert!(s.contains("[Environment]::MachineName"));
    assert!(s.contains("$global:__AntermHome = if ($HOME)"));
}

/// A bare `StartsWith($home)` also matches the sibling directory whose name
/// merely begins with the home directory's, so `/Users/annex` used to be
/// retitled `~ex`. The separator has to be part of the match.
#[test]
fn powershell_home_abbreviation_requires_a_path_separator() {
    let s = POWERSHELL_INTEGRATION;
    assert!(s.contains("$titlePath.StartsWith($global:__AntermHome + '/')"));
    assert!(s.contains("$titlePath -eq $global:__AntermHome"));
}

#[test]
fn powershell_encoded_command_round_trips_utf16le() {
    let script = "Write-Host 'héllo ✓'";
    assert_eq!(
        decode_utf16le_base64(&powershell_encoded_command(script)),
        script
    );
}

pub(super) fn decode_utf16le_base64(b64: &str) -> String {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut bytes = Vec::new();
    let mut acc = 0u32;
    let mut nbits = 0;
    for c in b64.bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            bytes.push((acc >> nbits) as u8);
        }
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    String::from_utf16(&units).expect("valid UTF-16LE")
}

#[test]
fn throwaway_dir_is_unique_per_call() {
    let a = throwaway_dir("anterm-test-").expect("dir a");
    let b = throwaway_dir("anterm-test-").expect("dir b");
    assert_ne!(a, b);
    assert!(
        a.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("anterm-test-")
    );
    assert!(a.is_dir() && b.is_dir());
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn setup_nushell_writes_config_and_points_at_it() {
    let inj = setup_nushell().expect("nushell setup should succeed");
    let dir = inj.dir.clone().expect("nushell needs a throwaway dir");
    assert_eq!(inj.args[0], "--config");
    assert_eq!(inj.args.len(), 2);
    let written = std::fs::read_to_string(&inj.args[1]).expect("config written");
    assert_eq!(
        written,
        nushell_config_script_with(nushell_user_config_path().as_deref())
    );
    assert!(inj.env.is_empty());
    assert!(!inj.replaces_argv);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nushell_integration_emits_every_osc_133_mark_and_cwd() {
    let s = NUSHELL_INTEGRATION;
    for mark in ["133;A", "133;B", "133;C", "133;D;"] {
        assert!(s.contains(mark), "Nushell must emit {mark:?}");
    }
    assert!(s.contains("7;file://"));
    assert!(
        s.contains(r#"($nu.os-info.name? | default '') == 'windows'"#),
        "the backslash translation must be gated on Windows — a Unix path \
         may legally contain a literal backslash"
    );
    assert!(
        s.contains(r#"str replace -a '%' '%25'"#),
        "must percent-escape"
    );
    assert!(s.contains("__anterm_cmd_active"));
    assert!(s.contains("hide-env __anterm_cmd_active"));
    assert!(s.contains("prompt_indicator?"));
    assert!(
        s.contains(r##"($env.ANTERM_SHELL_INTEGRATION? | default "") == """##),
        "must guard install on the sentinel being empty, like the other shells"
    );
}

#[test]
fn nushell_config_script_substitutes_a_literal_source_or_a_noop() {
    let with = nushell_config_script_with(Some(Path::new(
        r"C:\Users\ann\AppData\Roaming\nushell\config.nu",
    )));
    assert!(with.contains("source 'C:\\Users\\ann\\AppData\\Roaming\\nushell\\config.nu'"));
    let source_at = with.find("source ").expect("sources the user config");
    let hooks_at = with.find("hooks.pre_prompt").expect("appends hooks");
    assert!(
        source_at < hooks_at,
        "the user config must be sourced before the hooks are appended"
    );

    let without = nushell_config_script_with(None);
    assert!(
        !without.contains("source '") && !without.contains("source \""),
        "no config.nu means nothing to source"
    );
    assert!(without.contains("no user config.nu to restore"));
    assert!(without.contains("hooks.pre_prompt"));
}

#[test]
fn nu_string_literal_quotes_paths_for_the_wrapper() {
    assert_eq!(
        nu_string_literal(r"C:\Users\ann\nushell\config.nu"),
        r"'C:\Users\ann\nushell\config.nu'"
    );
    assert_eq!(
        nu_string_literal("/home/ann/.config/nushell/config.nu"),
        "'/home/ann/.config/nushell/config.nu'"
    );
    // An apostrophe (legal on Windows) forces the double-quoted fallback.
    assert_eq!(
        nu_string_literal(r"C:\it's\config.nu"),
        r#""C:\\it's\\config.nu""#
    );
    // Single quotes are fully literal in Nushell, so `$` needs no escaping
    // there; only an apostrophe forces the double-quoted fallback.
    assert_eq!(
        nu_string_literal(r"C:\a$b\config.nu"),
        r"'C:\a$b\config.nu'"
    );
    assert_eq!(
        nu_string_literal(r"C:\it's\$x\config.nu"),
        r#""C:\\it's\\\$x\\config.nu""#
    );
}

/// nu-path resolves the config dir from `$XDG_CONFIG_HOME` on every
/// platform, but only when it is non-empty *and* absolute. Both halves are
/// load-bearing: the original Windows arm ignored the variable outright,
/// and the original non-Windows arm accepted a relative value nu rejects.
#[test]
fn nushell_config_dir_follows_nu_s_own_resolution_rules() {
    use std::ffi::OsStr;
    // `/x` is not absolute on Windows (no prefix), so the fixtures have to
    // be shaped for the host or the rule under test is never exercised.
    let abs_xdg = if cfg!(windows) { r"C:\xdg" } else { "/xdg" };
    let platform = || {
        Some(PathBuf::from(if cfg!(windows) {
            r"C:\platform"
        } else {
            "/platform"
        }))
    };
    let want = |base: &str| Some(PathBuf::from(base).join("nushell"));

    // Unset, empty, or relative: nu ignores it and takes the platform dir.
    for ignored in [None, Some(OsStr::new("")), Some(OsStr::new("relative/dir"))] {
        assert_eq!(
            nushell_config_dir_from(ignored, platform()),
            want(&platform().unwrap().to_string_lossy()),
            "XDG_CONFIG_HOME {ignored:?} must not displace the platform dir"
        );
    }

    // Non-empty and absolute: it wins, even where there is no platform dir.
    assert_eq!(
        nushell_config_dir_from(Some(OsStr::new(abs_xdg)), platform()),
        want(abs_xdg)
    );
    assert_eq!(
        nushell_config_dir_from(Some(OsStr::new(abs_xdg)), None),
        want(abs_xdg)
    );
    assert_eq!(nushell_config_dir_from(None, None), None);
}

/// The macOS half of the same rule, stated as itself: nu follows the
/// platform convention there, which is *not* where anterm keeps its own
/// config (`~/.config/anterm`). Reading `~/.config/nushell` on a Mac finds
/// nothing, and the wrapper then replaces a config it thinks is absent.
#[cfg(target_os = "macos")]
#[test]
fn nushell_config_dir_is_application_support_on_macos() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    assert_eq!(
        platform_config_dir(),
        Some(home.join("Library/Application Support"))
    );
}

/// Ask the real binary. A hand-copy of nu's resolution rules is only as
/// good as the reading behind it — this is the check that fails outright
/// if the two ever disagree on this machine.
#[cfg(unix)]
#[test]
fn nushell_config_dir_is_what_nu_itself_reports() {
    let Some(nu) = test_nushell_path() else {
        eprintln!("skipping: Nushell not installed");
        return;
    };
    let xdg_absolute = std::env::temp_dir().join("anterm-nu-xdg");
    let cases: [Option<&std::ffi::OsStr>; 4] = [
        None,
        Some(std::ffi::OsStr::new("")),
        Some(std::ffi::OsStr::new("relative/dir")),
        Some(xdg_absolute.as_os_str()),
    ];
    for xdg in cases {
        let mut cmd = std::process::Command::new(&nu);
        cmd.args(["-n", "-c", "$nu.default-config-dir"]);
        match xdg {
            Some(v) => cmd.env("XDG_CONFIG_HOME", v),
            None => cmd.env_remove("XDG_CONFIG_HOME"),
        };
        let out = cmd.output().expect("run nu");
        // A relative value makes nu warn before it answers, so take the
        // last line, not the whole of stdout.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let theirs = PathBuf::from(stdout.lines().last().unwrap_or_default().trim());
        let ours = nushell_config_dir_from(xdg, platform_config_dir())
            .expect("this machine has a config dir");
        assert_eq!(
            ours, theirs,
            "anterm and nu disagree on the config dir for XDG_CONFIG_HOME={xdg:?}"
        );
    }
}

/// End to end over a real pty: a user who *has* a config.nu must still
/// have it after anterm points `nu --config` at the wrapper. `--config`
/// replaces the user's config rather than adding to it, so a wrapper that
/// fails to source it silently strips their prompt, aliases and
/// keybindings.
#[cfg(unix)]
#[test]
fn nushell_restores_the_user_config_and_still_reports_the_prompt_cycle() {
    let Some(nu) = test_nushell_path() else {
        eprintln!("skipping: Nushell not installed");
        return;
    };
    let nu = nu.to_string_lossy().into_owned();
    let home = tempfile::tempdir().expect("tempdir");
    let user_config = home.path().join("config.nu");
    // Shaped like a real config.nu: a wholesale `$env.config = {...}` (the
    // conventional ending, and the one that would wipe hooks added before
    // it), plus a marker only the user's own file could have set.
    std::fs::write(
        &user_config,
        "$env.config = { show_banner: false }\n$env.ANTERM_USER_CONFIG_LOADED = \"yes\"\n",
    )
    .expect("write user config");

    let mut injection = setup_nushell_with(Some(&user_config)).expect("nushell integration");
    // What `setup` does at every spawn boundary, and what this test
    // bypasses by reaching for `setup_nushell_with` directly. Without it
    // the wrapper is a no-op whenever the test itself runs inside an
    // integrated pane — the guard sees the inherited `1` and skips the
    // whole block, and the marks in the transcript are then Nushell's own.
    injection
        .env
        .insert("ANTERM_SHELL_INTEGRATION".to_string(), String::new());
    let text = prompt_cycle_over_pty(
        &nu,
        &injection,
        // One line: Nushell's line editor holds the tty in raw mode and
        // drops what arrives while a command is running, so a second `\r`
        // never reaches it and its prompt cycle never happens. `1 / 0` is
        // the failure — a bare `false` in Nushell is a value.
        b"print $\"probe=($env.ANTERM_USER_CONFIG_LOADED? | default 'MISSING')\"; 1 / 0\r",
        Some(home.path()),
    );

    assert!(
        text.contains("probe=yes"),
        "the user's own config.nu must survive --config; got:\n{text}"
    );
    // Assert on anterm's *own* marks, not on the substring `133;A`: stock
    // Nushell emits its own OSC 133 cycle alongside these, so a bare
    // substring passes even when the wrapper never ran. anterm's hooks
    // terminate with BEL and Nushell's with ST, which tells them apart.
    for mark in ["133;A\u{7}", "133;C\u{7}", FAILED_COMMAND_MARK] {
        assert!(
            text.contains(mark),
            "Nushell must still report {mark:?} after sourcing the user config; got:\n{text}"
        );
    }
    // Same reasoning for the cwd: the hook falls back to `localhost` for
    // the host, while Nushell's native OSC 7 names the real one.
    assert!(
        text.contains("7;file://localhost/"),
        "anterm's own OSC 7 must be among the reports; got:\n{text}"
    );
    assert_eq!(
        last_osc7(&text),
        home.path().canonicalize().expect("canonical tempdir"),
        "the pane's reported cwd must be where nu actually is"
    );
}

/// `shells::nushell_path` is Windows-only (nu has no fixed install path
/// there); on unix the binary is whatever `nu` resolves to on PATH.
#[cfg(unix)]
fn test_nushell_path() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("nu"))
        .find(|p| p.is_file())
}

#[cfg(windows)]
#[test]
fn nushell_reports_the_full_prompt_cycle_over_a_real_pty() {
    let Some(nu) = crate::shells::nushell_path() else {
        eprintln!("skipping: Nushell not installed");
        return;
    };
    let nu = nu.to_string_lossy().into_owned();
    let injection = setup(Some(&nu), &[], false).expect("nushell integration");
    // One line: Nushell's line editor holds the tty in raw mode and can
    // drop what arrives while a command is running, so a second `\r` may
    // never reach it. `cd` first, then a command that fails — `1 / 0`
    // errors with exit status 1 (a literal `false` is just a value).
    let text = prompt_cycle_over_pty(
        &nu,
        &injection,
        b"cd C:/Windows; 1 / 0\r",
        Some(Path::new("C:/")),
    );

    for mark in ["133;A", "133;B", "133;C", FAILED_COMMAND_MARK] {
        assert!(
            text.contains(mark),
            "Nushell must report {mark:?}; got:\n{text}"
        );
    }
    let cwd = reported_cwd(&text);
    assert!(
        cwd.exists(),
        "Nushell reported a cwd the Windows side cannot resolve: {cwd:?} \
         — a drive-relative path, so the URI leading-slash translation regressed"
    );
    assert_eq!(
        last_osc7(&text),
        PathBuf::from("C:/Windows"),
        "`cd` must move the pane's reported cwd; got:\n{text}"
    );
}
