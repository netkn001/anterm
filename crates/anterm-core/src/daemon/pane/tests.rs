use std::path::Path;
#[cfg(windows)]
use std::sync::Condvar;

use portable_pty::CommandBuilder;

use crate::kitty_graphics::ImageDelete;
use crate::term_modes::Query;
use crate::daemon::shell_integration;

use super::*;
use super::fanout::{attach_subscriber, resize_state};
use super::restore::{REDRAW_NUDGE_COOLDOWN, detached_reply, replay_state};
use super::ring::{MAX_RING_SEGMENTS, RING_CAP};
use super::signals::{
    MAX_OSC_TITLE, ObservedFacts, SniffSignals, agent_from_shell_mark, hex_val,
    is_foreground_command, is_interrupt_key, percent_decode, settle_interrupt,
    shell_mark_capture_changed, strip_uri_drive_slash,
};
#[cfg(target_os = "macos")]
use super::spawn_env::{LOCALE_DEFINITION_DIR, system_locale_identifier};
#[cfg(not(windows))]
use super::spawn_env::{default_prog_with_override, default_shell_name};
#[cfg(windows)]
use super::spawn_env::wsl_remote_context;
use super::spawn_env::{
    ANTERM_CONFIG_DIR_ENV, ChosenShell, LOCALE_FALLBACK_KEY, SHELL_ENV,
    apply_common_command_setup, apply_shell_integration, build_shell_command, character_locale,
    choose_shell, config_dir_env, has_custom_args, initial_working_directory,
    launched_shell_program, locale_fallback_is_needed, pane_environment, posix_locale_stem,
    reroute_configured_zdotdir, shell_env_path, shell_program_problem,
};

/// Only Prime Agent's daemon reports from outside the pane, and only to a
/// pane running Prime Agent.
#[test]
fn a_detached_report_is_taken_only_from_prime_agent_in_its_own_pane() {
    use super::owns_detached_report;
    use crate::cli_agent::CLIAgent;
    let prime = Some(CLIAgent::PrimeAgent);
    assert!(owns_detached_report(prime, prime));
    assert!(!owns_detached_report(prime, None));
    assert!(!owns_detached_report(prime, Some(CLIAgent::Pi)));
    assert!(!owns_detached_report(
        Some(CLIAgent::Pi),
        Some(CLIAgent::Pi)
    ));
    assert!(!owns_detached_report(None, prime));
}

/// A pane inherits the directory the last one *reported* — the logical
/// path, `/tmp/x` and not the `/private/tmp/x` the kernel resolves it to.
/// `cwd()` alone loses that: the shell falls back to `getcwd()` and one
/// tab reads `/tmp/x` while the tab opened from it reads `/private/tmp/x`.
/// `PWD` is what carries the name across, and what the shell checks.
/// An AppImage launch leaves `ARGV0` in the daemon's environment; a zsh
/// that inherits it renames every command it runs, which breaks Volta's
/// shims (#1100).
#[test]
fn the_appimage_runtimes_argv0_does_not_reach_the_pane() {
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.env("ARGV0", "./anterm-26.9.5-linux-x86_64.AppImage");
    apply_common_command_setup(&mut cmd, &None, 1, "sh");
    assert_eq!(cmd.get_env("ARGV0"), None);
}

#[cfg(unix)]
#[test]
fn an_inherited_directory_keeps_the_name_it_was_reached_by() {
    let mut cmd = CommandBuilder::new("/bin/sh");
    // `CommandBuilder` starts from this process's environment, so `PWD`
    // already names wherever the daemon happens to be. The pane's own
    // directory has to win over it.
    let inherited = cmd.get_env("PWD").map(std::ffi::OsStr::to_owned);
    let dir = Some(std::path::PathBuf::from("/tmp"));
    apply_common_command_setup(&mut cmd, &dir, 1, "sh");
    assert_ne!(cmd.get_env("PWD").map(std::ffi::OsStr::to_owned), inherited);
    assert_eq!(
        cmd.get_env("PWD"),
        Some(std::ffi::OsStr::new("/tmp")),
        "the shell is told which name it arrived by"
    );
}

#[test]
fn a_shell_that_cannot_run_is_named_once_not_wrapped_four_deep() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("not-a-shell");
    let problem = shell_program_problem(&missing.to_string_lossy()).expect("a problem");
    assert!(problem.contains("no such shell"), "{problem}");
    assert!(problem.contains("not-a-shell"), "{problem}");
    // One sentence: none of the layers this used to arrive wrapped in.
    assert!(!problem.contains("ENOENT"), "{problem}");
    assert!(!problem.contains("Unable to spawn"), "{problem}");

    // A directory is a different mistake and says so.
    let as_dir = dir.path().to_string_lossy().to_string();
    assert!(
        shell_program_problem(&as_dir)
            .expect("a problem")
            .contains("directory")
    );

    // A bare name goes through PATH; guessing at it here would be worse
    // than saying nothing.
    assert_eq!(shell_program_problem("zsh"), None);

    // Something real passes. The test binary is the one path guaranteed
    // to exist and carry the execute bit on every platform this runs on —
    // `/bin/sh` named a file that is simply absent on Windows, so the
    // check meant to prove a good shell passes proved the opposite there.
    let real = std::env::current_exe().expect("the running test binary");
    assert_eq!(shell_program_problem(&real.to_string_lossy()), None);
}

#[cfg(unix)]
#[test]
fn a_shell_without_the_execute_bit_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("not-executable");
    std::fs::write(&path, b"#!/bin/sh\n").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let problem = shell_program_problem(&path.to_string_lossy()).expect("a problem");
    assert!(problem.contains("not executable"), "{problem}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert_eq!(shell_program_problem(&path.to_string_lossy()), None);
}

#[test]
fn initial_working_directory_skips_paths_that_are_not_directories() {
    let real = std::env::temp_dir();
    assert!(real.is_dir(), "temp dir should exist");

    assert_eq!(
        initial_working_directory(Some(real.clone())),
        Some(real.clone())
    );

    for bogus in [
        "/home/someone/definitely-not-here",
        "/c/Users/definitely-not-here",
    ] {
        let got = initial_working_directory(Some(PathBuf::from(bogus)));
        assert_ne!(
            got.as_deref(),
            Some(Path::new(bogus)),
            "{bogus} is not a directory here and must not be handed to spawn"
        );
        if let Some(d) = got {
            assert!(d.is_dir(), "fallback {d:?} must be a real directory");
        }
    }

    let file = real.join("anterm-iwd-probe");
    std::fs::write(&file, b"x").expect("write probe file");
    let got = initial_working_directory(Some(file.clone()));
    assert_ne!(got.as_deref(), Some(file.as_path()));
    let _ = std::fs::remove_file(&file);
}

/// A hook's report that arrives over the socket lands like one read off
/// the output — but only from a process that runs in the pane.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_codex_report_finds_its_pane_by_session_then_directory() {
    let pane = |pane: u64, codex: bool, cwd: &str, session: Option<&str>| CodexCandidate {
        pane,
        runs_codex: codex,
        cwd: Some(PathBuf::from(cwd)),
        session: session.map(str::to_string),
    };
    let panes = [
        pane(1, true, "/work/a", Some("s-a")),
        pane(2, true, "/work/b", None),
        pane(3, false, "/work/b", None),
        pane(4, true, "/work/c", Some("s-c")),
        pane(5, true, "/work/c", None),
    ];
    let target = |named: u64, session: Option<&str>, cwd: &str| {
        codex_report_target(&panes, named, session, Some(Path::new(cwd)), false)
    };

    // The app-server was started in pane 1, so every hook names it.
    assert_eq!(target(1, Some("s-a"), "/work/a"), Some(1));
    assert_eq!(
        target(1, Some("s-c"), "/work/elsewhere"),
        Some(4),
        "a reported session stays with its pane"
    );
    assert_eq!(
        target(1, Some("s-new"), "/work/b"),
        Some(2),
        "a new session goes to the one Codex pane in its directory"
    );
    assert_eq!(
        target(1, Some("s-new"), "/work/c"),
        Some(5),
        "of two there, the one yet to report a session"
    );
    assert_eq!(
        target(1, Some("s-next"), "/work/a"),
        Some(1),
        "a new session in a pane that had one before (/new) is still that pane's"
    );
    assert_eq!(
        codex_report_target(&panes, 1, Some("s-c2"), Some(Path::new("/work/c")), true),
        Some(4),
        "but a session /new started goes to the one that had a session"
    );
    assert_eq!(
        target(1, Some("s-new"), "/work/none"),
        None,
        "a directory no pane runs Codex in leaves every fresh pane in the running"
    );
    let elsewhere = [
        pane(1, true, "/work/a", Some("s-a")),
        pane(2, true, "/home", None),
    ];
    assert_eq!(
        codex_report_target(
            &elsewhere,
            1,
            Some("s-b"),
            Some(Path::new("/work/b")),
            false
        ),
        Some(2),
        "codex -C: the one Codex pane yet to report, not the app-server's"
    );

    let twins = [
        pane(6, true, "/work/d", None),
        pane(7, true, "/work/d", None),
    ];
    let twin = |named| {
        codex_report_target(&twins, named, Some("s"), Some(Path::new("/work/d")), false)
    };
    assert_eq!(twin(7), Some(7), "a tie goes to the pane the hook named");
    assert_eq!(twin(1), None, "and is no one's otherwise");
}

#[test]
fn a_hook_report_counts_only_from_inside_the_pane() {
    let (tx, rx) = mpsc::channel();
    let pane = DaemonPane::spawn(
        1,
        Some(PathBuf::from("/")),
        ws(80, 24),
        Some(ShellSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "exec cat".into()],
            args_are_anterm_defaults: false,
        }),
        None,
        None,
        false,
        || {},
    )
    .expect("spawn pane");
    pane.attach(tx);
    let shell = pane.pty().and_then(|p| p.shell_pid).expect("shell pid");
    let body = r#"{"v":1,"agent":"claude","event":"prompt-submit","session_id":"s-1"}"#;

    let outsider = pane.report_agent_event(std::process::id(), body);
    let garbled = pane.report_agent_event(shell, "{not json");
    let applied = pane.report_agent_event(shell, body);
    let mut status = None;
    while let Ok(msg) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
        if let DaemonMsg::AgentStatus(s) = msg {
            status = s;
        }
    }
    pane.kill();

    assert!(
        outsider.is_err(),
        "the test runner does not run in the pane"
    );
    assert!(garbled.is_err());
    assert_eq!(applied, Ok(()));
    let status = status.expect("the report reached the pane's subscribers");
    assert_eq!(status.status, crate::cli_agent::AgentStatus::Working);
    assert_eq!(status.session_id.as_deref(), Some("s-1"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn live_pane_reports_an_uninstrumented_shells_cwd() {
    let target = std::path::Path::new("/usr");

    let (tx, rx) = mpsc::channel();
    let pane = DaemonPane::spawn(
        1,
        Some(PathBuf::from("/")),
        ws(80, 24),
        Some(ShellSpec {
            program: "sh".into(),
            args: vec!["-c".into(), "cd /usr && exec cat".into()],
            args_are_anterm_defaults: false,
        }),
        None,
        None,
        false,
        || {},
    )
    .expect("spawn pane");
    pane.attach(tx);

    let mut reported = None;
    for _ in 0..200 {
        pane.write_input(b"\n");
        while let Ok(msg) = rx.try_recv() {
            if let DaemonMsg::Cwd(p) = msg {
                reported = Some(p);
            }
        }
        if reported.as_deref() == Some(target) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    pane.kill();

    assert_eq!(
        reported.as_deref(),
        Some(target),
        "a pane whose shell emits no OSC 7 must still report where it \
         actually is — this is what a new tab inherits (issue #187)"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn live_pty_child_argv_detects_the_agent() {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let mut cmd = CommandBuilder::new("bash");
    cmd.args(["-c", "exec -a codex cat"]);
    let mut child = pty.slave.spawn_command(cmd).expect("spawn child");
    let master = Mutex::new(Some(pty.master));

    let mut detected = None;
    for _ in 0..200 {
        if let Some(agent) = foreground_agent(&master).flatten() {
            detected = Some(agent);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();

    let (agent, argv) = detected.expect("agent detected from live PTY child");
    assert_eq!(
        agent,
        crate::cli_agent::CLIAgent::Codex,
        "a live PTY child with argv[0]=codex must be detected as Codex"
    );
    assert_eq!(
        argv.first().map(String::as_str),
        Some("codex"),
        "the observed argv rides along with the detection"
    );
}

#[test]
fn choose_shell_prefers_override_then_config_then_default() {
    let over = ShellSpec {
        program: "fish".into(),
        args: vec!["-l".into()],
        args_are_anterm_defaults: true,
    };
    let cfg = ("zsh".to_string(), vec!["-i".to_string()]);

    assert_eq!(
        choose_shell(Some(over.clone()), Some(cfg.clone())),
        Some(ChosenShell {
            program: "fish".to_string(),
            args: vec!["-l".to_string()],
            args_are_anterm_defaults: true,
        })
    );
    assert_eq!(
        choose_shell(None, Some(cfg.clone())),
        Some(ChosenShell {
            program: "zsh".to_string(),
            args: vec!["-i".to_string()],
            args_are_anterm_defaults: false,
        })
    );
    assert_eq!(choose_shell(None, None), None);
}

#[test]
fn only_user_authored_args_block_shell_integration() {
    let chosen = |args: Vec<&str>, anterm: bool| ChosenShell {
        program: r"C:\Program Files\Git\bin\bash.exe".to_string(),
        args: args.into_iter().map(str::to_string).collect(),
        args_are_anterm_defaults: anterm,
    };

    assert!(!has_custom_args(Some(&chosen(vec!["-i", "-l"], true))));
    assert!(has_custom_args(Some(&chosen(vec!["-i", "-l"], false))));
    assert!(!has_custom_args(Some(&chosen(vec![], false))));
    assert!(!has_custom_args(Some(&chosen(vec![], true))));
    assert!(!has_custom_args(None));
}

#[cfg(windows)]
#[test]
fn wsl_panes_are_tagged_as_a_foreign_filesystem() {
    let spec = |program: &str, args: Vec<&str>| ChosenShell {
        program: program.to_string(),
        args: args.into_iter().map(str::to_string).collect(),
        args_are_anterm_defaults: true,
    };

    let ctx = wsl_remote_context(Some(&spec(
        "wsl.exe",
        vec!["--distribution", "Ubuntu-24.04", "--cd", "~"],
    )))
    .expect("wsl.exe must be tagged");
    assert_eq!(ctx.kind, RemoteKind::Wsl);
    assert_eq!(ctx.target, "Ubuntu-24.04");
    assert!(ctx.argv.is_empty());

    assert_eq!(
        wsl_remote_context(Some(&spec("wsl.exe", vec!["-d", "Debian"])))
            .expect("short flag")
            .target,
        "Debian"
    );
    assert_eq!(
        wsl_remote_context(Some(&spec("wsl.exe", vec![])))
            .expect("default distro is still WSL")
            .target,
        crate::shells::default_wsl_distro().unwrap_or_default(),
        "no --distribution resolves to the machine's default distro"
    );
    assert_eq!(
        wsl_remote_context(Some(&spec("wsl.exe", vec!["--distribution=Arch"])))
            .expect("joined flag")
            .target,
        "Arch"
    );
    assert!(wsl_remote_context(Some(&spec(r"C:\Windows\System32\WSL.EXE", vec![]))).is_some());

    let from_config = choose_shell(None, Some(("wsl.exe".to_string(), Vec::new())));
    assert_eq!(
        wsl_remote_context(from_config.as_ref()).map(|c| c.kind),
        Some(RemoteKind::Wsl),
        "a configured wsl.exe is as much a WSL pane as a dropdown one"
    );

    assert!(wsl_remote_context(Some(&spec("powershell.exe", vec![]))).is_none());
    assert!(
        wsl_remote_context(Some(&spec(r"C:\Program Files\Git\bin\bash.exe", vec![]))).is_none()
    );
    assert!(wsl_remote_context(None).is_none());
}

#[test]
fn arg_based_integration_rebuilds_default_shell_builder() {
    let mut cmd = CommandBuilder::new_default_prog();
    let injection = shell_integration::Injection {
        env: std::collections::HashMap::new(),
        args: vec!["-C".to_string(), "echo ready".to_string()],
        replaces_argv: false,
        dir: None,
    };

    apply_shell_integration(&mut cmd, "/bin/fish", &injection);

    assert!(!cmd.is_default_prog(), "argv can now be appended safely");
    let argv: Vec<_> = cmd
        .get_argv()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv, vec!["/bin/fish", "-C", "echo ready"]);
}

#[test]
fn a_configured_zdotdir_feeds_the_redirector_instead_of_replacing_it() {
    let env = vec![
        ("ZDOTDIR".to_string(), "/home/a/.config/zsh".to_string()),
        ("EDITOR".to_string(), "vi".to_string()),
    ];

    let rerouted = reroute_configured_zdotdir(env.clone(), true);
    assert_eq!(
        rerouted,
        vec![
            (
                "ANTERM_USER_ZDOTDIR".to_string(),
                "/home/a/.config/zsh".to_string()
            ),
            ("EDITOR".to_string(), "vi".to_string()),
        ]
    );
    // Without the redirector there is nothing to protect.
    assert_eq!(reroute_configured_zdotdir(env.clone(), false), env);
}

#[test]
fn env_only_integration_keeps_default_login_shell_builder() {
    let mut cmd = CommandBuilder::new_default_prog();
    let mut env = std::collections::HashMap::new();
    env.insert("ZDOTDIR".to_string(), "/tmp/anterm-zdotdir-test".to_string());
    let injection = shell_integration::Injection {
        env,
        args: Vec::new(),
        replaces_argv: false,
        dir: None,
    };

    apply_shell_integration(&mut cmd, "/bin/zsh", &injection);

    assert!(
        cmd.is_default_prog(),
        "zsh still launches as the login shell"
    );
    assert_eq!(
        cmd.get_env("ZDOTDIR").and_then(|value| value.to_str()),
        Some("/tmp/anterm-zdotdir-test")
    );
}

#[cfg(not(windows))]
#[test]
fn detected_shell_override_uses_explicit_command_builder() {
    let portable_shell = CommandBuilder::new_default_prog().get_shell();
    let detected_shell = format!("{portable_shell}-detected");
    let cmd = default_prog_with_override(Some(detected_shell.clone()));

    assert!(!cmd.is_default_prog());
    let argv: Vec<_> = cmd
        .get_argv()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv, vec![detected_shell.clone()]);
    assert_eq!(default_shell_name(&cmd), detected_shell);
}

#[cfg(not(windows))]
#[test]
fn no_detected_shell_keeps_portable_login_default() {
    let cmd = default_prog_with_override(None);

    assert!(cmd.is_default_prog());
    assert!(!default_shell_name(&cmd).is_empty());
}

#[test]
fn join_bounded_returns_true_when_the_thread_finishes() {
    let handle = std::thread::spawn(|| {});
    assert!(join_bounded(handle, Duration::from_secs(5)));
}

#[test]
fn join_bounded_times_out_on_a_stuck_thread() {
    let (unblock, blocked) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let _ = blocked.recv();
    });
    assert!(!join_bounded(handle, Duration::from_millis(50)));
    drop(unblock);
}

#[test]
fn gate_passes_below_high_water() {
    let gate = OutputGate::new();
    gate.add((OutputGate::HIGH_WATER - 1) as usize);
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    assert!(
        t0.elapsed() < Duration::from_millis(100),
        "no wait expected"
    );
}

#[test]
fn gate_parks_at_high_water_until_drained() {
    let gate = Arc::new(OutputGate::new());
    gate.add(OutputGate::HIGH_WATER as usize);

    let drainer = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            gate.sub(OutputGate::HIGH_WATER as usize);
        })
    };
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    let waited = t0.elapsed();
    drainer.join().unwrap();
    assert!(waited >= Duration::from_millis(40), "must park until sub()");
    assert!(
        waited < OutputGate::MAX_WAIT,
        "the drain, not the escape timeout, must unpark"
    );
}

#[test]
fn gate_reset_unparks_a_throttled_reader() {
    let gate = Arc::new(OutputGate::new());
    gate.add(OutputGate::HIGH_WATER as usize * 2);

    let resetter = {
        let gate = gate.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            gate.reset();
        })
    };
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    resetter.join().unwrap();
    assert!(t0.elapsed() < OutputGate::MAX_WAIT);
}

#[test]
fn gate_tolerates_negative_drift() {
    let gate = OutputGate::new();
    gate.sub(1024);
    gate.add(512);
    let t0 = std::time::Instant::now();
    gate.wait_below_high_water();
    assert!(t0.elapsed() < Duration::from_millis(100));
}

fn ws(cols: u16, rows: u16) -> WinSize {
    WinSize {
        cols,
        rows,
        cell_w: 8,
        cell_h: 16,
    }
}

#[test]
fn ring_under_cap_keeps_all() {
    let mut ring = ReplayRing::new(ws(80, 24));
    ring.append(b"hello ");
    ring.append(b"world");
    assert_eq!(ring.flatten(), b"hello world");
}

#[test]
fn ring_over_cap_drops_oldest() {
    let mut ring = ReplayRing::new(ws(80, 24));
    ring.append(&vec![b'a'; RING_CAP]);
    assert_eq!(ring.len, RING_CAP);
    ring.append(&vec![b'b'; 100]);
    assert_eq!(ring.len, RING_CAP);
    let flat = ring.flatten();
    assert_eq!(&flat[..RING_CAP - 100], &vec![b'a'; RING_CAP - 100][..]);
    assert_eq!(&flat[RING_CAP - 100..], &vec![b'b'; 100][..]);
}

/// Issue #857's `33;C`: Pi marks every streamed line with
/// `ESC ] 133;B BEL ESC ] 133;C BEL`, and a pane that streams long enough
/// has the cap fall through one of them sooner or later. A replay begins
/// in the client's ground state, so whatever tail the cut left in front
/// was painted as text — here `33;C`, next to the line it marked.
///
/// Every cut, at every byte of the line: the head has to land where a
/// fresh parser can start, which is a sequence's `ESC` or the first byte
/// of a whole character.
#[test]
fn ring_eviction_never_leaves_a_sequence_tail_or_half_a_character_in_front() {
    let line = "\x1b]133;B\x07\x1b]133;C\x07下一步。\r\n".as_bytes();
    // The two marks, then each character (the CJK ones three bytes wide),
    // then the end of the line, where the filler starts.
    let starts = [0, 8, 16, 19, 22, 25, 28, 29, 30];
    assert_eq!(*starts.last().unwrap(), line.len());
    for cut in 0..=line.len() {
        let mut ring = ReplayRing::new(ws(80, 24));
        ring.append(line);
        ring.append(&vec![b'.'; RING_CAP - line.len() + cut]);
        let head = *starts.iter().find(|&&s| s >= cut).unwrap();
        let flat = ring.flatten();
        assert_eq!(
            &flat[..line.len() - head],
            &line[head..],
            "a cut at byte {cut} must move the head on to byte {head}"
        );
        assert_eq!(ring.len, flat.len());
    }
}

/// Issue #857: the two sniffers that rewrite the stream before the ring
/// and the client see it must hand every byte that is not theirs on
/// unchanged, wherever a read splits it — a Pi frame (synchronized update,
/// line marks, CJK text, cursor-up rewrite) at every split point, and a
/// byte at a time. Both what the ring records and what the client is sent.
#[test]
fn out_of_band_stripping_passes_a_streamed_frame_through_every_split() {
    let frame = "\x1b[?2026h\x1b[2A\r\x1b[2K## 资源\r\n\x1b[2K\
                 \x1b]133;B\x07\x1b]133;C\x07下一步。\x1b[?2026l"
        .as_bytes();
    let writer: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(std::io::sink())));
    let run = |chunks: &[&[u8]]| {
        let mut clipboard = ClipboardSniffer::default();
        clipboard.set_controller(Some(1), true);
        let mut graphics = GraphicsSniffer::new_local(true);
        let (mut recorded, mut sent) = (Vec::new(), Vec::new());
        for chunk in chunks {
            let mut frames = Vec::new();
            let pass = lift_out_of_band(
                chunk,
                &mut clipboard,
                &mut graphics,
                true,
                &writer,
                &mut frames,
            );
            recorded.extend_from_slice(&pass);
            if frames.is_empty() {
                sent.extend_from_slice(&pass);
            }
            for f in frames {
                match f {
                    GraphicsFrame::Output(b) => sent.extend(b),
                    _ => panic!("no out-of-band frame in a plain stream"),
                }
            }
        }
        (recorded, sent)
    };
    for cut in 0..=frame.len() {
        let (recorded, sent) = run(&[&frame[..cut], &frame[cut..]]);
        assert_eq!(recorded, frame, "ring, split at byte {cut}");
        assert_eq!(sent, frame, "client, split at byte {cut}");
    }
    let bytewise: Vec<&[u8]> = frame.chunks(1).collect();
    assert_eq!(run(&bytewise), (frame.to_vec(), frame.to_vec()));
}

/// A string sequence the cap falls into is evicted up to its terminator
/// however far that is — the client would not have shown a byte of it —
/// and the state carries across appends, so a later eviction still knows
/// it is inside one.
#[test]
fn ring_eviction_carries_an_open_sequence_across_appends() {
    let mut ring = ReplayRing::new(ws(80, 24));
    ring.append(b"\x1b]52;c;");
    ring.append(&vec![b'Q'; RING_CAP - 7]);
    // Cuts two bytes into the OSC: the rest of it goes too.
    ring.append(b"\x07ok");
    assert!(ring.flatten().starts_with(b"ok"), "the payload is gone");
    assert_eq!(ring.len, 2);
}

#[test]
fn ring_giant_chunk_starts_at_a_sequence_boundary() {
    let mut ring = ReplayRing::new(ws(80, 24));
    let mut big = b"\x1b]133;C\x07".to_vec();
    big.extend(vec![b'x'; RING_CAP - 4]);
    ring.append(&big);
    let flat = ring.flatten();
    assert!(flat.iter().all(|&b| b == b'x'), "no `C BEL` left in front");
    assert_eq!(ring.len, RING_CAP - 4);
}

#[test]
fn ring_giant_chunk_keeps_tail() {
    let mut ring = ReplayRing::new(ws(100, 24));
    ring.append(b"seed");
    ring.resize(ws(80, 24));
    let mut big = vec![b'x'; RING_CAP];
    big.extend_from_slice(b"TAIL");
    ring.append(&big);
    assert_eq!(ring.len, RING_CAP);
    assert_eq!(ring.segments.len(), 1);
    assert_eq!(&ring.flatten()[RING_CAP - 4..], b"TAIL");
}

#[test]
fn ring_resize_splits_replay_into_geometry_segments() {
    let mut ring = ReplayRing::new(ws(100, 24));
    ring.append(b"wide bytes");
    ring.resize(ws(80, 24));
    ring.append(b"narrow bytes");

    let (tx, rx) = mpsc::channel();
    ring.replay(&mut |m| {
        let _ = tx.send(m);
    });
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(100, 24)));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"wide bytes"));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 24)));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"narrow bytes"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_seeded_ring_replays_the_old_screen_at_the_size_it_was_written() {
    use crate::daemon::scrollback::Segment;

    let mut ring = ReplayRing::seeded(
        vec![Segment {
            size: ws(100, 24),
            bytes: b"what the dead pane had on it".to_vec(),
        }],
        ws(80, 30),
    );
    ring.append(b"the new shell's prompt");

    let (tx, rx) = mpsc::channel();
    ring.replay(&mut |m| {
        let _ = tx.send(m);
    });
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(100, 24)));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"what the dead pane had on it"),
        "restored output has to be replayed at the width it was produced at, or the client \
         rewraps it to whatever the window happens to be now"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 30)));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"the new shell's prompt")
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn seeding_does_not_make_an_untouched_pane_look_like_it_produced_output() {
    use crate::daemon::scrollback::Segment;

    let ring = ReplayRing::seeded(
        vec![Segment {
            size: ws(80, 24),
            bytes: b"restored".to_vec(),
        }],
        ws(80, 24),
    );
    assert_eq!(
        ring.appended, 0,
        "the mark answers 'has this pane written anything since the last snapshot', and \
         bytes it was handed at birth are not an answer of yes"
    );
}

#[test]
fn a_handoff_keeps_the_modes_the_ring_dropped_off_its_front() {
    use crate::daemon::scrollback::Segment;

    let mut head = TerminalModes::new();
    head.feed(b"\x1b[?1049h\x1b[?2031h\x1b[>1u");
    let (ring, modes) = ReplayRing::adopted(
        vec![Segment {
            size: ws(80, 24),
            bytes: b"long after startup".to_vec(),
        }],
        ws(80, 24),
        head.restore_bytes().as_deref(),
    );

    assert!(modes.is_on(crate::term_modes::COLOR_SCHEME_UPDATES));
    let prelude = modes
        .replay_prelude(&ring.head_modes, &ring.modes())
        .expect("a prelude");
    let has = |needle: &[u8]| prelude.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"\x1b[?2031h"));
    assert!(
        has(b"\x1b[>1u"),
        "the keyboard flags come back from the head fold, so it has to cross the exec too"
    );
}

#[test]
fn an_empty_seed_leaves_an_ordinary_ring() {
    let mut ring = ReplayRing::seeded(Vec::new(), ws(80, 24));
    ring.append(b"output");
    assert_eq!(
        ring.segments.len(),
        1,
        "the ring always has exactly one tail"
    );
    assert_eq!(ring.flatten(), b"output");
}

#[cfg(unix)]
#[test]
fn a_descriptor_that_is_not_a_terminal_is_refused_rather_than_adopted() {
    use std::os::fd::AsRawFd as _;

    let file = std::fs::File::open("/dev/null").expect("open /dev/null");
    let err = AdoptedMaster::from_fd(file.as_raw_fd())
        .err()
        .expect("a plain file is not a pty");
    assert!(
        err.to_string().contains("not a terminal"),
        "the refusal was {err}"
    );
    // Refused means not taken: the caller still owns what it passed, and
    // dropping `file` here is what closes it. Adopting first and failing
    // later would have closed a descriptor belonging to someone else.
    drop(file);
}

#[cfg(unix)]
#[test]
fn an_adopted_master_goes_back_to_close_on_exec() {
    let pair = native_pty_system()
        .openpty(PtySize::default())
        .expect("open a pty");
    let fd = pair
        .master
        .as_raw_fd()
        .expect("a fresh master has a descriptor");
    // What crossing the exec leaves behind: `dup` hands out a descriptor
    // with close-on-exec clear, exactly like one that was deliberately
    // stripped to survive.
    let inherited = unsafe { libc::dup(fd) };
    assert!(inherited >= 0, "dup the master");
    let master = AdoptedMaster::from_fd(inherited).expect("a pty is adoptable");
    let flags = unsafe { libc::fcntl(inherited, libc::F_GETFD) };
    assert!(
        flags >= 0 && flags & libc::FD_CLOEXEC != 0,
        "an adopted master left inheritable ends up in every child this daemon spawns, \
         and a child holding it keeps the pty from ever hanging up"
    );
    drop(master);
}

#[test]
fn the_restore_preamble_hands_the_new_shell_a_terminal_it_can_use() {
    let bytes = restore_preamble(Some("this shell is new"));
    let text = String::from_utf8(bytes).expect("the preamble is text");
    // A snapshot is cut at the front, so it can begin inside anything: an
    // unterminated SGR run, a hidden cursor, an alternate screen whose exit
    // never came because the daemon died while a full-screen app was up.
    assert!(text.contains("\x1b[?1049l"), "leave any alternate screen");
    assert!(text.contains("\x1b[?25h"), "give the cursor back");
    assert!(text.contains("\x1b[?7h"), "put autowrap back");
    assert!(text.contains("\x1b[0m"), "drop any colour left mid-run");
    assert!(text.contains("this shell is new"));

    // #850: the same argument, for the modes that make the terminal talk
    // back. The snapshot ends inside a program that enabled mouse
    // reporting and was killed before it could disable it, so replaying it
    // leaves the emulator reporting every pointer move into a shell that
    // never asked — the line fills with SGR reports as long as the pointer
    // is over the pane. Focus reporting writes unprompted for the same
    // reason; bracketed paste and DECCKM mangle what the user types.
    for (seq, what) in [
        ("\x1b[?9l", "X10 mouse reporting"),
        ("\x1b[?1000l", "click reporting"),
        ("\x1b[?1002l", "cell-motion reporting"),
        ("\x1b[?1003l", "all-motion reporting"),
        ("\x1b[?1005l", "the UTF-8 mouse encoding"),
        ("\x1b[?1006l", "the SGR mouse encoding"),
        ("\x1b[?1015l", "the urxvt mouse encoding"),
        ("\x1b[?1016l", "the SGR-pixel mouse encoding"),
        ("\x1b[?1004l", "focus reporting"),
        ("\x1b[?2004l", "bracketed paste"),
        ("\x1b[?1l", "application cursor keys"),
        ("\x1b[=0;1u", "the kitty keyboard flags"),
    ] {
        assert!(
            text.contains(seq),
            "the preamble has to turn {what} off: the process that turned it \
             on was killed by a hangup with no grace period and never sent \
             its own reset, so the replay's last word on it is `on`"
        );
    }

    // Not reset: alternate scroll is on in a default terminal, so clearing
    // it would leave the pane further from the default than it started.
    assert!(
        !text.contains("\x1b[?1007l"),
        "?1007 defaults to on; resetting it walks away from the default"
    );
}

/// Whether the stream's last word on a private mode is `h`, `l`, or
/// nothing — which is all a client's emulator ends up remembering of it.
fn last_mode_switch(stream: &[u8], mode: &str) -> Option<u8> {
    let last = |needle: Vec<u8>| {
        stream
            .windows(needle.len())
            .rposition(|w| w == needle.as_slice())
    };
    let on = last(format!("\x1b[?{mode}h").into_bytes());
    let off = last(format!("\x1b[?{mode}l").into_bytes());
    match (on, off) {
        (Some(h), Some(l)) => Some(if h > l { b'h' } else { b'l' }),
        (Some(_), None) => Some(b'h'),
        (None, Some(_)) => Some(b'l'),
        (None, None) => None,
    }
}

/// Issue #850, along the whole chain rather than at the preamble alone: a
/// snapshot is a raw byte stream, it seeds the restored pane's ring
/// verbatim, and the client feeds it straight to its parser. So a snapshot
/// whose last word on mouse reporting is `h` puts the *client* into mouse
/// reporting, against a shell that never asked, and every pointer move
/// over the pane is typed into its line as an SGR report.
#[test]
fn a_ring_restored_from_a_full_screen_snapshot_ends_with_reporting_off() {
    let size = ws(80, 24);
    // What the photograph of a pane running `claude`, `vim` or `btop`
    // actually holds. There is no epilogue: `DaemonPane::kill` is a hangup
    // with no grace period, so the program never sent its own `?1002l`,
    // and the snapshot was taken before it could have anyway.
    let snapshot = crate::daemon::scrollback::Segment {
        size,
        bytes: b"\x1b[?1049h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h\
\x1b[?2004hframe one\x1b[Hframe two"
            .to_vec(),
    };
    let mut ring = ReplayRing::seeded(vec![snapshot], size);
    ring.append(&restore_preamble(Some("the shell below is new")));
    let replayed = ring.flatten();

    for mode in [
        "1000", "1002", "1003", "1005", "1006", "1004", "2004", "1049",
    ] {
        assert_eq!(
            last_mode_switch(&replayed, mode),
            Some(b'l'),
            "a client replaying this ring ends with ?{mode} still on, which is \
             what types SGR reports into the restored shell (#850)"
        );
    }
}

/// The ConPTY constraint, from the daemon's side. A pane whose shell runs
/// on a ConPTY must open with an empty viewport and the cursor at the
/// top-left, because that is the state conhost's own screen buffer starts
/// in and every row it names afterwards is counted from there. Restored
/// output left on screen shifts all of them, and the shell's first repaint
/// of the line being typed lands on the old text — see
/// [`SCROLL_RESTORED_AWAY`].
#[test]
fn the_preamble_clears_the_way_for_conpty_and_only_for_conpty() {
    let text = String::from_utf8(restore_preamble(Some("this shell is new"))).unwrap();
    if cfg!(windows) {
        assert!(
            text.ends_with("\x1b[2J\x1b[H"),
            "the restored screen has to be scrolled into history and the cursor \
             homed *last*, after the banner: anything printed afterwards would \
             take back the row conhost counts from. The preamble ends {:?}",
            &text[text.len().saturating_sub(16)..]
        );
    } else {
        assert!(
            !text.contains("\x1b[2J"),
            "on a real pty the shell positions itself relatively, so the screen \
             the user asked to have back stays where they can see it"
        );
    }
}

#[test]
fn a_restored_pane_reannounces_its_title() {
    let bytes = retitle("✳ fixing the switcher");
    assert_eq!(
        bytes, b"\x1b]0;\xe2\x9c\xb3 fixing the switcher\x07",
        "the OSC that set the title was trimmed out of the snapshot long ago; \
         the restore has to say it again or the tab comes back as the default name"
    );
}

#[test]
fn a_stored_title_cannot_smuggle_control_bytes_into_the_stream() {
    let bytes = retitle("evil\x07\x1b]0;title\x1b\\rest");
    assert_eq!(
        bytes, b"\x1b]0;evil]0;title\\rest\x07",
        "a BEL or ESC inside the title would terminate the OSC early and leak \
         the rest as input to the terminal"
    );
}

#[test]
fn a_banner_cannot_smuggle_extra_lines_into_the_pane() {
    let text = String::from_utf8(restore_preamble(Some("first\r\nsecond"))).unwrap();
    assert!(
        text.contains("first  second"),
        "the rule is drawn as one line, so the words placed in it stay on one line"
    );
    assert_eq!(
        text.matches("\r\n").count(),
        2,
        "one break before the rule and one after it, and no others"
    );
}

#[test]
fn a_pane_with_nothing_to_say_still_gets_the_resets() {
    for banner in [None, Some(""), Some("   ")] {
        let text = String::from_utf8(restore_preamble(banner)).unwrap();
    assert!(
        text.starts_with("\x1b[?1049l"),
        "the terminal still has to be handed back in a usable state"
    );
    assert!(
        !text.contains('\n'),
        "with no words there is no rule to draw, so no line is spent on one"
    );
}
}

#[test]
fn restore_preamble_resets_scroll_margins_and_origin_mode() {
let text = String::from_utf8(restore_preamble(Some("restored"))).unwrap();
let decstbm = text
    .find("\x1b[r")
    .expect("DECSTBM reset must be emitted in restore preamble");
let decom = text
    .find("\x1b[?6l")
    .expect("DECOM reset must be emitted in restore preamble");
if cfg!(windows) {
    let home = text.rfind("\x1b[H").unwrap();
    assert!(decstbm < home && decom < home);
} else {
    assert!(decstbm < text.len() && decom < text.len());
}
}

#[test]
fn ring_idle_resizes_collapse_and_replay_ends_at_current_size() {
    let mut ring = ReplayRing::new(ws(100, 24));
    ring.append(b"bytes");
    ring.resize(ws(100, 24));
    assert_eq!(ring.segments.len(), 1);
    ring.resize(ws(90, 24));
    ring.resize(ws(80, 30));
    assert_eq!(ring.segments.len(), 2);

    let (tx, rx) = mpsc::channel();
    ring.replay(&mut |m| {
        let _ = tx.send(m);
    });
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(100, 24)));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"bytes"));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 30)));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b.is_empty()));
    assert!(rx.try_recv().is_err());
}

#[test]
fn ring_eviction_drops_emptied_segments() {
    let mut ring = ReplayRing::new(ws(100, 24));
    ring.append(b"old");
    ring.resize(ws(80, 24));
    ring.append(&vec![b'n'; RING_CAP - 2]);
    assert_eq!(ring.segments.len(), 2, "two bytes of the old segment left");
    ring.append(b"nn");
    assert_eq!(ring.segments.len(), 1, "the emptied old segment is gone");
    assert_eq!(ring.len, RING_CAP);

    let (tx, rx) = mpsc::channel();
    ring.replay(&mut |m| {
        let _ = tx.send(m);
    });
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(s)) if s == ws(80, 24)));
}

#[test]
fn ring_caps_segment_count_by_merging_oldest() {
    let mut ring = ReplayRing::new(ws(100, 24));
    let rounds = MAX_RING_SEGMENTS + 10;
    for i in 0..rounds {
        ring.append(format!("seg{i:02} ").as_bytes());
        ring.resize(ws(101 + i as u16, 24));
    }
    assert_eq!(ring.segments.len(), MAX_RING_SEGMENTS);

    let flat = String::from_utf8(ring.flatten()).unwrap();
    let expect: String = (0..rounds).map(|i| format!("seg{i:02} ")).collect();
    assert_eq!(flat, expect);

    let head = ring.segments.front().unwrap();
    assert_eq!(head.size, ws(111, 24));
    assert!(
        String::from_utf8(head.to_vec())
            .unwrap()
            .ends_with("seg11 ")
    );
}

#[test]
fn sniff_osc7_cwd() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]7;file://host/Users/me/dev\x07");
    assert_eq!(sig.cwd, Some(PathBuf::from("/Users/me/dev")));
}

/// The tree's only source of the name a window actually shows on a tab. A
/// tokenizer that does not list 0 and 2 sniffs nothing here, and the
/// switcher goes back to calling every agent tab "Claude Code".
#[test]
fn sniff_osc_title() {
    let mut s = OscSniffer::new();
    assert_eq!(
        s.feed(b"\x1b]0;user@host:~/dev\x07").title.as_deref(),
        Some("user@host:~/dev")
    );
    assert_eq!(
        s.feed(b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\")
            .title
            .as_deref(),
        Some("✳ fixing the switcher")
    );
    // The last one in a read is the one that stuck.
    assert_eq!(
        s.feed(b"\x1b]0;first\x07\x1b]0;second\x07")
            .title
            .as_deref(),
        Some("second")
    );
    // An empty title is a reset, and has to be told apart from a read with
    // no title in it at all.
    assert_eq!(s.feed(b"\x1b]2;\x07").title.as_deref(), Some(""));
    assert_eq!(s.feed(b"plain output\r\n").title, None);
    // OSC 1 names the icon, which the window ignores; so do we.
    assert_eq!(s.feed(b"\x1b]1;icon\x07").title, None);

    let long = format!("\x1b]0;{}\x07", "t".repeat(MAX_OSC_TITLE + 10));
    assert_eq!(
        s.feed(long.as_bytes()).title.map(|t| t.chars().count()),
        Some(MAX_OSC_TITLE)
    );
}

/// Titles arrive interleaved with everything else in the same read, and the
/// tokenizer has one identifier filter for the lot: sniffing a title must
/// not cost a cwd or a prompt mark.
#[test]
fn a_title_does_not_swallow_the_other_marks_in_the_same_read() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]0;user@host:~/dev\x07\x1b]7;file://host/dev\x07\x1b]133;A\x07");
    assert_eq!(sig.title.as_deref(), Some("user@host:~/dev"));
    assert_eq!(sig.cwd, Some(PathBuf::from("/dev")));
    assert!(sig.shell.last().unwrap().at_prompt);
}

#[test]
fn a_window_reattaching_to_an_ssh_pane_learns_it_is_connected() {
    use crate::daemon::protocol::SshPhase;
    let mut st = test_state(true);
    st.ssh_phase = Some(SshPhase::Connected);
    let (tx, rx) = std::sync::mpsc::channel();
    replay_state(&st, &tx, false);
    drop(tx);
    assert!(
        rx.iter().any(|m| matches!(
            m,
            DaemonMsg::SshStatus {
                phase: SshPhase::Connected
            }
        )),
        "the replay must carry the connection phase"
    );
}

/// The client forgets its cwd on every `RemoteContext`, so a replay that
/// sent the far shell's directory first would have it wiped straight away.
#[test]
fn a_window_reattaching_to_an_ssh_pane_keeps_its_remote_directory() {
    let mut st = test_state(true);
    st.remote = Some(RemoteContext {
        kind: RemoteKind::NativeSsh,
        argv: Vec::new(),
        target: "alice@box".into(),
    });
    st.cwd = Some(PathBuf::from("/home/alice/my_service"));
    let (tx, rx) = std::sync::mpsc::channel();
    replay_state(&st, &tx, false);
    drop(tx);
    let order: Vec<&str> = rx
        .iter()
        .filter_map(|m| match m {
            DaemonMsg::RemoteContext(_) => Some("remote"),
            DaemonMsg::Cwd(_) => Some("cwd"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["remote", "cwd"]);
}

#[test]
fn a_title_is_kept_until_it_changes_and_a_reset_clears_it() {
    let mut st = test_state(true);
    apply_signals(
        &mut st,
        SniffSignals {
            title: Some("✳ fixing the switcher".into()),
            ..SniffSignals::default()
        },
    );
    assert_eq!(st.osc_title.as_deref(), Some("✳ fixing the switcher"));

    // A read with no title in it leaves the stored one alone.
    apply_signals(&mut st, SniffSignals::default());
    assert_eq!(st.osc_title.as_deref(), Some("✳ fixing the switcher"));

    apply_signals(
        &mut st,
        SniffSignals {
            title: Some(String::new()),
            ..SniffSignals::default()
        },
    );
    assert_eq!(st.osc_title, None, "an empty title clears, not blanks");
}

/// #889: the tab went on reading "✳ fixing the switcher" long after Claude
/// Code had exited and the pane was back at its own prompt, because
/// nothing in the pane path ever retired an OSC 0/2 — only another one
/// replaced it. The `D` mark says the command that wrote it is over.
#[test]
fn a_title_a_command_set_is_retired_when_that_command_finishes() {
    let mut st = test_state(true);
    let mut s = OscSniffer::new();

    let mut read = |st: &mut PaneState, bytes: &[u8]| apply_signals(st, s.feed(bytes));

    read(
        &mut st,
        b"\x1b]7;file://h/work/anterm\x07\x1b]133;A\x07\x1b]133;B\x07",
    );
    read(&mut st, b"\x1b]133;C;claude\x07");
    read(&mut st, b"\x1b]2;\xe2\x9c\xb3 fixing the switcher\x1b\\");
    assert_eq!(
        st.osc_title.as_deref(),
        Some("✳ fixing the switcher"),
        "a running command names the tab"
    );

    // The user's precmd chain can take hundreds of ms, which is why the
    // `D` emitter is prepended to it — so the mark routinely lands in a
    // read of its own, ahead of anything the next prompt writes.
    read(&mut st, b"\x1b]133;D;0\x07");
    assert_eq!(
        st.osc_title, None,
        "the program that wrote the title has exited"
    );
    assert_eq!(
        st.cwd.as_deref(),
        Some(std::path::Path::new("/work/anterm")),
        "and the rung below it in the label ladder still knows the place"
    );
}

/// The two cases the retirement must not touch: a program that is still
/// running, and a shell that titles its own prompt.
#[test]
fn a_running_program_and_a_shells_own_prompt_title_both_keep_theirs() {
    let mut st = test_state(true);
    let mut s = OscSniffer::new();

    apply_signals(&mut st, s.feed(b"\x1b]133;C;vim\x07\x1b]2;vim\x1b\\"));
    assert_eq!(
        st.osc_title.as_deref(),
        Some("vim"),
        "nothing said the command ended"
    );

    // A shell that re-titles itself does it between the `D` and the `A`
    // (anterm's zsh helper prepends the `D`; the PowerShell one titles in
    // its prompt function), so the title is the last word in the read.
    apply_signals(
        &mut st,
        s.feed(b"\x1b]133;D;0\x07\x1b]0;me@box:~/dev\x07\x1b]133;A\x07"),
    );
    assert_eq!(
        st.osc_title.as_deref(),
        Some("me@box:~/dev"),
        "the prompt's own title outranks the retirement it follows"
    );

    // And that prompt title is nobody's command to retire.
    apply_signals(&mut st, s.feed(b"\x1b]133;C;ls\x07\x1b]133;D;0\x07"));
    assert_eq!(st.osc_title.as_deref(), Some("me@box:~/dev"));
}

/// A pane with no shell integration never sees a mark, so nothing changes
/// for it: whatever it last called itself is all anterm has.
#[test]
fn a_pane_without_shell_integration_keeps_its_title() {
    let mut st = test_state(true);
    let mut s = OscSniffer::new();
    apply_signals(&mut st, s.feed(b"\x1b]0;user@host:~/dev\x07"));
    apply_signals(&mut st, s.feed(b"lots of output\r\n"));
    assert_eq!(st.osc_title.as_deref(), Some("user@host:~/dev"));
}

#[test]
fn sniff_osc133_prompt() {
    let mut s = OscSniffer::new();
    let b = s.feed(b"\x1b]133;B\x07");
    assert!(b.shell.last().unwrap().active);
    assert!(b.shell.last().unwrap().at_prompt);

    let c = s.feed(b"\x1b]133;C\x07");
    assert!(!c.shell.last().unwrap().at_prompt);

    let d = s.feed(b"\x1b]133;D;130\x07");
    assert!(d.shell.last().unwrap().at_prompt);
    assert_eq!(d.shell.last().unwrap().last_exit_code, Some(130));
}

#[test]
fn a_full_command_cycle_in_one_chunk_still_reports_leaving_the_prompt() {
    let mut s = OscSniffer::new();
    s.feed(b"\x1b]133;A\x07\x1b]133;B\x07");

    let sig =
        s.feed(b"\x1b]133;C;echo%20hi\x07hi\r\n\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07");
    let states: Vec<bool> = sig.shell.iter().map(|s| s.at_prompt).collect();
    assert_eq!(
        states,
        vec![false, true],
        "the chunk must report leaving the prompt and coming back, not just the end state"
    );
    assert_eq!(sig.shell.last().unwrap().last_exit_code, Some(0));
    assert_eq!(sig.shell.last().unwrap().command, None);
}

#[test]
fn marks_on_the_same_side_of_the_prompt_boundary_fold_into_one_state() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]133;D;3\x07\x1b]133;A\x07\x1b]133;B\x07");
    assert_eq!(sig.shell.len(), 1, "D/A/B is one at-prompt state");
    assert!(sig.shell[0].at_prompt);
    assert_eq!(sig.shell[0].last_exit_code, Some(3));
}

#[test]
fn sniff_osc133_command_capture_drives_agent_detection() {
    let custom = std::collections::HashMap::new();
    let mut s = OscSniffer::new();

    let c = s.feed(b"\x1b]133;C;claude%20--help\x07");
    let shell = c.shell.last().unwrap();
    assert!(!shell.at_prompt);
    assert_eq!(shell.command.as_deref(), Some("claude --help"));
    assert_eq!(
        agent_from_shell_mark(shell, &custom),
        Some((
            crate::cli_agent::CLIAgent::Claude,
            vec!["claude".to_string(), "--help".to_string()],
        ))
    );

    let d = s.feed(b"\x1b]133;D;0\x07");
    let shell = d.shell.last().unwrap();
    assert_eq!(shell.command, None);
    assert_eq!(agent_from_shell_mark(shell, &custom), None);

    let c = s.feed(b"\x1b]133;C;git%20status\x07");
    let shell = c.shell.last().unwrap();
    assert_eq!(shell.command.as_deref(), Some("git status"));
    assert_eq!(agent_from_shell_mark(shell, &custom), None);

    let c = s.feed(b"\x1b]133;C\x07");
    assert_eq!(c.shell.last().unwrap().command, None);

    let _ = s.feed(b"\x1b]133;C;codex\x07");
    let a = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    assert_eq!(a.shell.last().unwrap().command.as_deref(), Some("codex"));
    let d = s.feed(b"\x1b]133;D;0\x07");
    assert_eq!(d.shell.last().unwrap().command, None);

    let c = s.feed(b"\x1b]133;C;echo%20a%0Aecho%20b\x07");
    assert_eq!(
        c.shell.last().unwrap().command.as_deref(),
        Some("echo a\necho b")
    );

    let c = s.feed(b"\x1b]133;C;%20%20\x07");
    assert_eq!(c.shell.last().unwrap().command, None);
}

#[test]
fn shell_mark_agent_detection_keeps_launch_argv_for_resume() {
    let custom = std::collections::HashMap::new();
    let mut s = OscSniffer::new();

    let c = s.feed(b"\x1b]133;C;claude%20--dangerously-skip-permissions\x07");
    let (agent, argv) = agent_from_shell_mark(c.shell.last().unwrap(), &custom).unwrap();
    assert_eq!(agent, crate::cli_agent::CLIAgent::Claude);
    assert_eq!(
        agent.resume_command("abc-123", Some(&argv)).as_deref(),
        Some("claude --dangerously-skip-permissions --resume abc-123")
    );

    // Case must survive capture: a lowercased path or model name would
    // replay the wrong flags.
    let c = s.feed(b"\x1b]133;C;claude%20--model%20Opus\x07");
    let (agent, argv) = agent_from_shell_mark(c.shell.last().unwrap(), &custom).unwrap();
    assert_eq!(argv, ["claude", "--model", "Opus"]);
    assert_eq!(
        agent.resume_command("abc", Some(&argv)).as_deref(),
        Some("claude --model Opus --resume abc")
    );

    // PowerShell call-operator launches keep working end to end.
    let c = s.feed(b"\x1b]133;C;%26%20%22C%3A%5Ctools%5Cclaude.exe%22%20--continue\x07");
    let (agent, argv) = agent_from_shell_mark(c.shell.last().unwrap(), &custom).unwrap();
    assert_eq!(agent, crate::cli_agent::CLIAgent::Claude);
    assert_eq!(argv, [r"C:\tools\claude.exe", "--continue"]);
    assert_eq!(
        agent.resume_command("abc", Some(&argv)).as_deref(),
        Some("claude --resume abc"),
        "stale session flags are dropped, not replayed"
    );
}

#[test]
fn sniff_osc133_stray_marks_do_not_reapply_mark_detection() {
    let mut s = OscSniffer::new();

    let mut prev = ShellState::default();
    let c = s.feed(b"\x1b]133;C;.%5Cdev.ps1\x07").shell.pop().unwrap();
    assert!(shell_mark_capture_changed(&prev, &c));
    assert_eq!(
        agent_from_shell_mark(&c, &std::collections::HashMap::new()),
        None
    );
    prev = c;

    let ab = s.feed(b"\x1b]133;A\x1b]133;B\x07").shell.pop().unwrap();
    assert!(!shell_mark_capture_changed(&prev, &ab));
    prev = ab;

    let d = s.feed(b"\x1b]133;D;0\x07").shell.pop().unwrap();
    assert!(shell_mark_capture_changed(&prev, &d));
}

#[test]
fn sniff_osc133_edit_mode_does_not_emit_prompt_state() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]133;V;1\x07");
    assert!(
        sig.shell.is_empty(),
        "edit-mode metadata must not bump prompt state or prompt sequence"
    );

    let b = s.feed(b"\x1b]133;B\x07");
    assert!(b.shell.last().unwrap().active);
    assert!(b.shell.last().unwrap().at_prompt);
}

#[test]
fn foreground_command_distinguishes_the_shell_from_a_command() {
    assert!(!is_foreground_command(Some(1000), Some(1000)));
    assert!(is_foreground_command(Some(2000), Some(1000)));
    assert!(!is_foreground_command(None, Some(1000)));
    assert!(!is_foreground_command(Some(2000), None));
    assert!(!is_foreground_command(Some(0), Some(1000)));
}

#[test]
fn foreground_program_prompt_marks_do_not_claim_the_prompt() {
    let mut s = OscSniffer::new();
    let mut signals = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    assert!(
        signals.shell.last().unwrap().at_prompt,
        "the raw marks read as at-prompt"
    );

    let ssh_running = is_foreground_command(Some(2000), Some(1000));
    if signals.shell.iter().any(|st| st.at_prompt) && ssh_running {
        suppress_relayed_prompt_marks(&mut signals.shell);
    }
    assert!(
        !signals.shell.last().unwrap().at_prompt,
        "a foreground program's prompt marks must not engage the local editor"
    );
    assert!(
        signals.shell.last().unwrap().mark_at_prompt,
        "the mark's own reading survives: over `ssh` it is the far shell speaking, and \
         the only thing this side knows about whether it is busy (#840)"
    );

    let mut local = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    let shell_idle = is_foreground_command(Some(1000), Some(1000));
    if local.shell.iter().any(|st| st.at_prompt) && shell_idle {
        suppress_relayed_prompt_marks(&mut local.shell);
    }
    assert!(local.shell.last().unwrap().at_prompt);
}

/// The collapse that suppression performs must not hand the pane back a
/// prompt it has already left. A far shell's whole turn can arrive in one
/// read — the previous command's `D`, the prompt's `A`/`B`, then the next
/// command's `C` — and once `at_prompt` is cleared across the batch those
/// two entries differ only in the mark's own reading. The collapse keeps
/// the earlier entry, so that reading has to travel with it or freeness
/// answers `free` while a remote command is running (#840).
#[test]
fn suppression_collapses_a_batch_onto_its_newest_marks_reading() {
    let mut s = OscSniffer::new();
    // Bare `133;C`: nushell's integration emits it with no command name, as
    // do several third-party ones, so `command` cannot tell the two entries
    // apart either.
    let mut turn = s.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07");
    suppress_relayed_prompt_marks(&mut turn.shell);
    assert_eq!(
        turn.shell.len(),
        1,
        "still one `Prompt` message, exactly as before the mark was added"
    );
    assert!(
        !turn.shell.last().unwrap().mark_at_prompt,
        "the newest mark started a command, so the pane is not at a prompt"
    );

    // And the other order: a command that starts and finishes inside one
    // read has to end at the prompt, not at the `C` that opened it.
    let mut turn = s.feed(b"\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07");
    suppress_relayed_prompt_marks(&mut turn.shell);
    assert_eq!(turn.shell.len(), 1);
    assert!(
        turn.shell.last().unwrap().mark_at_prompt,
        "the newest mark is the prompt the far shell just drew"
    );
}

/// A prompt mark that arrives while the pane is pointed at a remote host
/// can only be the far shell's — the near one cannot be at a prompt while
/// the connection owns its pty. That is what proves the far side runs the
/// shell integration, and it is the difference between "the remote is
/// busy" and "we cannot tell" (#840).
#[test]
fn a_remote_panes_prompt_mark_latches_and_a_new_hop_clears_it() {
    let mark = |mark_at_prompt: bool, command: Option<&str>| ShellState {
        active: true,
        at_prompt: false,
        mark_at_prompt,
        last_exit_code: None,
        command: command.map(str::to_string),
    };
    let hop = |target: &str| RemoteContext {
        kind: RemoteKind::Ssh,
        argv: vec!["ssh".into(), target.into()],
        target: target.into(),
    };
    /// One pass of the reader's ordering: signals, then the hop the same
    /// read detected, then the latch.
    fn read(st: &mut PaneState, shell: Vec<ShellState>, remote: Option<Option<RemoteContext>>) {
        let saw_prompt_mark = shell.iter().any(|s| s.mark_at_prompt);
        apply_signals(
            st,
            SniffSignals {
                shell,
                ..SniffSignals::default()
            },
        );
        if let Some(remote) = remote {
            apply_remote_context(st, remote);
        }
        latch_remote_prompt(st, saw_prompt_mark);
    }

    // Local: the same mark proves nothing about any far side.
    let mut st = test_state(true);
    read(&mut st, vec![mark(true, None)], None);
    assert!(!st.remote_prompt_seen);

    // The near shell's own "I started `ssh`" — no prompt in it — must not
    // latch, even though the hop lands in the same read.
    read(
        &mut st,
        vec![mark(false, Some("ssh build-box"))],
        Some(Some(hop("build-box"))),
    );
    assert!(
        !st.remote_prompt_seen,
        "starting the connection is not evidence about what is behind it"
    );

    // The far shell's opening prompt does — including when it arrives in
    // the very read that first sees the pane as remote, which on a unix
    // host is the common case: the relayed prompt is suppressed, so it
    // cannot trigger the immediate re-probe that would have split the two.
    let mut fresh = test_state(true);
    read(
        &mut fresh,
        vec![mark(true, None)],
        Some(Some(hop("build-box"))),
    );
    assert!(fresh.remote_prompt_seen);
    read(&mut st, vec![mark(true, None)], None);
    assert!(st.remote_prompt_seen);

    // A second hop has to prove itself over again.
    read(&mut st, Vec::new(), Some(Some(hop("other-box"))));
    assert!(!st.remote_prompt_seen);

    // And coming back to the local shell clears it too.
    read(&mut st, vec![mark(true, None)], None);
    assert!(st.remote_prompt_seen);
    read(&mut st, Vec::new(), Some(None));
    assert!(!st.remote_prompt_seen);
}

#[test]
fn sniff_resyncs_on_new_osc_after_an_unterminated_one() {
    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]133;A\x1b]133;B\x07");
    assert!(
        sig.shell.last().is_some_and(|sh| sh.at_prompt),
        "OSC 133;B after an unterminated 133;A was dropped (no resync on `]`)"
    );

    let mut s = OscSniffer::new();
    let sig = s.feed(b"\x1b]7;file://host/dropped\x1b]7;file://host/kept\x07");
    assert_eq!(sig.cwd, Some(PathBuf::from("/kept")));
}

#[test]
fn at_prompt_covers_prompt_draw_gap_across_chunks() {
    let mut s = OscSniffer::new();

    assert!(!s.feed(b"\x1b]133;C\x07").shell.last().unwrap().at_prompt);

    let d = s.feed(b"\x1b]133;D;0\x07");
    assert!(
        d.shell.last().unwrap().at_prompt,
        "D should mark us back at the prompt before the prompt text is drawn"
    );

    let chunk = s.feed(
        b"\x1b]133;A\x07\x1b]7;file://host/repo/anterm\x07\r\nanterm git:(main) \xe2\x9e\x9c ",
    );
    assert!(
        chunk.shell.last().unwrap().at_prompt,
        "prompt visible but at_prompt=false — the mis-routing window is still open"
    );

    assert!(s.feed(b"\x1b]133;B\x07").shell.last().unwrap().at_prompt);
}

#[test]
fn pty_size_clamps_and_computes_pixels() {
    let ps = pty_size(WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 17,
    });
    assert_eq!(ps.rows, 24);
    assert_eq!(ps.cols, 80);
    assert_eq!(ps.pixel_width, 80 * 8);
    assert_eq!(ps.pixel_height, 24 * 17);

    let z = pty_size(WinSize {
        cols: 0,
        rows: 0,
        cell_w: 0,
        cell_h: 0,
    });
    assert_eq!(z.rows, 1);
    assert_eq!(z.cols, 1);
    assert_eq!(z.pixel_width, 0);
    assert_eq!(z.pixel_height, 0);

    let big = pty_size(WinSize {
        cols: u16::MAX,
        rows: u16::MAX,
        cell_w: u16::MAX,
        cell_h: u16::MAX,
    });
    assert_eq!(big.pixel_width, u16::MAX);
    assert_eq!(big.pixel_height, u16::MAX);
}

#[test]
fn parse_osc7_forms_and_rejections() {
    assert_eq!(
        parse_osc7(b"7;file://host/Users/me/dev"),
        Some(PathBuf::from("/Users/me/dev"))
    );
    assert_eq!(parse_osc7(b"7;file:///etc"), Some(PathBuf::from("/etc")));
    assert_eq!(parse_osc7(b"7;/var/log"), Some(PathBuf::from("/var/log")));
    assert_eq!(
        parse_osc7(b"7;file://host/a%20b"),
        Some(PathBuf::from("/a b"))
    );
    assert_eq!(
        parse_osc7(b"7;file://host/%E4%B8%AD%E6%96%87"),
        Some(PathBuf::from("/中文"))
    );
    assert_eq!(
        parse_osc7(b"7;file://host/tmp/a%2520b"),
        Some(PathBuf::from("/tmp/a%20b"))
    );
    assert!(parse_osc7(b"8;file://host/x").is_none());
    assert!(parse_osc7(b"7;file://host").is_none());
    assert!(parse_osc7(b"7;relative/path").is_none());
    assert!(parse_osc7(b"7;file://host").is_none());
}

#[test]
fn strip_uri_drive_slash_only_unwraps_drive_paths() {
    assert_eq!(strip_uri_drive_slash("/C:/Users/foo"), "C:/Users/foo");
    assert_eq!(strip_uri_drive_slash("/d:/x"), "d:/x");
    assert_eq!(strip_uri_drive_slash("/home/me/dev"), "/home/me/dev");
    assert_eq!(strip_uri_drive_slash("//host/share"), "//host/share");
    assert_eq!(strip_uri_drive_slash("C:/already"), "C:/already");
    assert_eq!(strip_uri_drive_slash("/"), "/");
}

#[test]
fn percent_decode_handles_escapes_and_garbage() {
    assert_eq!(percent_decode(b"a%20b"), b"a b");
    assert_eq!(percent_decode(b"%2F"), b"/");
    assert_eq!(percent_decode(b"%2f"), b"/");
    assert_eq!(percent_decode(b"%GG"), b"%GG");
    assert_eq!(percent_decode(b"x%2"), b"x%2");
    assert_eq!(percent_decode(b"plain"), b"plain");
}

#[test]
fn hex_val_ranges() {
    assert_eq!(hex_val(b'0'), Some(0));
    assert_eq!(hex_val(b'9'), Some(9));
    assert_eq!(hex_val(b'a'), Some(10));
    assert_eq!(hex_val(b'f'), Some(15));
    assert_eq!(hex_val(b'A'), Some(10));
    assert_eq!(hex_val(b'F'), Some(15));
    assert!(hex_val(b'g').is_none());
    assert!(hex_val(b' ').is_none());
    assert!(hex_val(b'/').is_none());
}

#[test]
fn osc133_exit_code_parsing() {
    let mut s = OscSniffer::new();
    let d = s.feed(b"\x1b]133;D\x07");
    assert!(d.shell.last().unwrap().at_prompt);
    assert_eq!(d.shell.last().unwrap().last_exit_code, None);

    let d = s.feed(b"\x1b]133;D;oops\x07");
    assert_eq!(d.shell.last().unwrap().last_exit_code, None);

    let d = s.feed(b"\x1b]133;D;-1\x07");
    assert_eq!(d.shell.last().unwrap().last_exit_code, Some(-1));
}

/// A discard writer for `spawn_reader` tests that don't inspect the PTY
/// write-back path (graphics query replies).
fn null_writer() -> Arc<Mutex<Box<dyn Write + Send>>> {
    Arc::new(Mutex::new(Box::new(std::io::sink())))
}

fn test_state(alive: bool) -> PaneState {
    PaneState {
        id: 0,
        ring: ReplayRing::new(ws(80, 24)),
        subscriber: None,
        subscriber_epoch: 0,
        allow_remote_clipboard_write: false,
        clipboard_write_from_spec: None,
        observers: Vec::new(),
        observer_seq: 0,
        lease: None,
        lease_watch: false,
        shell_spec: None,
        cwd: None,
        osc_title: None,
        shell: ShellState::default(),
        remote_prompt_seen: false,
        ssh_phase: None,
        modes: TerminalModes::default(),
        remote: None,
        agent: None,
        agent_session: None,
        agent_argv: None,
        agent_clock: AgentClock::default(),
        alive,
        exit_code: None,
        last_redraw_nudge: None,
    }
}

#[test]
fn interrupt_keys_are_recognised_in_every_encoding() {
    for key in [
        &b"\x1b"[..],
        b"\x03",
        b"\x1b[27u",
        b"\x1b[27;1u",
        b"\x1b[27;1:1u",
        b"\x1b[27;65u",
        b"\x1b[99;5u",
        b"\x1b[99;5:1u",
        b"\x1b[99:67;5u",
        b"\x1b[27;5;99~",
    ] {
        assert!(is_interrupt_key(key), "{key:?}");
    }
    for key in [
        &b"\x1b[A"[..],
        b"\x1b\x1b",
        b"\x1b[27;1:3u",
        b"\x1b[27;3u",
        b"\x1b[99u",
        b"\x1b[99;6u",
        b"abc\x03",
        b"\x1b[13u",
        b"q",
    ] {
        assert!(!is_interrupt_key(key), "{key:?}");
    }
}

/// Grok Build's Esc does not stop its turn; its Ctrl+C does.
#[test]
fn grok_builds_turn_is_stopped_by_ctrl_c_alone() {
    use crate::cli_agent::CLIAgent;
    let grok = Some(CLIAgent::Grok);
    assert!(!interrupts(grok, b"\x1b"));
    assert!(!interrupts(grok, b"\x1b[27;1:1u"));
    assert!(interrupts(grok, b"\x03"));
    assert!(interrupts(grok, b"\x1b[99;5u"));
    assert!(interrupts(Some(CLIAgent::Pi), b"\x1b"));
    assert!(interrupts(None, b"\x1b"));
}

fn working_session() -> crate::cli_agent::AgentSessionState {
    crate::cli_agent::AgentSessionState {
        status: crate::cli_agent::AgentStatus::Working,
        rich: true,
        ..Default::default()
    }
}

#[test]
fn an_interrupt_settles_only_if_nothing_spoke_after_it() {
    use crate::cli_agent::AgentStatus;

    let mut st = test_state(true);
    st.agent_session = Some(working_session());
    st.agent_clock.interrupt_pending = true;
    let armed = st.agent_clock.generation;
    settle_interrupt(&mut st, armed);
    let sess = st.agent_session.as_ref().unwrap();
    assert_eq!(sess.status, AgentStatus::Done);
    assert!(sess.inferred);
    assert!(!st.agent_clock.interrupt_pending);

    let mut st = test_state(true);
    st.agent_session = Some(working_session());
    let armed = st.agent_clock.generation;
    st.agent_clock.generation += 1;
    settle_interrupt(&mut st, armed);
    assert_eq!(
        st.agent_session.as_ref().unwrap().status,
        AgentStatus::Working,
        "a hook event after the key has the last word"
    );

    let mut st = test_state(false);
    st.agent_session = Some(working_session());
    let armed = st.agent_clock.generation;
    settle_interrupt(&mut st, armed);
    assert_eq!(
        st.agent_session.as_ref().unwrap().status,
        AgentStatus::Working,
        "a dead pane is left to its exit report"
    );
}

#[test]
fn an_interrupt_arms_once_and_only_mid_turn() {
    let state = Arc::new(Mutex::new(test_state(true)));
    arm_interrupt(&state);
    assert!(
        !state.lock().unwrap().agent_clock.interrupt_pending,
        "no agent"
    );

    state.lock().unwrap().agent_session = Some(crate::cli_agent::AgentSessionState {
        rich: false,
        ..working_session()
    });
    arm_interrupt(&state);
    assert!(
        !state.lock().unwrap().agent_clock.interrupt_pending,
        "a status without hooks behind it has no Stop to miss"
    );

    state.lock().unwrap().agent_session = Some(working_session());
    arm_interrupt(&state);
    assert!(state.lock().unwrap().agent_clock.interrupt_pending);
}

#[test]
fn a_turn_silent_past_the_limit_goes_idle() {
    use crate::cli_agent::AgentStatus;

    let now = std::time::Instant::now();
    let mut st = test_state(true);
    st.agent_session = Some(working_session());

    expire_stale_agent(&mut st, now);
    assert_eq!(
        st.agent_clock.last_event,
        Some(now),
        "a turn with no event time starts the clock rather than expiring"
    );
    expire_stale_agent(&mut st, now + AGENT_STALE_AFTER - Duration::from_secs(1));
    assert_eq!(
        st.agent_session.as_ref().unwrap().status,
        AgentStatus::Working
    );

    expire_stale_agent(&mut st, now + AGENT_STALE_AFTER);
    let sess = st.agent_session.as_ref().unwrap();
    assert_eq!(sess.status, AgentStatus::Idle);
    assert!(sess.inferred);
}

#[test]
fn hook_events_restart_the_agent_clock() {
    use crate::cli_agent::{AgentEvent, AgentEventKind};

    let mut st = test_state(true);
    let before = st.agent_clock.generation;
    apply_agent_signals(
        &mut st,
        vec![AgentEvent {
            agent: None,
            kind: AgentEventKind::PromptSubmit,
            session_id: None,
            message: None,
            cwd: None,
            source: None,
            readout: Default::default(),
        }],
        None,
    );
    assert_ne!(st.agent_clock.generation, before);
    assert!(st.agent_clock.last_event.is_some());
}

#[test]
fn a_status_that_leaves_and_returns_in_one_batch_is_restamped() {
    use crate::cli_agent::{AgentEvent, AgentEventKind, AgentSessionState, AgentStatus};

    let event = |kind| AgentEvent {
        agent: None,
        kind,
        session_id: None,
        message: None,
        cwd: None,
        source: None,
        readout: Default::default(),
    };
    let mut st = test_state(true);
    st.agent_session = Some(AgentSessionState {
        status: AgentStatus::Done,
        since_ms: Some(1),
        ..Default::default()
    });
    apply_agent_signals(
        &mut st,
        vec![
            event(AgentEventKind::PromptSubmit),
            event(AgentEventKind::Stop),
        ],
        None,
    );
    let sess = st.agent_session.as_ref().unwrap();
    assert_eq!(sess.status, AgentStatus::Done);
    assert!(
        sess.since_ms.is_some_and(|ms| ms > 1),
        "a new turn finished, so its Done is not the old one"
    );
}

#[test]
fn observed_facts_prefer_the_sessions_argv_and_carry_its_status() {
    use crate::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let mut st = test_state(true);
    assert_eq!(
        observed_facts(&st),
        ObservedFacts {
            cwd: None,
            osc_title: None,
            agent: None,
            shell: None,
        }
    );

    st.cwd = Some(PathBuf::from("/work/api"));
    st.agent = Some(CLIAgent::Claude);
    st.agent_argv = Some(vec!["claude".into()]);
    st.agent_session = Some(AgentSessionState {
        status: AgentStatus::Working,
        session_id: Some("sess-1".into()),
        launch_argv: Some(vec!["claude".into(), "--model".into(), "opus".into()]),
        ..Default::default()
    });

    let facts = observed_facts(&st);
    assert_eq!(facts.cwd.as_deref(), Some("/work/api"));
    let agent = facts.agent.expect("an agent in the foreground is a fact");
    assert_eq!(agent.agent, CLIAgent::Claude);
    assert_eq!(agent.session_id.as_deref(), Some("sess-1"));
    assert_eq!(
        agent.launch_argv.as_deref(),
        Some(&["claude".to_string(), "--model".into(), "opus".into()][..]),
        "the session's own argv outranks the identity poll's capture"
    );
    assert_eq!(agent.status, Some(AgentStatus::Working));

    st.agent_session = None;
    let facts = observed_facts(&st);
    assert_eq!(
        facts.agent.unwrap().launch_argv.as_deref(),
        Some(&["claude".to_string()][..])
    );
}

#[test]
fn a_pane_killed_with_its_agent_keeps_the_facts_a_resume_needs() {
    use crate::cli_agent::{AgentSessionState, CLIAgent};
    use crate::machine::{
        AgentFacts, MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
        withdraw_observations,
    };

    const PANE: u64 = 77;
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(MACHINE_FILE));
    store
        .tab_create(
            None,
            PaneSeed {
                pane: PANE,
                cwd: Some("/work/api".to_string()),
                ssh_spec: None,
                agent: Some(AgentFacts {
                    agent: CLIAgent::Claude,
                    session_id: Some("sess-1".to_string()),
                    launch_argv: Some(vec!["claude".to_string()]),
                    status: None,
                }),
                shell: None,
            },
            None,
            None,
        )
        .unwrap();
    publish_observations(&store);

    let run = |shutting_down: bool| {
        let mut state = test_state(true);
        state.id = PANE;
        state.agent = Some(CLIAgent::Claude);
        state.agent_session = Some(AgentSessionState {
            session_id: Some("sess-1".to_string()),
            ..Default::default()
        });
        DaemonPane::spawn_reader(
            Arc::new(Mutex::new(state)),
            Arc::new(AtomicBool::new(shutting_down)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(b"\x1b]133;D;0\x07".to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| Some(None)),
                cwd: Box::new(|| None),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();
    };

    run(true);
    let kept = store
        .pane(PANE)
        .expect("the record outlives the pane")
        .agent
        .expect("a teardown must not report the agent away");
    assert_eq!(kept.session_id.as_deref(), Some("sess-1"));

    run(false);
    assert!(
        store.pane(PANE).unwrap().agent.is_none(),
        "an agent that left a pane still in use is a fact, and clears"
    );

    withdraw_observations();
}

/// The whole path a title takes into the tree: sniffed out of the pane's
/// own output, kept on its state, and written to the record every other
/// viewer reads. Nothing else can name the tabs of a workspace this process
/// does not own — without this the switcher fell back to the agent, and
/// every tab of a workspace running one read "Claude Code".
#[test]
fn a_pane_title_reaches_the_machine_tree() {
    use crate::machine::{
        MACHINE_FILE, MachineStore, OBSERVE_SLOT, PaneSeed, publish_observations,
        withdraw_observations,
    };

    const PANE: u64 = 78;
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().unwrap();
    let store = MachineStore::open(dir.path().join(MACHINE_FILE));
    store
        .tab_create(None, PaneSeed::bare(PANE), None, None)
        .unwrap();
    publish_observations(&store);

    let run = |had: Option<&str>, output: &[u8]| {
        let mut state = test_state(true);
        state.id = PANE;
        state.osc_title = had.map(str::to_string);
        DaemonPane::spawn_reader(
            Arc::new(Mutex::new(state)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            Box::new(std::io::Cursor::new(output.to_vec())),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(|| None),
                remote: Box::new(|| None),
                agent: Box::new(|| Some(None)),
                cwd: Box::new(|| None),
            },
            Arc::new(DeathReporter::new(|| {})),
        )
        .join()
        .unwrap();
        store.pane(PANE).expect("the record was seeded").osc_title
    };

    assert_eq!(
        run(None, b"\x1b]0;\xe2\x9c\xb3 fixing the switcher\x07").as_deref(),
        Some("✳ fixing the switcher")
    );
    assert_eq!(
        run(Some("✳ fixing the switcher"), b"\x1b]2;\x07"),
        None,
        "a pane that resets its title clears the one on record"
    );

    withdraw_observations();
}

/// Claude Code ends its OSC 11 theme query with DA1 and waits for both.
/// Started in a pane no window shows (an agent woken at launch), it got
/// neither and never checked the theme again.
#[test]
fn a_detached_pane_answers_its_queries_and_an_attached_one_leaves_them_to_the_client() {
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let sink = Sink::default();
    let writer: Mutex<Box<dyn Write + Send>> = Mutex::new(Box::new(sink.clone()));
    let mut st = test_state(true);
    let da1 = detached_reply(Query::DeviceAttributes, false);
    let read = |st: &mut PaneState, bytes: &[u8]| {
        record_output(st, bytes);
        answer_queries(&writer, &detached_queries(st));
        std::mem::take(&mut *sink.0.lock().unwrap())
    };

    assert_eq!(
        read(&mut st, b"\x1b]11;?\x07\x1b[c\x1b[0c"),
        [da1, da1].concat()
    );
    assert!(read(&mut st, b"\x1b[").is_empty());
    assert_eq!(
        read(&mut st, b"c"),
        da1,
        "split across reads, answered once"
    );

    let (tx, _rx) = mpsc::channel();
    st.subscriber = Some(tx);
    assert!(
        read(&mut st, b"\x1b[c\x1b[?996n").is_empty(),
        "the client answers"
    );
    assert!(read(&mut st, b"\x1b[").is_empty());
    st.subscriber = None;
    assert_eq!(
        read(&mut st, b"c"),
        da1,
        "split across a detach, answered once"
    );
    assert!(read(&mut st, b"\x1b[").is_empty());
    let (tx, _rx) = mpsc::channel();
    st.subscriber = Some(tx);
    assert!(
        read(&mut st, b"c").is_empty(),
        "split across an attach, the client's"
    );
    assert!(
        st.ring.modes().take_queries().is_empty(),
        "a handoff refolding the ring does not ask again"
    );
}

#[test]
fn a_detached_colour_scheme_query_reports_the_theme_the_window_last_drew() {
    assert_eq!(detached_reply(Query::ColorScheme, true), b"\x1b[?997;1n");
    assert_eq!(detached_reply(Query::ColorScheme, false), b"\x1b[?997;2n");
}

#[test]
fn sentinel_events_drive_agent_session_state() {
    use crate::cli_agent::{AgentStatus, CLIAgent};

    let mut st = test_state(true);
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(tx);

    let mut sniffer = OscSniffer::new();
    let stream = concat!(
        "\x1b]777;notify;anterm://cli-agent;",
        r#"{"v":1,"agent":"claude","event":"session-start","session_id":"sid-9"}"#,
        "\x07",
        "\x1b]777;notify;anterm://cli-agent;",
        r#"{"v":1,"agent":"claude","event":"prompt-submit"}"#,
        "\x07",
    );
    apply_signals(&mut st, sniffer.feed(stream.as_bytes()));

    assert_eq!(st.agent, Some(CLIAgent::Claude));
    let sess = st.agent_session.clone().expect("session state exists");
    assert_eq!(sess.status, AgentStatus::Working);
    assert_eq!(sess.session_id.as_deref(), Some("sid-9"));
    assert!(sess.rich);

    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::Agent(Some(CLIAgent::Claude)))
    ));
    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::AgentStatus(Some(s))) if s.status == AgentStatus::Working
    ));

    let waiting = concat!(
        "\x1b]777;notify;anterm://cli-agent;",
        r#"{"event":"notification","message":"Claude needs your permission to use Bash"}"#,
        "\x07",
    );
    apply_signals(&mut st, sniffer.feed(waiting.as_bytes()));
    assert_eq!(
        st.agent_session.as_ref().unwrap().status,
        AgentStatus::Waiting
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::AgentStatus(Some(s))) if s.message.as_deref().unwrap().contains("permission")
    ));

    apply_agent(&mut st, None);
    assert!(st.agent_session.is_none());
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::AgentStatus(None))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Agent(None))));
}

#[test]
fn a_foreign_agent_hook_does_not_replace_the_foreground_session() {
    use crate::cli_agent::CLIAgent;

    let mut st = test_state(true);
    let mut sniffer = OscSniffer::new();
    let omp = concat!(
        "\x1b]777;notify;anterm://cli-agent;",
        r#"{"v":1,"agent":"omp","event":"session-start","session_id":"01a0d21f-2f79-72e0-bb16-d6d908aaa6e0"}"#,
        "\x07",
    );
    apply_signals(&mut st, sniffer.feed(omp.as_bytes()));
    assert_eq!(st.agent, Some(CLIAgent::OhMyPi));

    let claude = concat!(
        "\x1b]777;notify;anterm://cli-agent;",
        r#"{"v":1,"agent":"claude","event":"session-start","session_id":"e18a20ff-4c8c-4b94-867b-dd4b79032a6c"}"#,
        "\x07",
    );
    apply_signals(&mut st, sniffer.feed(claude.as_bytes()));
    assert_eq!(
        st.agent_session.as_ref().unwrap().session_id.as_deref(),
        Some("01a0d21f-2f79-72e0-bb16-d6d908aaa6e0"),
        "a Claude hook on an omp pane must not steal the session id"
    );

    apply_agent(&mut st, Some((CLIAgent::Claude, vec!["claude".into()])));
    assert!(
        st.agent_session.is_none(),
        "switching the foreground agent drops the previous session"
    );
}

#[test]
fn a_kitty_notification_marks_a_hookless_agent_waiting() {
    use crate::cli_agent::{AgentStatus, CLIAgent};

    let mut st = test_state(true);
    st.agent = Some(CLIAgent::Claude);
    let mut sniffer = OscSniffer::new();
    apply_signals(
        &mut st,
        sniffer.feed(
            b"\x1b]99;i=7:d=0:p=title;Claude Code\x07\x1b]99;i=7:p=body;Claude needs your permission\x07\x1b]99;i=7:d=1:a=focus;\x07",
        ),
    );
    let sess = st.agent_session.clone().unwrap();
    assert_eq!(sess.status, AgentStatus::Waiting);
    assert_eq!(
        sess.message.as_deref(),
        Some("Claude needs your permission")
    );
}

#[test]
fn opaque_notifications_only_fall_back_when_no_rich_state() {
    use crate::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let mut st = test_state(true);
    let mut sniffer = OscSniffer::new();
    apply_signals(&mut st, sniffer.feed(b"\x1b]9;Build finished\x07"));
    assert!(st.agent_session.is_none());

    st.agent = Some(CLIAgent::Codex);
    apply_signals(
        &mut st,
        sniffer.feed(b"\x1b]9;Codex wants to run tests\x07"),
    );
    let sess = st.agent_session.clone().unwrap();
    assert_eq!(sess.status, AgentStatus::Waiting);
    assert!(!sess.rich);

    st.agent_session = Some(AgentSessionState {
        status: AgentStatus::Working,
        message: None,
        session_id: Some("sid".into()),
        launch_argv: None,
        rich: true,
        cwd: None,
        activity: 0,
        turns: 0,
        inferred: false,
        readout: Default::default(),
        since_ms: None,
    });
    apply_signals(&mut st, sniffer.feed(b"\x1b]9;noise\x07"));
    assert_eq!(
        st.agent_session.as_ref().unwrap().status,
        AgentStatus::Working
    );
}

#[test]
fn probed_cwd_corrects_a_pane_whose_shell_never_reports_osc7() {
    let mut st = test_state(true);
    st.cwd = Some(PathBuf::from("/Users/alice"));
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(tx);

    apply_probed_cwd(&mut st, Some(PathBuf::from("/Users/alice/dev/anterm")));

    assert_eq!(st.cwd.as_deref(), Some(Path::new("/Users/alice/dev/anterm")));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/dev/anterm"))
    );
}

#[cfg(unix)]
#[test]
fn probed_cwd_keeps_the_shells_spelling_for_a_symlinked_path() {
    let tmp = std::env::temp_dir().join(format!("anterm-cwd-{}", std::process::id()));
    let real = tmp.join("real");
    let link = tmp.join("link");
    std::fs::create_dir_all(&real).unwrap();
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let mut st = test_state(true);
    st.cwd = Some(link.clone());
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(tx);

    apply_probed_cwd(&mut st, Some(real.canonicalize().unwrap()));

    assert_eq!(st.cwd.as_deref(), Some(link.as_path()));
    assert!(rx.try_recv().is_err(), "same directory → nothing to tell");

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn probed_cwd_matching_the_reported_one_says_nothing() {
    let mut st = test_state(true);
    st.cwd = Some(PathBuf::from("/Users/alice/dev/anterm"));
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(tx);

    apply_probed_cwd(&mut st, Some(PathBuf::from("/Users/alice/dev/anterm")));

    assert_eq!(st.cwd.as_deref(), Some(Path::new("/Users/alice/dev/anterm")));
    assert!(rx.try_recv().is_err());
}

#[test]
fn probed_cwd_declines_to_speak_for_a_remote_pane() {
    let mut st = test_state(true);
    st.remote = Some(RemoteContext {
        kind: RemoteKind::Ssh,
        argv: vec!["ssh".into(), "build-box".into()],
        target: "build-box".into(),
    });
    st.cwd = None;
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(tx);

    apply_probed_cwd(&mut st, Some(PathBuf::from("/Users/alice/dev/anterm")));

    assert_eq!(st.cwd, None);
    assert!(rx.try_recv().is_err());
}

#[test]
fn probed_cwd_absent_leaves_the_reported_cwd_alone() {
    let mut st = test_state(true);
    st.cwd = Some(PathBuf::from("/work"));
    let (tx, rx) = mpsc::channel();
    st.subscriber = Some(tx);

    apply_probed_cwd(&mut st, None);

    assert_eq!(st.cwd.as_deref(), Some(Path::new("/work")));
    assert!(rx.try_recv().is_err());
}

#[test]
fn attach_replays_state_in_order_and_installs_subscriber() {
    let mut st = test_state(true);
    st.ring.append(b"screen");
    st.cwd = Some(PathBuf::from("/work"));

    let (tx, rx) = mpsc::channel();
    let epoch = attach_subscriber(&mut st, tx);
    assert_eq!(epoch, 1);
    assert!(st.subscriber.is_some());
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"screen"));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p.as_path() == std::path::Path::new("/work"))
    );
    assert!(rx.try_recv().is_err());
}

/// Issue #711. A pane whose shell printed its prompt and then handed the
/// terminal to a full-screen program keeps `at_prompt` set for as long as
/// that program runs — nothing clears the flag but a `133;C` the program
/// has no reason to send. The live path already refuses to believe a
/// prompt mark that arrives while a foreground command owns the pty; the
/// replay used to re-assert the stored value regardless.
///
/// What the client does with "at a prompt" is scrub the TUI modes it finds
/// in its grid — the alternate screen first (`\x1b[?1049l`) — because a
/// prompting shell means no full-screen program can own the pane. On a
/// replay that alternate screen is the one the ring just rebuilt, so the
/// scrub swapped it away and left the primary screen underneath: the shell
/// banner and the command line the pane was born with, with the program
/// still painting differential updates into a grid that no longer holds
/// what they are differences from.
#[test]
fn a_replay_does_not_claim_a_prompt_while_a_program_owns_the_pane() {
    let mut st = test_state(true);
    // The shell prompted, then `claude` took the terminal and entered the
    // alternate screen. No `133;C` ever arrived, so the flag still stands.
    st.shell = ShellState {
        active: true,
        at_prompt: true,
        // The mark said the same thing; nothing has superseded it.
        mark_at_prompt: true,
        last_exit_code: Some(0),
        command: None,
    };
    st.ring.append(b"\x1b[?1049h\x1b[2Jthe agent's screen");

    let (tx, rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, true);

    let replayed = drain(&rx);
    let prompt = replayed
        .iter()
        .find_map(|msg| match msg {
            DaemonMsg::Prompt {
                active, at_prompt, ..
            } => Some((*active, *at_prompt)),
            _ => None,
        })
        .expect("shell integration is on, so the replay reports the prompt state");
    assert_eq!(
        prompt,
        (true, false),
        "a pane with a program in the foreground is not at a prompt, whatever the last \
         OSC 133 mark said; claiming otherwise costs the client the screen the ring just \
         replayed"
    );
}

/// The other half of the same gate, and the reason it is a gate rather
/// than a deletion: an `ssh` that died mid-`vim` leaves `?1049h` in the
/// ring with no `?1049l` behind it, and the host shell's next prompt is
/// the only thing that says the alternate screen is stale. With nothing in
/// the foreground the report goes out as it always did, and a re-attach
/// still heals that pane.
#[test]
fn a_replay_still_reports_a_prompt_when_the_shell_owns_the_pane() {
    let mut st = test_state(true);
    st.shell = ShellState {
        active: true,
        at_prompt: true,
        // The mark said the same thing; nothing has superseded it.
        mark_at_prompt: true,
        last_exit_code: Some(0),
        command: None,
    };
    st.ring.append(b"\x1b[?1049hstranded alt screen");

    let (tx, rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, false);

    assert!(
        drain(&rx).iter().any(|msg| matches!(
            msg,
            DaemonMsg::Prompt {
                active: true,
                at_prompt: true,
                ..
            }
        )),
        "with no foreground command the stored prompt state is the truth, and the client \
         needs it to leave a stranded alternate screen"
    );
}

#[test]
fn attach_replays_initial_cwd_even_before_shell_reports_osc7() {
    let mut st = test_state(true);
    st.cwd = Some(PathBuf::from("/Users/alice/clone/anterm"));

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/clone/anterm"))
    );
}

/// Issue #774: `btop` sends its alternate-screen and mouse-reporting
/// prefix once, when it starts, and then refreshes for hours. The ring
/// holds the last few megabytes of those refreshes and nothing of the
/// prefix, so a client that rebuilt its terminal from replayed bytes alone
/// came back on the primary screen with reporting off — and its wheel,
/// reading those modes, scrolled the scrollback of a screen that has none.
#[test]
fn attach_restores_modes_whose_bytes_the_ring_has_dropped() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h\x1b[?1002h\x1b[?1006h");
    // A long enough run of refreshes to push the prefix out of the front.
    record_output(&mut st, &vec![b'.'; RING_CAP]);
    assert!(
        !st.ring.flatten().windows(8).any(|w| w == b"\x1b[?1049h"),
        "the point of the test is that the prefix is gone from the ring"
    );

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    // Ahead of the replayed screen, so the frames land in the buffer the
    // modes put the client on.
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[?1049h\x1b[?1002h\x1b[?1006h"),
        "the modes the ring lost must be re-sent, in the order they were set"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
}

/// #1074: an agent CLI pushes its kitty keyboard flags once at startup,
/// and a long session pushes that push out of the ring. A client that
/// reattached without it sent Escape as a bare `ESC`.
///
/// The stack goes back as it stood at the front of the ring, not as it
/// ends: the pop and push the ring still holds act on it from there, and
/// stacking them on the final state would leave the client a level deeper
/// than the program.
#[test]
fn attach_restores_kitty_keyboard_flags_the_ring_has_dropped() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[>1u");
    record_output(&mut st, &vec![b'.'; RING_CAP]);
    record_output(&mut st, b"\x1b[<u\x1b[>1u");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[>1u"),
        "the push the ring lost must be re-sent ahead of it"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
}

#[test]
fn kitty_keyboard_flags_the_ring_still_holds_are_left_to_it() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[>1u");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))),
        "nothing goes ahead of a ring that carries the push itself"
    );
}

#[test]
fn a_pane_that_left_the_alternate_screen_restores_no_modes() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h\x1b[?1002hvim\x1b[?1002l\x1b[?1049l");
    record_output(&mut st, b"$ ");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    // Straight to the ring: a shell prompt is not owed a mode frame, and
    // sending one would put the pane on a screen it had left.
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(rx.try_recv().is_err());
}

/// The other side of the same coin, and the common case: reconnect a minute
/// after opening `vim` and the ring still holds the whole session, prefix
/// included. Re-sending the prefix then would turn the ring's own `?1049h`
/// into a no-op, so the shell scrollback the ring holds ahead of it would be
/// painted into the alternate screen — which keeps no history and is thrown
/// away when the program exits, leaving the user back on a blank primary
/// buffer instead of the prompt they left behind.
#[test]
fn a_prefix_the_ring_still_carries_is_left_to_the_ring() {
    let mut st = test_state(true);
    record_output(&mut st, b"$ vim notes.md\r\n");
    record_output(&mut st, b"\x1b[?1049h\x1b[?1002h\x1b[?1006hthe file\r\n");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);

    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))),
        "the ring speaks for its own modes; nothing goes ahead of it"
    );
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(rx.try_recv().is_err());
}

/// A mode the cap falls through is a mode the ring cannot set: the front
/// evicts the rest of the sequence too rather than replay its tail as
/// text, so the mode has to come from the fold.
#[test]
fn a_prefix_the_ring_cut_in_half_is_restored() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h");
    // Four bytes over the cap, so the cap falls just after the `ESC [ ? 1`
    // the sequence opens with, and the `049h` after it goes as well.
    record_output(&mut st, &vec![b'.'; RING_CAP - 4]);
    assert!(st.ring.flatten().iter().all(|&b| b == b'.'));

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);
    assert!(
        matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[?1049h"),
        "the bytes left in the ring put no client on the alternate screen"
    );
}

/// An observer joins mid-session too, and reads the same pane state.
#[test]
fn observers_are_told_the_pane_modes_as_well() {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h");
    record_output(&mut st, &vec![b'.'; RING_CAP]);

    let (tx, rx) = mpsc::channel();
    observe_subscriber(&mut st, tx, Arc::new(OutputGate::new()), false);
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"\x1b[?1049h"));
}

#[test]
fn attach_to_a_dead_pane_replays_exited() {
    let mut st = test_state(false);
    st.ring.append(b"final screen");

    let (tx, rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(rx.try_recv(), Ok(DaemonMsg::Snapshot(_))));
    assert!(matches!(
        rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
}

fn drain(rx: &mpsc::Receiver<DaemonMsg>) -> Vec<DaemonMsg> {
    let mut got = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        got.push(msg);
    }
    got
}

/// A controller that watches leases, and an observer: what each hears.
fn leased_rig() -> (
    PaneState,
    mpsc::Receiver<DaemonMsg>,
    u64,
    mpsc::Receiver<DaemonMsg>,
) {
    let mut st = test_state(true);
    let (controller_tx, controller_rx) = mpsc::channel();
    attach_subscriber(&mut st, controller_tx);
    st.lease_watch = true;
    let (observer_tx, observer_rx) = mpsc::channel();
    let phone = observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&controller_rx);
    drain(&observer_rx);
    (st, controller_rx, phone, observer_rx)
}

#[test]
fn a_lease_resizes_for_observers_and_only_names_the_holder_to_the_controller() {
    let (mut st, controller_rx, phone, observer_rx) = leased_rig();

    let pty = lease_take(&mut st, phone, ws(40, 30), "phone".into());
    assert_eq!(pty, Some(ws(40, 30)));
    assert_eq!(st.ring.size(), ws(40, 30), "a replay shows the leased size");
    assert_eq!(
        drain(&observer_rx),
        vec![
            DaemonMsg::Size(ws(40, 30)),
            DaemonMsg::Lease(Some("phone".into()))
        ]
    );
    assert_eq!(
        drain(&controller_rx),
        vec![DaemonMsg::Lease(Some("phone".into()))],
        "the controller keeps its grid: no Size, or it would resize back"
    );
}

#[test]
fn the_desk_resizing_under_a_lease_is_kept_for_later() {
    let (mut st, controller_rx, phone, observer_rx) = leased_rig();
    lease_take(&mut st, phone, ws(40, 30), "phone".into());
    drain(&controller_rx);
    drain(&observer_rx);

    assert_eq!(
        controller_resize(&mut st, ws(120, 40), false),
        None,
        "the pty stays put"
    );
    assert_eq!(st.ring.size(), ws(40, 30));
    assert_eq!(drain(&controller_rx), vec![DaemonMsg::Size(ws(120, 40))]);
    assert!(drain(&observer_rx).is_empty());

    // Letting go restores what the desk asked for last, not what it had.
    assert_eq!(lease_release(&mut st, phone), Some(ws(120, 40)));
    assert_eq!(st.ring.size(), ws(120, 40));
    assert_eq!(
        drain(&observer_rx),
        vec![DaemonMsg::Lease(None), DaemonMsg::Size(ws(120, 40))]
    );
    assert_eq!(
        drain(&controller_rx),
        vec![DaemonMsg::Lease(None), DaemonMsg::Size(ws(120, 40))]
    );
    assert!(st.lease.is_none());
}

#[test]
fn taking_back_ends_the_lease_and_restores_the_desk() {
    let (mut st, controller_rx, phone, observer_rx) = leased_rig();
    lease_take(&mut st, phone, ws(40, 30), "phone".into());
    drain(&controller_rx);
    drain(&observer_rx);

    assert_eq!(lease_end(&mut st), Some(ws(80, 24)));
    assert_eq!(st.ring.size(), ws(80, 24));
    assert_eq!(
        drain(&observer_rx),
        vec![DaemonMsg::Lease(None), DaemonMsg::Size(ws(80, 24))]
    );
    assert_eq!(lease_end(&mut st), None, "nothing left to end");
    // The observer that lost it cannot end a lease it no longer holds.
    assert_eq!(lease_release(&mut st, phone), None);
}

#[test]
fn controller_resize_skips_redundant_pty_resize_but_still_echoes_size() {
    let mut st = test_state(true);
    let (controller_tx, controller_rx) = mpsc::channel();
    attach_subscriber(&mut st, controller_tx);
    drain(&controller_rx);

    let initial = st.ring.size();
    assert_eq!(
        controller_resize(&mut st, initial, false),
        None,
        "identical geometry should not resize the pty again"
    );
    assert_eq!(
        drain(&controller_rx),
        vec![DaemonMsg::Size(initial)],
        "an attaching client still receives the Size echo"
    );

    // Cell metric changes matter on Unix (ws_xpixel/ws_ypixel) but are ignored
    // by Windows ConPTY's ResizePseudoConsole.
    let new_metrics = WinSize {
        cell_w: initial.cell_w + 2,
        cell_h: initial.cell_h + 2,
        ..initial
    };
    assert_eq!(
        controller_resize(&mut st, new_metrics, true),
        None,
        "ConPTY ignores cell pixel dimensions when cols and rows are unchanged"
    );
    assert_eq!(st.ring.size(), new_metrics);
    assert_eq!(drain(&controller_rx), vec![DaemonMsg::Size(new_metrics)]);

    let back_to_initial = initial;
    assert_eq!(
        controller_resize(&mut st, back_to_initial, false),
        Some(back_to_initial),
        "Unix PTYs and SSH forward cell pixel metric updates"
    );
    assert_eq!(drain(&controller_rx), vec![DaemonMsg::Size(back_to_initial)]);

    let taller = ws(initial.cols, initial.rows + 10);
    assert_eq!(controller_resize(&mut st, taller, true), Some(taller));
    assert_eq!(st.ring.size(), taller);
    assert_eq!(drain(&controller_rx), vec![DaemonMsg::Size(taller)]);
}

#[test]
fn a_second_observer_takes_over_and_the_desk_size_carries() {
    let (mut st, controller_rx, phone, observer_rx) = leased_rig();
    let (tablet_tx, tablet_rx) = mpsc::channel();
    let tablet = observe_subscriber(&mut st, tablet_tx, Arc::new(OutputGate::new()), false);
    drain(&tablet_rx);

    lease_take(&mut st, phone, ws(40, 30), "phone".into());
    lease_take(&mut st, tablet, ws(100, 50), "tablet".into());
    drain(&controller_rx);
    let heard = drain(&observer_rx);
    assert_eq!(heard.last(), Some(&DaemonMsg::Size(ws(100, 50))));
    assert!(
        heard.contains(&DaemonMsg::Lease(None)),
        "the phone is told it lost it"
    );

    assert_eq!(lease_release(&mut st, phone), None);
    assert_eq!(lease_release(&mut st, tablet), Some(ws(80, 24)));
}

#[test]
fn a_controller_that_did_not_ask_hears_no_lease() {
    let (mut st, controller_rx, phone, _observer_rx) = leased_rig();
    st.lease_watch = false;
    lease_take(&mut st, phone, ws(40, 30), "phone".into());
    assert!(drain(&controller_rx).is_empty());
    lease_end(&mut st);
    assert_eq!(drain(&controller_rx), vec![DaemonMsg::Size(ws(80, 24))]);
}

#[test]
fn a_new_controller_must_ask_again() {
    let (mut st, _controller_rx, _phone, _observer_rx) = leased_rig();
    let (tx, _rx) = mpsc::channel();
    attach_subscriber(&mut st, tx);
    assert!(!st.lease_watch);
}

#[test]
fn an_unknown_observer_cannot_take_a_lease() {
    let (mut st, _c, _phone, _o) = leased_rig();
    assert_eq!(lease_take(&mut st, 999, ws(40, 30), "ghost".into()), None);
    assert!(st.lease.is_none());
}

#[test]
fn observe_replays_state_without_displacing_the_controller() {
    let mut st = test_state(true);
    st.ring.append(b"screen");
    st.cwd = Some(PathBuf::from("/work"));

    let (controller_tx, controller_rx) = mpsc::channel();
    let epoch = attach_subscriber(&mut st, controller_tx);
    drain(&controller_rx);

    let (observer_tx, observer_rx) = mpsc::channel();
    let id = observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    assert_eq!(
        st.subscriber_epoch, epoch,
        "observing must not bump the controller epoch"
    );
    assert!(st.subscriber.is_some(), "the controller keeps its seat");

    assert!(matches!(observer_rx.try_recv(), Ok(DaemonMsg::Size(_))));
    assert!(matches!(observer_rx.try_recv(), Ok(DaemonMsg::Snapshot(b)) if b == b"screen"));
    assert!(
        matches!(observer_rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/work"))
    );
    assert!(
        controller_rx.try_recv().is_err(),
        "an observer joining must be invisible to the controller"
    );

    notify(&mut st, DaemonMsg::Output(b"tick".to_vec()));
    assert!(matches!(controller_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tick"));
    assert!(matches!(observer_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tick"));

    st.observers.retain(|obs| obs.id != id);
    notify(&mut st, DaemonMsg::Output(b"tock".to_vec()));
    assert!(matches!(controller_rx.try_recv(), Ok(DaemonMsg::Output(_))));
    assert!(
        observer_rx.try_recv().is_err(),
        "a departed observer hears nothing"
    );
}

#[test]
fn a_new_controller_preempts_the_old_while_observers_survive() {
    let mut st = test_state(true);

    let (first_tx, first_rx) = mpsc::channel();
    let first_epoch = attach_subscriber(&mut st, first_tx);
    drain(&first_rx);

    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&observer_rx);

    let (second_tx, second_rx) = mpsc::channel();
    let second_epoch = attach_subscriber(&mut st, second_tx);
    assert!(second_epoch > first_epoch);
    drain(&second_rx);

    notify(&mut st, DaemonMsg::Output(b"live".to_vec()));
    assert!(
        matches!(first_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
        "the preempted controller's channel must be gone, exactly as before"
    );
    assert!(matches!(second_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"live"));
    assert!(
        matches!(observer_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"live"),
        "a controller handover must not evict read-only observers"
    );

    if st.subscriber_epoch == first_epoch {
        st.subscriber = None;
    }
    assert!(
        st.subscriber.is_some(),
        "a stale epoch's detach must not unseat the new controller"
    );
}

#[test]
fn a_gone_observer_is_pruned_on_the_next_broadcast() {
    let mut st = test_state(true);
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drop(observer_rx);

    notify(&mut st, DaemonMsg::Output(b"x".to_vec()));
    assert!(
        st.observers.is_empty(),
        "a dead observer must not accumulate"
    );
}

#[test]
fn a_stalled_observer_is_dropped_at_its_budget_while_the_controller_streams_on() {
    let mut st = test_state(true);
    let (controller_tx, controller_rx) = mpsc::channel();
    attach_subscriber(&mut st, controller_tx);
    drain(&controller_rx);

    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&observer_rx);

    let pane_gate = OutputGate::new();
    let chunk = vec![b'x'; 1024 * 1024];
    let sends = (OBSERVER_BUDGET / chunk.len() as i64) as usize + 2;
    for _ in 0..sends {
        fan_out_output(&mut st, &chunk, Vec::new(), &pane_gate);
        pane_gate.sub(chunk.len());
    }

    assert!(
        st.observers.is_empty(),
        "an observer past its budget must be pruned"
    );
    let mut controller_bytes = 0usize;
    while let Ok(DaemonMsg::Output(b)) = controller_rx.try_recv() {
        controller_bytes += b.len();
    }
    assert_eq!(
        controller_bytes,
        sends * chunk.len(),
        "the controller stream must stay complete"
    );
    let mut observer_bytes = 0i64;
    while let Ok(DaemonMsg::Output(b)) = observer_rx.try_recv() {
        observer_bytes += b.len() as i64;
    }
    assert!(
        observer_bytes <= OBSERVER_BUDGET,
        "a stalled observer must never hold more than its budget, held {observer_bytes}"
    );
}

/// A phone opening a pane with a long history: the replay is a full ring,
/// which is the whole observer budget. Once the writer has put it on the
/// wire, live output must still reach the observer.
/// The pane keeps printing while a full ring's replay is still going out
/// to the phone; that output is the observer's too.
#[test]
fn an_observer_still_reading_a_full_ring_replay_gets_live_output() {
    let mut st = test_state(true);
    st.ring.append(&vec![b'h'; RING_CAP]);
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);

    fan_out_output(&mut st, b"tick", Vec::new(), &OutputGate::new());
    assert_eq!(st.observers.len(), 1, "the observer is still subscribed");
    assert!(
        observer_rx
            .try_iter()
            .any(|m| matches!(m, DaemonMsg::Output(b) if b == b"tick"))
    );
}

/// One dropped at its budget hears why, and then its channel closes, so
/// its stream ends rather than going quiet for good.
#[test]
fn an_observer_dropped_at_its_budget_is_told_and_let_go() {
    let mut st = test_state(true);
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    drain(&observer_rx);

    let chunk = vec![b'x'; 1024 * 1024];
    while !st.observers.is_empty() {
        fan_out_output(&mut st, &chunk, Vec::new(), &OutputGate::new());
    }
    let last = observer_rx.try_iter().last();
    assert!(matches!(last, Some(DaemonMsg::Error(m)) if m == OBSERVER_BEHIND));
    assert!(matches!(
        observer_rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[cfg(unix)]
#[test]
fn an_observer_that_drained_a_full_ring_replay_keeps_getting_output() {
    use std::os::unix::net::UnixStream;
    let mut st = test_state(true);
    st.ring.append(&vec![b'h'; RING_CAP]);
    let (observer_tx, observer_rx) = mpsc::channel();
    let observer_gate = Arc::new(OutputGate::new());
    observe_subscriber(&mut st, observer_tx, observer_gate.clone(), false);

    // The daemon's own writer puts the replay on the wire, as for a phone.
    let (mut client, server) = UnixStream::pair().unwrap();
    let writer =
        crate::daemon::server::spawn_writer(observer_rx, server, observer_gate.clone());
    let mut replayed = 0;
    while replayed < RING_CAP {
        if let DaemonMsg::Snapshot(b) = DaemonMsg::read(&mut client).unwrap() {
            replayed += b.len();
        }
    }
    let paid = std::time::Instant::now() + Duration::from_secs(5);
    while observer_gate.queued_bytes() > 0 && std::time::Instant::now() < paid {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(observer_gate.queued_bytes(), 0, "the replay is paid off");

    let pane_gate = OutputGate::new();
    fan_out_output(&mut st, b"live", Vec::new(), &pane_gate);
    assert_eq!(st.observers.len(), 1, "the observer is still subscribed");
    assert_eq!(
        DaemonMsg::read(&mut client).unwrap(),
        DaemonMsg::Output(b"live".to_vec())
    );
    st.observers.clear();
    writer.join().unwrap();
}

/// A controller's gate starts at its replay, so crediting that replay as
/// it is written brings the gate back to zero rather than below it.
#[test]
fn a_controller_gate_is_charged_for_its_replay() {
    let mut st = test_state(true);
    st.ring.append(b"screen");
    let gate = OutputGate::new();
    gate.add(123);
    let (tx, rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &gate, false, false);
    assert_eq!(gate.queued_bytes(), b"screen".len() as i64);
    while let Ok(msg) = rx.try_recv() {
        gate.sub(gated_len(&msg));
    }
    assert_eq!(gate.queued_bytes(), 0);
}

#[test]
fn a_draining_observer_under_the_cap_stays_subscribed() {
    let mut st = test_state(true);
    let (observer_tx, observer_rx) = mpsc::channel();
    let observer_gate = Arc::new(OutputGate::new());
    observe_subscriber(&mut st, observer_tx, observer_gate.clone(), false);
    drain(&observer_rx);

    let pane_gate = OutputGate::new();
    let chunk = vec![b'y'; 1024 * 1024];
    let sends = (OBSERVER_BUDGET / chunk.len() as i64) as usize * 3;
    let mut got = 0usize;
    for _ in 0..sends {
        fan_out_output(&mut st, &chunk, Vec::new(), &pane_gate);
        pane_gate.sub(chunk.len());
        while let Ok(DaemonMsg::Output(b)) = observer_rx.try_recv() {
            observer_gate.sub(b.len());
            got += b.len();
        }
    }
    assert_eq!(
        st.observers.len(),
        1,
        "an observer that keeps draining must stay subscribed"
    );
    assert_eq!(got, sends * chunk.len(), "and must miss no bytes");
}

#[test]
fn resize_echoes_size_to_the_controller_between_old_and_new_output() {
    let mut st = test_state(true);
    let (controller_tx, controller_rx) = mpsc::channel();
    attach_subscriber(&mut st, controller_tx);
    drain(&controller_rx);

    let pane_gate = OutputGate::new();
    fan_out_output(&mut st, b"old-width bytes", Vec::new(), &pane_gate);
    resize_state(&mut st, ws(120, 30));
    fan_out_output(&mut st, b"new-width bytes", Vec::new(), &pane_gate);

    match controller_rx.try_recv().unwrap() {
        DaemonMsg::Output(b) => assert_eq!(b, b"old-width bytes"),
        other => panic!("expected the pre-resize Output first, got {other:?}"),
    }
    match controller_rx.try_recv().unwrap() {
        DaemonMsg::Size(size) => assert_eq!((size.cols, size.rows), (120, 30)),
        other => panic!("expected the Size echo in stream order, got {other:?}"),
    }
    match controller_rx.try_recv().unwrap() {
        DaemonMsg::Output(b) => assert_eq!(b, b"new-width bytes"),
        other => panic!("expected the post-resize Output last, got {other:?}"),
    }
}

#[test]
fn death_notifies_observers_but_only_controllers_defer_the_reap() {
    let with_observer_only = Arc::new(Mutex::new(test_state(true)));
    let (observer_tx, observer_rx) = mpsc::channel();
    observe_subscriber(
        &mut with_observer_only.lock().unwrap(),
        observer_tx,
        Arc::new(OutputGate::new()),
        false,
    );
    drain(&observer_rx);
    let (dead_tx, dead_rx) = mpsc::channel();
    DeathReporter::new(move || dead_tx.send(()).unwrap())
        .report(&with_observer_only, &AtomicBool::new(false));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        dead_rx.try_recv().is_ok(),
        "read-only observers must not keep a dead pane in the registry"
    );

    let with_both = Arc::new(Mutex::new(test_state(true)));
    let (controller_tx, controller_rx) = mpsc::channel();
    let (observer_tx, observer_rx) = mpsc::channel();
    {
        let mut st = with_both.lock().unwrap();
        attach_subscriber(&mut st, controller_tx);
        observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    }
    drain(&controller_rx);
    drain(&observer_rx);
    let (dead_tx, dead_rx) = mpsc::channel();
    DeathReporter::new(move || dead_tx.send(()).unwrap())
        .report(&with_both, &AtomicBool::new(false));
    assert!(matches!(
        controller_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        dead_rx.try_recv().is_err(),
        "an attached death is still the detach path's to reclaim"
    );
}

#[test]
fn agent_state_snapshot_reports_only_panes_with_a_session() {
    use crate::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let mut st = test_state(true);
    st.id = 42;
    assert_eq!(agent_state_snapshot(&st), None);

    st.agent = Some(CLIAgent::Claude);
    assert_eq!(
        agent_state_snapshot(&st),
        None,
        "a detected agent without session state is not yet a fact worth listing"
    );

    st.agent_session = Some(AgentSessionState {
        status: AgentStatus::Waiting,
        session_id: Some("sess-1".into()),
        ..Default::default()
    });
    let snap = agent_state_snapshot(&st).expect("a session state is the fact");
    assert_eq!(snap.pane_id, 42);
    assert_eq!(snap.agent, Some(CLIAgent::Claude));
    assert_eq!(snap.state.status, AgentStatus::Waiting);
    assert_eq!(snap.state.session_id.as_deref(), Some("sess-1"));
}

/// The foreground probe only ever runs on output, and the prompt a shell
/// draws after a command is the last output a pane produces until the user
/// types again. So an `ssh` that exited inside the poll interval used to
/// leave the pane reporting itself as remote indefinitely: nothing came
/// along to probe on. A prompt mark now forces the probe (#817).
#[test]
fn a_prompt_mark_reprobes_the_foreground_inside_the_poll_interval() {
    /// Hands the reader one chunk per `read`, so two prompt marks arrive as
    /// two passes through the loop a few microseconds apart — well inside
    /// `REMOTE_CONTEXT_POLL_INTERVAL`.
    struct Chunks(std::collections::VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                Some(chunk) => {
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    Ok(chunk.len())
                }
                None => Ok(0),
            }
        }
    }

    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);

    let probes_taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let taken = probes_taken.clone();
    let remote = Box::new(move || {
        // First probe: `ssh` holds the pty. Every one after: it is gone.
        (taken.fetch_add(1, Ordering::SeqCst) == 0).then(|| RemoteContext {
            kind: RemoteKind::Ssh,
            argv: vec!["ssh".into(), "box".into()],
            target: "box".into(),
        })
    });

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(Chunks(
            [
                b"\x1b]133;C;ssh box\x07".to_vec(),
                b"\x1b]133;D;0\x07".to_vec(),
            ]
            .into_iter()
            .collect(),
        )),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote,
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    handle.join().unwrap();

    assert_eq!(
        probes_taken.load(Ordering::SeqCst),
        2,
        "the prompt mark did not force a second probe"
    );
    assert!(
        state.lock().unwrap().remote.is_none(),
        "the pane still reports the ssh session it has already left"
    );
    let reported: Vec<Option<String>> = sub_rx
        .try_iter()
        .filter_map(|msg| match msg {
            DaemonMsg::RemoteContext(ctx) => Some(ctx.map(|c| c.target)),
            _ => None,
        })
        .collect();
    assert_eq!(
        reported,
        vec![Some("box".to_string()), None],
        "the client was never told the pane came home"
    );
}

#[test]
fn reader_eof_with_subscriber_sends_exited_not_on_dead() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);
    let dead = Arc::new(AtomicBool::new(false));
    let dead_flag = dead.clone();

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(b"tail".to_vec())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(move || {
            dead_flag.store(true, Ordering::SeqCst)
        })),
    );
    handle.join().unwrap();

    assert!(!state.lock().unwrap().alive);
    assert_eq!(state.lock().unwrap().ring.flatten(), b"tail");
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"tail"));
    assert!(matches!(
        sub_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        !dead.load(Ordering::SeqCst),
        "an attached death is the detach path's to reclaim, not on_dead's"
    );
}

#[cfg(windows)]
#[test]
fn closing_conpty_waits_for_delayed_reader_output_before_exit() {
    /// A master whose destruction opens the simulated ConPTY output pipe.
    /// The production `ConPtyMasterPty` performs the equivalent transition
    /// by calling `ClosePseudoConsole` from its destructor.
    struct SignallingMaster {
        released: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Drop for SignallingMaster {
        fn drop(&mut self) {
            let (lock, ready) = &*self.released;
            *lock.lock().unwrap() = true;
            ready.notify_all();
        }
    }

    impl MasterPty for SignallingMaster {
        fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_size(&self) -> anyhow::Result<PtySize> {
            Ok(PtySize::default())
        }

        fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
            Ok(Box::new(std::io::sink()))
        }
    }

    /// Models a reader that stays delayed well past any plausible grace
    /// period, then receives a final output chunk followed by EOF.
    struct DelayedTailReader {
        released: Arc<(Mutex<bool>, Condvar)>,
        tail: std::io::Cursor<Vec<u8>>,
        delayed: bool,
    }

    impl Read for DelayedTailReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.delayed {
                let (lock, ready) = &*self.released;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = ready.wait(released).unwrap();
                }
                drop(released);
                std::thread::sleep(Duration::from_millis(800));
                self.delayed = true;
            }
            self.tail.read(buf)
        }
    }

    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let master: Mutex<Option<Box<dyn MasterPty + Send>>> =
        Mutex::new(Some(Box::new(SignallingMaster {
            released: released.clone(),
        })));
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);

    let reader = DaemonPane::spawn_reader(
        state,
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(DelayedTailReader {
            released,
            tail: std::io::Cursor::new(b"final output".to_vec()),
            delayed: false,
        }),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(|| {})),
    );

    close_pty_master(&master);
    assert!(
        sub_rx.recv_timeout(Duration::from_millis(600)).is_err(),
        "closing the master must not publish Exited while the reader is still delayed"
    );
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Output(bytes)) if bytes == b"final output"
    ));
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(sub_rx.try_recv().is_err(), "no output may follow Exited");
    reader.join().unwrap();
}

/// An inert stand-in for the ConPTY master: releasing it is instant, which
/// is what `ClosePseudoConsole` does once the pipe has no pending bytes.
#[cfg(windows)]
struct InertMaster;

#[cfg(windows)]
impl MasterPty for InertMaster {
    fn resize(&self, _size: PtySize) -> anyhow::Result<()> {
        Ok(())
    }

    fn get_size(&self) -> anyhow::Result<PtySize> {
        Ok(PtySize::default())
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn Read + Send>> {
        Ok(Box::new(std::io::empty()))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn Write + Send>> {
        Ok(Box::new(std::io::sink()))
    }
}

#[cfg(windows)]
fn exit_drain_fixture() -> (
    Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    Arc<Mutex<PaneState>>,
    Arc<AtomicBool>,
    Arc<DeathReporter>,
    mpsc::Receiver<DaemonMsg>,
) {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);
    (
        Arc::new(Mutex::new(Some(
            Box::new(InertMaster) as Box<dyn MasterPty + Send>
        ))),
        state,
        Arc::new(AtomicBool::new(false)),
        Arc::new(DeathReporter::new(|| {})),
        sub_rx,
    )
}

/// A grandchild that inherited the ConPTY output pipe keeps it open after
/// the shell is gone, so the reader never sees EOF and can never publish
/// the exit. The monitor's drain window is the only thing that stops such
/// a pane from reading as alive forever.
#[cfg(windows)]
#[test]
fn a_pty_that_never_reaches_eof_still_reports_the_exit() {
    struct NeverEofReader;

    impl Read for NeverEofReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            // Long enough to outlast the window below, bounded so the
            // thread cannot outlive the test binary.
            std::thread::sleep(Duration::from_secs(5));
            Ok(0)
        }
    }

    let (master, state, shutting_down, death, sub_rx) = exit_drain_fixture();
    let _reader = DaemonPane::spawn_reader(
        state.clone(),
        shutting_down.clone(),
        Arc::new(OutputGate::new()),
        Box::new(NeverEofReader),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        death.clone(),
    );

    drain_then_report(
        master,
        state,
        shutting_down,
        death,
        Duration::from_millis(50),
    );
    assert!(
        matches!(
            sub_rx.recv_timeout(Duration::from_secs(1)),
            Ok(DaemonMsg::Exited { code: None })
        ),
        "the monitor must report the exit once its drain window elapses"
    );
}

/// The ordinary case: EOF arrives, the reader reports, and the monitor
/// neither waits out its window nor publishes a second `Exited`.
#[cfg(windows)]
#[test]
fn the_exit_monitor_defers_to_the_reader_that_saw_eof() {
    let (master, state, shutting_down, death, sub_rx) = exit_drain_fixture();
    let reader = DaemonPane::spawn_reader(
        state.clone(),
        shutting_down.clone(),
        Arc::new(OutputGate::new()),
        Box::new(std::io::empty()),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        death.clone(),
    );

    let started = std::time::Instant::now();
    drain_then_report(master, state, shutting_down, death, Duration::from_secs(30));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the monitor must stop waiting as soon as the reader has reported"
    );
    assert!(matches!(
        sub_rx.recv_timeout(Duration::from_secs(1)),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        sub_rx.try_recv().is_err(),
        "the idempotent reporter must not publish a second Exited"
    );
    reader.join().unwrap();
}

/// Issue #213 end-to-end at the reader: a chunk carrying text plus a
/// kitty graphics query and a transmit-and-display must (1) keep only the
/// text in the replay ring and the `Output` frame, (2) write the `a=q` reply
/// back to the PTY writer, and (3) forward the image out-of-band as an
/// `Image` frame the client can decode.
#[test]
fn reader_strips_graphics_and_forwards_them_out_of_band() {
    use crate::kitty_graphics::Image;
    use base64::Engine as _;

    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);

    // A writer we can read back, to prove the query reply reached the PTY.
    #[derive(Clone)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let sink = Arc::new(Mutex::new(Vec::new()));
    let writer: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(SharedBuf(sink.clone()))));

    // One 1x1 opaque-red RGBA pixel, transmitted-and-displayed inline.
    let pixel = [0xffu8, 0x00, 0x00, 0xff];
    let b64 = base64::engine::general_purpose::STANDARD.encode(pixel);
    let stream = format!(
        "before\x1b_Gi=1,a=q,t=d,f=32,s=1,v=1;AAAA\x1b\\\
         mid\x1b_Ga=T,f=32,t=d,s=1,v=1,i=1;{b64}\x1b\\after"
    );

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(stream.into_bytes())),
        writer,
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    handle.join().unwrap();

    // (1) The ring holds the text, none of the graphics bytes.
    assert_eq!(state.lock().unwrap().ring.flatten(), b"beforemidafter");
    // (2) The query reply went to the PTY writer.
    assert_eq!(sink.lock().unwrap().as_slice(), b"\x1b_Gi=1;OK\x1b\\");
    // (3) The subscriber sees the passthrough and the image *in stream order*:
    // the text before the image, then the image at its cursor cell, then the
    // text after it. The `a=q` reply splits "before" from "mid" into two
    // Output frames; the image sits between "mid" and "after".
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"before"));
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"mid"));
    match sub_rx.try_recv() {
        Ok(DaemonMsg::Image(frame)) => {
            let img = Image::decode_frame(&frame).expect("decodable image frame");
            assert_eq!(img.id, 1);
            assert_eq!((img.width, img.height), (1, 1));
            assert_eq!(img.to_rgba8().unwrap(), pixel);
        }
        other => panic!("expected Image frame, got {other:?}"),
    }
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"after"));
}

/// A window that reopens onto a native ssh pane it outlived attaches by
/// pane id: it never sees the profile that dialled the host, so it sends
/// `allow_remote_clipboard_write: false` because that is all it has. The
/// spec's answer has to survive that, or the permission the user granted
/// is revoked on the first re-attach and every later copy comes back
/// `EPERM` with the switch still showing "on".
#[test]
fn a_spec_granted_clipboard_permission_survives_a_re_attach() {
    let mut st = test_state(true);
    st.clipboard_write_from_spec = Some(true);
    let (tx, _rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, false);
    assert!(st.allow_remote_clipboard_write);

    // And a profile that says no is not something an attaching client can
    // talk its way past either.
    let mut st = test_state(true);
    st.clipboard_write_from_spec = Some(false);
    let (tx, _rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), true, false);
    assert!(!st.allow_remote_clipboard_write);

    // A pane with no spec of its own — everything on a remote
    // `anterm-server` — is exactly as permitted as its controller says.
    let mut st = test_state(true);
    let (tx, _rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), true, false);
    assert!(st.allow_remote_clipboard_write);
    let (tx, _rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, false);
    assert!(!st.allow_remote_clipboard_write);
}

#[test]
fn reader_strips_allowed_clipboard_images_and_only_notifies_the_controller() {
    use base64::Engine as _;

    let state = Arc::new(Mutex::new(test_state(true)));
    let (controller_tx, controller_rx) = mpsc::channel();
    let (observer_tx, observer_rx) = mpsc::channel();
    {
        let mut st = state.lock().unwrap();
        attach_subscriber_with_permissions(
            &mut st,
            controller_tx,
            &OutputGate::new(),
            true,
            false,
        );
        observe_subscriber(&mut st, observer_tx, Arc::new(OutputGate::new()), false);
    }
    drain(&controller_rx);
    drain(&observer_rx);

    let mime = base64::engine::general_purpose::STANDARD.encode("image/png");
    let data = base64::engine::general_purpose::STANDARD.encode(b"png-data");
    let stream = format!(
        "before\x1b]5522;type=write:id=req-1\x1b\\\
         \x1b]5522;type=wdata:mime={mime};{data}\x1b\\\
         \x1b]5522;type=wdata\x1b\\after"
    );
    DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(stream.into_bytes())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(|| {})),
    )
    .join()
    .unwrap();

    assert_eq!(state.lock().unwrap().ring.flatten(), b"beforeafter");
    assert!(matches!(
        controller_rx.try_recv(),
        Ok(DaemonMsg::Output(bytes)) if bytes == b"before"
    ));
    let frame = match controller_rx.try_recv().unwrap() {
        DaemonMsg::ClipboardWrite(frame) => frame,
        other => panic!("expected ClipboardWrite, got {other:?}"),
    };
    let write = crate::clipboard::ClipboardWrite::decode_frame(frame).unwrap();
    assert_eq!(write.mime, "image/png");
    assert_eq!(write.data, b"png-data");
    assert_eq!(write.id.as_deref(), Some("req-1"));
    assert!(matches!(
        controller_rx.try_recv(),
        Ok(DaemonMsg::Output(bytes)) if bytes == b"after"
    ));

    assert!(matches!(
        observer_rx.try_recv(),
        Ok(DaemonMsg::Output(bytes)) if bytes == b"before"
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(DaemonMsg::Output(bytes)) if bytes == b"after"
    ));
    assert!(
        !observer_rx
            .try_iter()
            .any(|msg| matches!(msg, DaemonMsg::ClipboardWrite(_))),
        "an observer must never receive a clipboard side effect"
    );
}

#[test]
fn reader_rejects_clipboard_images_when_permission_is_off() {
    use base64::Engine as _;

    let state = Arc::new(Mutex::new(test_state(true)));
    let (controller_tx, controller_rx) = mpsc::channel();
    attach_subscriber(&mut state.lock().unwrap(), controller_tx);
    drain(&controller_rx);

    #[derive(Clone)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let sink = Arc::new(Mutex::new(Vec::new()));
    let writer: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(SharedBuf(sink.clone()))));
    let mime = base64::engine::general_purpose::STANDARD.encode("image/png");
    let data = base64::engine::general_purpose::STANDARD.encode(b"png-data");
    let stream = format!(
        "\x1b]5522;type=write:id=nope\x1b\\\
         \x1b]5522;type=wdata:mime={mime};{data}\x1b\\\
         \x1b]5522;type=wdata\x1b\\"
    );
    DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(stream.into_bytes())),
        writer,
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(|| {})),
    )
    .join()
    .unwrap();

    assert!(state.lock().unwrap().ring.flatten().is_empty());
    assert!(
        !controller_rx
            .try_iter()
            .any(|msg| matches!(msg, DaemonMsg::ClipboardWrite(_)))
    );
    assert_eq!(
        *sink.lock().unwrap(),
        crate::clipboard::response(Some("nope"), "EPERM")
    );
}

/// A kitty delete (`a=d`) is lifted out and forwarded as a `DeleteImage`
/// selector, leaving the surrounding text intact in the ring.
#[test]
fn reader_forwards_graphics_deletes() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(b"x\x1b_Ga=d,d=A\x1b\\y".to_vec())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    handle.join().unwrap();

    assert_eq!(state.lock().unwrap().ring.flatten(), b"xy");
    // In stream order: text before the delete, the delete, then text after.
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"x"));
    match sub_rx.try_recv() {
        Ok(DaemonMsg::DeleteImage(sel)) => {
            let d = ImageDelete::decode(&sel).unwrap();
            assert_eq!(d.target, b'A');
        }
        other => panic!("expected DeleteImage, got {other:?}"),
    }
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(b)) if b == b"y"));
}

#[test]
fn push_image_frame_drops_over_max_frame() {
    let mut frames = Vec::new();
    // At the limit: queued.
    push_image_frame(&mut frames, vec![0u8; MAX_FRAME]);
    // Over the limit: dropped, so the writer's fatal `write_frame` error
    // (which would disconnect the client) is never reached.
    push_image_frame(&mut frames, vec![0u8; MAX_FRAME + 1]);
    assert_eq!(frames.len(), 1, "only the in-budget frame is queued");
    assert!(matches!(&frames[0], GraphicsFrame::Image(f) if f.len() == MAX_FRAME));
}

#[test]
fn reader_poll_applies_the_probed_cwd() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    {
        let mut st = state.lock().unwrap();
        st.cwd = Some(PathBuf::from("/Users/alice"));
        st.subscriber = Some(sub_tx);
    }

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(b"alice@host ~ % ".to_vec())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| Some(PathBuf::from("/Users/alice/dev/anterm"))),
        },
        Arc::new(DeathReporter::new(|| {})),
    );
    handle.join().unwrap();

    assert_eq!(
        state.lock().unwrap().cwd.as_deref(),
        Some(Path::new("/Users/alice/dev/anterm"))
    );
    assert!(matches!(sub_rx.try_recv(), Ok(DaemonMsg::Output(_))));
    assert!(
        matches!(sub_rx.try_recv(), Ok(DaemonMsg::Cwd(p)) if p == PathBuf::from("/Users/alice/dev/anterm"))
    );
}

#[test]
fn reader_eof_without_subscriber_fires_on_dead() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (dead_tx, dead_rx) = mpsc::channel();

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(Vec::new())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(move || dead_tx.send(()).unwrap())),
    );
    handle.join().unwrap();

    assert!(!state.lock().unwrap().alive);
    assert!(dead_rx.try_recv().is_ok(), "unattached death → on_dead");
}

#[test]
fn reader_eof_during_shutdown_is_silent() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let dead = Arc::new(AtomicBool::new(false));
    let dead_flag = dead.clone();

    let handle = DaemonPane::spawn_reader(
        state.clone(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(OutputGate::new()),
        Box::new(std::io::Cursor::new(Vec::new())),
        null_writer(),
        || false,
        ForegroundProbes {
            group: Box::new(|| None),
            remote: Box::new(|| None),
            agent: Box::new(|| None),
            cwd: Box::new(|| None),
        },
        Arc::new(DeathReporter::new(move || {
            dead_flag.store(true, Ordering::SeqCst)
        })),
    );
    handle.join().unwrap();

    assert!(!state.lock().unwrap().alive);
    assert!(!dead.load(Ordering::SeqCst));
}

#[test]
fn death_reporter_notifies_once_across_racing_callers() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let (sub_tx, sub_rx) = mpsc::channel();
    state.lock().unwrap().subscriber = Some(sub_tx);
    let shutting_down = AtomicBool::new(false);
    let calls = Arc::new(AtomicBool::new(false));
    let calls_flag = calls.clone();
    let death = DeathReporter::new(move || calls_flag.store(true, Ordering::SeqCst));

    death.report(&state, &shutting_down);
    death.report(&state, &shutting_down);

    assert!(!state.lock().unwrap().alive);
    assert!(matches!(
        sub_rx.try_recv(),
        Ok(DaemonMsg::Exited { code: None })
    ));
    assert!(
        sub_rx.try_recv().is_err(),
        "a second report must not re-notify"
    );
}

#[test]
fn death_reporter_fires_on_dead_at_most_once() {
    let state = Arc::new(Mutex::new(test_state(true)));
    let shutting_down = AtomicBool::new(false);
    let (dead_tx, dead_rx) = mpsc::channel();
    let death = DeathReporter::new(move || dead_tx.send(()).unwrap());

    death.report(&state, &shutting_down);
    death.report(&state, &shutting_down);

    assert!(dead_rx.try_recv().is_ok(), "unattached death → on_dead");
    assert!(dead_rx.try_recv().is_err(), "on_dead must fire only once");
}

#[test]
fn pane_environment_advertises_the_terminal_under_the_standard_names() {
    let env: std::collections::HashMap<_, _> =
        pane_environment(&std::collections::HashMap::new(), false, 7, "/bin/zsh")
            .into_iter()
            .collect();
    let version = env!("CARGO_PKG_VERSION");

    assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("anterm"));
    assert_eq!(env.get("FORCE_HYPERLINK").map(String::as_str), Some("1"));
    assert_eq!(
        env.get("TERM_PROGRAM_VERSION").map(String::as_str),
        Some(version)
    );
    assert_eq!(
        env.get(crate::agent_hooks::ANTERM_ENV_MARKER)
            .map(String::as_str),
        Some(version)
    );
    assert_eq!(
        env.get("TERM").map(String::as_str),
        Some("xterm-256color"),
        "terminfo name is what the pane's decoder actually implements"
    );
}

#[test]
fn pane_environment_hands_the_shell_its_own_address() {
    let env: std::collections::HashMap<_, _> =
        pane_environment(&std::collections::HashMap::new(), false, 42, "/bin/zsh")
            .into_iter()
            .collect();

    assert_eq!(
        env.get(ANTERM_PANE_ENV).map(String::as_str),
        Some("42"),
        "a CLI inside the pane needs its own pane id for address-free verbs"
    );
    match config_dir_env() {
        Some(dir) => assert_eq!(
            env.get(ANTERM_CONFIG_DIR_ENV),
            Some(&dir),
            "the shell is told which config dir this server runs on, so a CLI \
             there resolves both endpoints the same way the server opened them"
        ),
        None => assert!(
            !env.contains_key(ANTERM_CONFIG_DIR_ENV),
            "no resolvable config dir must not inject an empty ANTERM_CONFIG_DIR"
        ),
    }
}

#[cfg(unix)]
#[test]
fn pane_environment_names_the_shell_the_pane_actually_runs() {
    // The reported bug: a pane running fish still advertised the passwd
    // login shell, so tmux, `sudo -s` and agents inside it all picked zsh.
    let env: std::collections::HashMap<_, _> = pane_environment(
        &std::collections::HashMap::new(),
        false,
        1,
        "/opt/homebrew/bin/fish",
    )
    .into_iter()
    .collect();

    assert_eq!(
        env.get(SHELL_ENV).map(String::as_str),
        Some("/opt/homebrew/bin/fish"),
        "$SHELL must name the shell the pane is actually running"
    );
}

#[cfg(unix)]
#[test]
fn an_unconfigured_pane_still_announces_the_shell_it_runs() {
    // No configured shell means the pane runs the login shell, and naming
    // it is both harmless and correct: it is what the pane is running.
    let cmd = build_shell_command(None, &Some(PathBuf::from("/tmp")), 1)
        .expect("build default shell command")
        .0;
    let shell = cmd
        .get_env(SHELL_ENV)
        .and_then(|v| v.to_str())
        .map(str::to_string)
        .expect("the default shell resolves to an absolute path");

    assert!(
        shell.starts_with('/'),
        "$SHELL must be an absolute path, got {shell}"
    );
    assert_eq!(
        shell,
        default_shell_name(&CommandBuilder::new_default_prog()),
        "an unconfigured pane announces the very shell it was about to run"
    );
}

#[cfg(unix)]
#[test]
fn a_configured_shell_variable_outranks_the_shell_anterm_launched() {
    // Same precedence contract as TERM_PROGRAM: anterm describes the pane,
    // the user's own `env` block gets the last word.
    let configured = [("SHELL".to_string(), "/bin/ksh".to_string())]
        .into_iter()
        .collect();

    let applied: std::collections::HashMap<_, _> =
        pane_environment(&configured, false, 1, "/opt/homebrew/bin/fish")
            .into_iter()
            .collect();

    assert_eq!(applied.get(SHELL_ENV).map(String::as_str), Some("/bin/ksh"));
}

#[cfg(windows)]
#[test]
fn windows_panes_are_left_without_a_shell_variable() {
    let applied = pane_environment(
        &std::collections::HashMap::new(),
        false,
        1,
        r"C:\Program Files\PowerShell\7\pwsh.exe",
    );

    assert!(
        !applied
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(SHELL_ENV)),
        "$SHELL is a POSIX contract: the MSYS, Cygwin and WSL tools that \
         read it on Windows want a POSIX path, and a Windows one would \
         point them at a shell they cannot exec"
    );
}

#[test]
fn shell_env_path_only_ever_answers_an_absolute_path() {
    let never = |_: &str| false;
    let path = || Some("/usr/local/bin:/usr/bin:relative/bin".to_string());

    assert_eq!(
        shell_env_path("/opt/homebrew/bin/fish", path, never).as_deref(),
        Some("/opt/homebrew/bin/fish"),
        "an absolute program is already the answer, PATH untouched"
    );
    assert_eq!(
        shell_env_path("fish", path, |c| c == "/usr/local/bin/fish").as_deref(),
        Some("/usr/local/bin/fish"),
        "a bare command must be resolved the way exec would resolve it"
    );
    assert_eq!(
        shell_env_path("fish", path, never),
        None,
        "an unresolvable bare command must leave $SHELL alone rather than \
         hand a consumer a name it may resolve differently"
    );
    assert_eq!(
        shell_env_path("./fish", path, |_| true),
        None,
        "a cwd-relative program is meaningless to a process with another cwd"
    );
    assert_eq!(shell_env_path("  ", path, |_| true), None);
    assert_eq!(
        shell_env_path("fish", || None, |_| true),
        None,
        "no PATH to search means nothing to promise"
    );
}

#[test]
fn the_shell_we_announce_is_the_argv_we_are_about_to_exec() {
    let mut configured = CommandBuilder::new("/opt/homebrew/bin/fish");
    configured.args(["-l"]);
    assert_eq!(
        launched_shell_program(&configured, "/bin/zsh"),
        "/opt/homebrew/bin/fish",
        "a builder with an argv has already overruled whatever anterm resolved"
    );

    assert_eq!(
        launched_shell_program(&CommandBuilder::new_default_prog(), "/bin/zsh"),
        "/bin/zsh",
        "an untouched default prog is exactly what resolved_program describes"
    );
}

#[test]
fn shell_env_path_ignores_relative_path_entries() {
    // A relative PATH entry resolves against the *pane's* cwd, which is not
    // ours, so it can never yield the absolute path $SHELL must hold.
    assert_eq!(
        shell_env_path("fish", || Some("bin:.".to_string()), |_| true),
        None
    );
}

#[test]
fn pane_environment_lets_configured_env_override_identity_but_not_capability() {
    let configured = [
        ("TERM_PROGRAM", "iTerm.app"),
        ("TERM_PROGRAM_VERSION", "3.5.0"),
        ("TERM", "dumb"),
        ("COLORTERM", ""),
        ("EDITOR", "hx"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
    .collect();

    let applied: std::collections::HashMap<_, _> =
        pane_environment(&configured, false, 1, "/bin/zsh")
            .into_iter()
            .collect();

    assert_eq!(
        applied.get("TERM_PROGRAM").map(String::as_str),
        Some("iTerm.app")
    );
    assert_eq!(
        applied.get("TERM_PROGRAM_VERSION").map(String::as_str),
        Some("3.5.0")
    );
    assert_eq!(applied.get("EDITOR").map(String::as_str), Some("hx"));
    assert_eq!(
        applied.get("TERM").map(String::as_str),
        Some("xterm-256color")
    );
    assert_eq!(
        applied.get("COLORTERM").map(String::as_str),
        Some("truecolor")
    );
}

#[cfg(windows)]
#[test]
fn pane_environment_advertises_light_and_dark_backgrounds() {
    let empty = std::collections::HashMap::new();
    let light: std::collections::HashMap<_, _> =
        pane_environment(&empty, false, 1, "pwsh.exe")
            .into_iter()
            .collect();
    let dark: std::collections::HashMap<_, _> =
        pane_environment(&empty, true, 1, "pwsh.exe")
            .into_iter()
            .collect();

    assert_eq!(light.get("COLORFGBG").map(String::as_str), Some("0;15"));
    assert_eq!(dark.get("COLORFGBG").map(String::as_str), Some("15;0"));
}

#[cfg(windows)]
#[test]
fn configured_colorfgbg_wins_case_insensitively() {
    let configured = [("ColorFgBg".to_string(), "3;4".to_string())]
        .into_iter()
        .collect();
    let applied = pane_environment(&configured, false, 1, "pwsh.exe");

    assert!(!applied.iter().any(|(key, _)| key == "COLORFGBG"));
    assert!(
        applied
            .iter()
            .any(|(key, value)| key == "ColorFgBg" && value == "3;4")
    );
}

#[cfg(windows)]
#[test]
fn pane_environment_capability_keys_cannot_be_overridden_by_recasing() {
    let configured = [("Term", "dumb"), ("ColorTerm", ""), ("term_program", "x")]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();

    let applied = pane_environment(&configured, false, 1, "pwsh.exe");

    assert!(
        !applied.iter().any(|(k, _)| k == "Term" || k == "ColorTerm"),
        "a recased capability key must be filtered out, or it would land \
         in the same case-folded slot and win by coming later"
    );
    let get = |key: &str| {
        applied
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("TERM"), Some("xterm-256color"));
    assert_eq!(get("COLORTERM"), Some("truecolor"));
    assert_eq!(get("term_program"), Some("x"));
}

#[test]
fn locale_fallback_respects_inherited_and_configured_environments() {
    let environment = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<std::collections::HashMap<_, _>>()
    };
    let fallback_is_needed = |extra: &[(&str, &str)], parent: &[(&str, &str)]| {
        let extra = environment(extra);
        let parent = environment(parent);
        locale_fallback_is_needed(&extra, |key| parent.get(key).cloned())
    };

    assert!(fallback_is_needed(&[], &[]));
    assert!(fallback_is_needed(&[], &[("LANG", "")]));
    assert!(!fallback_is_needed(&[], &[("LANG", "en_US.UTF-8")]));
    assert!(!fallback_is_needed(&[], &[("LC_ALL", "C")]));
    assert!(!fallback_is_needed(&[("LC_CTYPE", "UTF-8")], &[]));
    assert!(!fallback_is_needed(&[("LC_CTYPE", "")], &[]));
    assert!(!fallback_is_needed(
        &[("LANG", "")],
        &[("LANG", "en_US.UTF-8")]
    ));
}

#[test]
fn locale_fallback_sets_a_variable_that_backs_every_category() {
    let injected =
        std::iter::once((LOCALE_FALLBACK_KEY.to_string(), "en_US.UTF-8".to_string()))
            .collect::<std::collections::HashMap<_, _>>();

    // Whatever we inject has to satisfy the check that gated it, or every
    // pane would keep re-deriving a fallback that is already in place.
    assert!(!locale_fallback_is_needed(&injected, |_| None));

    // And it has to back every category, not just character handling — see
    // LOCALE_FALLBACK_KEY. LC_CTYPE here would leave a shell warning about
    // LC_COLLATE and friends on every launch.
    assert_eq!(LOCALE_FALLBACK_KEY, "LANG");
}

#[test]
fn posix_locale_stem_drops_script_and_keyword_subtags() {
    let stem = |id: &str| posix_locale_stem(id);

    assert_eq!(stem("en_US").as_deref(), Some("en_US"));
    assert_eq!(stem("en-US").as_deref(), Some("en_US"));
    assert_eq!(stem("zh_Hans_CN").as_deref(), Some("zh_CN"));
    assert_eq!(
        stem("zh_Hans_CN@calendar=gregorian").as_deref(),
        Some("zh_CN")
    );
    assert_eq!(stem("es_419").as_deref(), Some("es_419"));
    assert_eq!(stem("EN_us").as_deref(), Some("en_US"));

    assert_eq!(stem("zh"), None);
    assert_eq!(stem("zh_Hans"), None);
    assert_eq!(stem(""), None);
    assert_eq!(stem("@calendar=gregorian"), None);
}

#[test]
fn character_locale_only_returns_installed_locales() {
    let installed = |names: &'static [&'static str]| move |n: &str| names.contains(&n);

    assert_eq!(
        character_locale(Some("zh_Hans_CN"), installed(&["zh_CN.UTF-8", "C.UTF-8"])),
        Some("zh_CN.UTF-8".to_string())
    );
    assert_eq!(
        character_locale(Some("en_CN"), installed(&["C.UTF-8", "en_US.UTF-8"])),
        Some("C.UTF-8".to_string())
    );
    assert_eq!(
        character_locale(Some("en_CN"), installed(&["en_US.UTF-8"])),
        Some("en_US.UTF-8".to_string())
    );
    assert_eq!(
        character_locale(None, installed(&["C.UTF-8", "en_US.UTF-8"])),
        Some("C.UTF-8".to_string())
    );
    assert_eq!(character_locale(Some("zh_Hans_CN"), installed(&[])), None);
}

#[cfg(target_os = "macos")]
#[test]
fn derived_character_locale_is_installed_on_this_machine() {
    let exists = |name: &str| {
        std::path::Path::new(LOCALE_DEFINITION_DIR)
            .join(name)
            .is_dir()
    };
    let locale = character_locale(system_locale_identifier().as_deref(), exists)
        .expect("macOS always ships en_US.UTF-8");
    assert!(exists(&locale), "derived {locale} is not installed");
    assert!(
        locale.ends_with(".UTF-8"),
        "derived {locale} is not a UTF-8 locale"
    );
}

#[test]
fn spawned_shell_carries_the_anterm_marker() {
    let cmd = build_shell_command(None, &Some(PathBuf::from("/tmp")), 42)
        .expect("build default shell command")
        .0;
    let anterm = cmd
        .get_env(crate::agent_hooks::ANTERM_ENV_MARKER)
        .and_then(|v| v.to_str());
    assert_eq!(
        anterm,
        Some(env!("CARGO_PKG_VERSION")),
        "the daemon must inject ANTERM into every spawned shell"
    );
    assert_eq!(
        cmd.get_env(ANTERM_PANE_ENV).and_then(|v| v.to_str()),
        Some("42"),
        "the daemon must tell every spawned shell which pane it is"
    );
    if let Some(dir) = config_dir_env() {
        assert_eq!(
            cmd.get_env(ANTERM_CONFIG_DIR_ENV).and_then(|v| v.to_str()),
            Some(dir.as_str()),
            "the daemon must tell every spawned shell which config dir it serves"
        );
    }
}

#[cfg(unix)]
#[test]
fn the_two_endpoints_are_siblings_derived_from_one_config_dir() {
    // The bug this pins: the control socket used to ignore the config dir
    // and sit in ~/.local/share/anterm as `daemon.sock`, the same basename the
    // pane socket uses. A `--config-dir` server therefore published someone
    // else's control endpoint, and anything deriving one path from the other
    // by filename got the wrong socket. Both must come from the config dir,
    // and they must not collide.
    let dir = std::path::Path::new("/tmp/anterm-endpoint-test");
    let pane = crate::daemon::transport::socket_path_for(dir);
    let control = crate::host::server::socket_path_in(dir, &[]).expect("short enough path");

    assert_eq!(pane.parent(), control.parent(), "one directory, two files");
    assert_ne!(
        pane.file_name(),
        control.file_name(),
        "same basename is what made them indistinguishable"
    );
    assert_eq!(pane, dir.join("daemon.sock"));
    assert_eq!(control, dir.join("control.sock"));
}

#[test]
fn apply_signals_updates_state() {
    let mut st = test_state(true);

    apply_signals(
        &mut st,
        SniffSignals {
            cwd: Some(PathBuf::from("/tmp/x")),
            ..SniffSignals::default()
        },
    );
    assert_eq!(st.cwd, Some(PathBuf::from("/tmp/x")));

    apply_signals(
        &mut st,
        SniffSignals {
            shell: vec![ShellState {
                active: true,
                at_prompt: true,
                mark_at_prompt: true,
                last_exit_code: Some(0),
                command: None,
            }],
            ..SniffSignals::default()
        },
    );
    assert!(st.shell.active && st.shell.at_prompt);
    assert_eq!(st.shell.last_exit_code, Some(0));

    apply_signals(&mut st, SniffSignals::default());
    assert_eq!(st.cwd, Some(PathBuf::from("/tmp/x")));
}

/// A quick launch types the agent's command the moment the shell is up, so
/// the agent's whole first screen can land inside the poll interval of the
/// prompt it was typed at — and then it waits for a key, printing nothing
/// more. Probing only on the interval, the pane never found out an agent
/// was running. A change of foreground process group now probes at once.
#[test]
fn a_new_foreground_group_is_probed_inside_the_poll_interval() {
    struct Chunks(std::collections::VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                Some(chunk) => {
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    Ok(chunk.len())
                }
                None => Ok(0),
            }
        }
    }

    let probes_for = |groups: Vec<i32>| {
        let state = Arc::new(Mutex::new(test_state(true)));
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = probes.clone();
        let groups = Mutex::new(groups.into_iter());
        let handle = DaemonPane::spawn_reader(
            state,
            Arc::new(AtomicBool::new(false)),
            Arc::new(OutputGate::new()),
            // Plain output, no prompt marks: only the interval or the
            // group can trigger a probe.
            Box::new(Chunks(
                [b"$ ".to_vec(), b"agent screen".to_vec()]
                    .into_iter()
                    .collect(),
            )),
            null_writer(),
            || false,
            ForegroundProbes {
                group: Box::new(move || groups.lock().unwrap().next()),
                remote: Box::new(|| None),
                agent: Box::new(move || {
                    counted.fetch_add(1, Ordering::SeqCst);
                    None
                }),
                cwd: Box::new(|| None),
            },
            Arc::new(DeathReporter::new(|| {})),
        );
        handle.join().unwrap();
        probes.load(Ordering::SeqCst)
    };

    assert_eq!(
        probes_for(vec![10, 10]),
        1,
        "same program: the interval rules"
    );
    assert_eq!(
        probes_for(vec![10, 11]),
        2,
        "a new program is probed at once"
    );
}

/// A full-screen program that has run long enough to push its `?1049h`
/// and its first full paint out of the ring: what is left is differences.
fn state_running_a_long_lived_program() -> PaneState {
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h\x1b[2Jthe first full paint");
    record_output(&mut st, &vec![b'x'; RING_CAP]);
    st
}

/// The resizes a nudge makes, in order.
fn run_recorded_nudge(
    transient: WinSize,
    settled: impl FnOnce() -> Option<WinSize> + Send + 'static,
) -> Vec<WinSize> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&calls);
    run_redraw_nudge(
        transient,
        Duration::from_millis(1),
        Box::new(move |size| record.lock().unwrap().push(size)),
        settled,
    )
    .expect("spawn the nudge thread")
    .join()
    .unwrap();
    calls.lock().unwrap().clone()
}

/// #1122: a client reattaching to such a program gets the alternate screen
/// and a stream of differences from a screen it never saw. The pty steps
/// a row away and back, so the program hears a size change and repaints.
#[test]
fn a_reattach_after_the_first_paint_left_the_ring_makes_the_program_repaint() {
    let mut st = state_running_a_long_lived_program();
    let (tx, _rx) = mpsc::channel();
    attach_subscriber_with_permissions(&mut st, tx, &OutputGate::new(), false, true);

    let transient = redraw_nudge(&mut st, true, std::time::Instant::now())
        .expect("the screen the program painted is gone from the ring");
    assert_eq!(transient, ws(80, 23), "one row off the pane's own size");

    let st = Arc::new(Mutex::new(st));
    let settled = Arc::clone(&st);
    let calls = run_recorded_nudge(transient, move || {
        let st = settled.lock().unwrap();
        st.alive.then(|| st.ring.size())
    });
    assert_eq!(
        calls,
        vec![ws(80, 23), ws(80, 24)],
        "away and back to where the pane was"
    );
    // Only the pty moved: the ring is still at the pane's size, so no
    // client and no lease saw a change.
    assert_eq!(st.lock().unwrap().ring.size(), ws(80, 24));
}

#[test]
fn a_reattach_that_replays_whole_leaves_the_program_alone() {
    let now = std::time::Instant::now();

    // The program's start is still in the ring: the replay rebuilds it.
    let mut st = test_state(true);
    record_output(&mut st, b"\x1b[?1049h\x1b[2Jthe first full paint");
    record_output(&mut st, &vec![b'x'; 4096]);
    assert_eq!(redraw_nudge(&mut st, true, now), None, "nothing was lost");

    // The ring dropped its front, but nothing is in the alternate screen:
    // a shell's scrollback replays as what it is.
    let mut st = test_state(true);
    record_output(&mut st, b"$ cat big.log\r\n");
    record_output(&mut st, &vec![b'x'; RING_CAP]);
    assert_eq!(redraw_nudge(&mut st, true, now), None, "primary screen");

    // Left the alternate screen, and the ring cut through both switches.
    let mut st = state_running_a_long_lived_program();
    record_output(&mut st, b"\x1b[?1049l$ ");
    record_output(&mut st, &vec![b'x'; RING_CAP]);
    assert_eq!(redraw_nudge(&mut st, true, now), None, "program left");

    let mut st = state_running_a_long_lived_program();
    assert_eq!(
        redraw_nudge(&mut st, false, now),
        None,
        "no program owns the pane to repaint"
    );

    let mut st = state_running_a_long_lived_program();
    st.alive = false;
    assert_eq!(redraw_nudge(&mut st, true, now), None, "pane is dead");
}

#[test]
fn a_burst_of_reattaches_makes_one_nudge() {
    let mut st = state_running_a_long_lived_program();
    let now = std::time::Instant::now();
    assert!(redraw_nudge(&mut st, true, now).is_some());
    assert_eq!(
        redraw_nudge(&mut st, true, now + Duration::from_millis(500)),
        None,
        "the repaint the first one asked for is on its way"
    );
    assert!(
        redraw_nudge(&mut st, true, now + REDRAW_NUDGE_COOLDOWN).is_some(),
        "a reattach well after it is nudged again"
    );
}

/// The size put back is the pane's size when it is put back: a resize that
/// landed in between, or a lease taken meanwhile, is what the pty ends at.
#[test]
fn a_nudge_puts_back_the_size_the_pane_has_by_then() {
    let calls = run_recorded_nudge(ws(80, 23), || Some(ws(100, 30)));
    assert_eq!(calls, vec![ws(80, 23), ws(100, 30)]);

    let calls = run_recorded_nudge(ws(80, 23), || None);
    assert_eq!(calls, vec![ws(80, 23)], "a pane that died is left alone");
}

#[test]
fn a_one_row_pane_is_nudged_a_row_taller() {
    let mut st = state_running_a_long_lived_program();
    st.ring.resize(ws(80, 1));
    assert_eq!(
        redraw_nudge(&mut st, true, std::time::Instant::now()),
        Some(ws(80, 2))
    );
}
