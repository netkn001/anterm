use std::path::PathBuf;
use std::time::Duration;

use super::probe::HANDSHAKE_TIMEOUT;
use super::process::live_daemon_pid;
use super::{ensure_running, preserve_live_daemon};
use crate::daemon::{pidfile, transport};

#[test]
fn automatic_recovery_preserves_a_live_unresponsive_daemon() {
    const VAR: &str = "ANTERM_TEST_UNRESPONSIVE_DIR";
    if let Some(dir) = std::env::var_os(VAR) {
        crate::config::set_config_dir(PathBuf::from(dir));
        let _seat = match crate::daemon::singleton::claim() {
            crate::daemon::singleton::Claim::Held(seat) => seat,
            other => panic!("could not claim test seat: {other:?}"),
        };
        crate::daemon::pidfile::write_current();
        // A live holder without a serving endpoint is enough to reproduce the
        // dangerous recovery path; ensure_running must not unlink its records.
        let error = ensure_running().unwrap_err().to_string();
        assert!(error.contains("left intact"), "{error}");
        assert_eq!(pidfile::read(), Some(std::process::id()));
        assert!(live_daemon_pid().is_some());
        // Also exercise an accepting listener that stalls during the handshake.
        let listener = transport::bind().unwrap();
        let peer = std::thread::spawn(move || {
            let _stream = listener.incoming().next().unwrap().unwrap();
            std::thread::sleep(HANDSHAKE_TIMEOUT + Duration::from_millis(250));
        });
        let error = ensure_running().unwrap_err().to_string();
        assert!(error.contains("left intact"), "{error}");
        assert_eq!(pidfile::read(), Some(std::process::id()));
        assert!(transport::endpoint_exists());
        peer.join().unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "daemon::spawn::tests::automatic_recovery_preserves_a_live_unresponsive_daemon",
            "--nocapture",
        ])
        .env(VAR, dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(preserve_live_daemon(None).is_ok());
}

#[cfg(windows)]
mod ctrl_c_flag_tests {
    use super::super::process::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS};
    use super::super::{DAEMON_CREATION_FLAGS, windows};

    /// `CREATE_NEW_PROCESS_GROUP` disables Ctrl+C for every process in the new
    /// group, and the daemon's group is every pane shell it ever spawns. Both
    /// spawn paths — the ordinary one and the Redirection Trust detour — have to
    /// stay clear of it or #451 and #314 come straight back.
    #[test]
    fn the_daemon_is_never_created_into_a_ctrl_c_free_process_group() {
        assert_eq!(
            DAEMON_CREATION_FLAGS & CREATE_NEW_PROCESS_GROUP,
            0,
            "the daemon must not disable Ctrl+C for everything it spawns"
        );
        assert_eq!(
            DAEMON_CREATION_FLAGS,
            DETACHED_PROCESS | CREATE_NO_WINDOW,
            "the daemon still wants no console and no window"
        );

        assert_eq!(
            windows::SPAWN_FLAGS & CREATE_NEW_PROCESS_GROUP,
            0,
            "the clean-parent spawn path must not reintroduce the group either"
        );
    }
}

#[cfg(windows)]
mod windows_spawn_tests {
    use std::io;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use windows_sys::Win32::System::Threading::{
        ProcessRedirectionTrustPolicy, SetProcessMitigationPolicy,
    };

    use super::super::windows;

    const INNER_ENV: &str = "ANTERM_REDIRECTION_TRUST_INNER";
    const CLEAN_PARENT_ENV: &str = "ANTERM_REDIRECTION_TRUST_CLEAN_PARENT";
    const PROBE_RESULT_ENV: &str = "ANTERM_REDIRECTION_TRUST_PROBE_RESULT";
    const TEST_NAME: &str =
        "daemon::spawn::tests::windows_spawn_tests::daemon_spawn_does_not_inherit_redirection_trust";

    /// Redirection Trust cannot be disabled after it is enforced, so the outer
    /// test delegates the destructive policy change to a short-lived copy of
    /// the test executable. This keeps the remaining test process clean.
    #[test]
    fn daemon_spawn_does_not_inherit_redirection_trust() {
        // A probe process reports both its inherited policy and whether it can
        // traverse the fixture. Checking the policy directly keeps this test
        // deterministic on elevated CI runners, whose own junctions may remain
        // trusted even while Redirection Trust is enforced.
        if let Some(result) = std::env::var_os(PROBE_RESULT_ENV) {
            let result = PathBuf::from(result);
            let junction_probe = result
                .parent()
                .expect("probe result has a fixture directory")
                .join("current")
                .join("probe.txt");
            let verdict = if windows::redirection_trust_enforced() {
                "ENFORCED"
            } else if junction_probe.exists() {
                "CLEAN_OK"
            } else {
                "CLEAN_BLOCKED"
            };
            std::fs::write(result, verdict).expect("write mitigation probe verdict");
            return;
        }

        if std::env::var_os(INNER_ENV).is_none() {
            let output = Command::new(std::env::current_exe().expect("locate test executable"))
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(INNER_ENV, "1")
                .env(CLEAN_PARENT_ENV, std::process::id().to_string())
                .output()
                .expect("spawn isolated mitigation test process");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success(),
                "isolated mitigation test failed:\nstdout:\n{stdout}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            // libtest exits 0 when `--exact` matches nothing, so a stale
            // TEST_NAME would turn this whole regression into a silent pass.
            assert!(
                stdout.contains("1 passed"),
                "the isolated mitigation test must actually run; TEST_NAME is probably stale:\n{stdout}"
            );
            return;
        }

        let clean_parent_pid: u32 = std::env::var(CLEAN_PARENT_ENV)
            .expect("clean parent pid")
            .parse()
            .expect("clean parent pid is numeric");

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "anterm-redirection-trust-{}-{unique}",
            std::process::id()
        ));
        let target = root.join("version");
        let junction = root.join("current");
        let inherited_result = root.join("inherited-result.txt");
        let result = root.join("result.txt");
        let batch = root.join("junction probe.cmd");
        std::fs::create_dir_all(&target).expect("create junction target");
        std::fs::write(target.join("probe.txt"), b"ok").expect("write junction probe");

        let comspec = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let linked = Command::new(&comspec)
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .status()
            .expect("create junction fixture");
        assert!(linked.success(), "mklink must create the junction fixture");

        let mut policy = 1u32;
        // SAFETY: The DWORD buffer exactly matches the Windows policy layout,
        // and only this disposable inner test process receives the policy.
        let enabled = unsafe {
            SetProcessMitigationPolicy(
                ProcessRedirectionTrustPolicy,
                (&raw mut policy).cast(),
                size_of::<u32>(),
            )
        };
        assert!(
            enabled != 0,
            "enable Redirection Trust: {}",
            io::Error::last_os_error()
        );
        assert!(
            windows::redirection_trust_enforced(),
            "anterm must detect the enforcing policy before selecting the alternate spawn path"
        );

        // First prove that an ordinary child inherits the enforced policy.
        // This is the red-capable half of the regression and does not depend on
        // how Windows classifies the junction created by the current account.
        let inherited = Command::new(std::env::current_exe().expect("locate test executable"))
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(PROBE_RESULT_ENV, &inherited_result)
            .status()
            .expect("spawn ordinary mitigation probe");
        assert!(inherited.success(), "ordinary mitigation probe must run");
        assert_eq!(
            std::fs::read_to_string(&inherited_result)
                .expect("read inherited mitigation verdict")
                .trim(),
            "ENFORCED",
            "an ordinary child must demonstrate the policy inheritance that the alternate spawn path removes"
        );

        // The clean helper inherits INNER_ENV from this disposable process. A
        // batch wrapper adds the result path before launching another copy of
        // the test executable, whose first branch records its actual policy.
        let test_exe = std::env::current_exe().expect("locate test executable");
        let script = format!(
            "@echo off\r\nset \"{PROBE_RESULT_ENV}={}\"\r\n\"{}\" --exact \"{TEST_NAME}\" --nocapture\r\n",
            result.display(),
            test_exe.display(),
        );
        std::fs::write(&batch, script).expect("write mitigation probe batch");

        windows::spawn_detached_with_parent(
            Path::new(&comspec),
            &[
                std::ffi::OsString::from("/d"),
                std::ffi::OsString::from("/c"),
                batch.as_os_str().to_owned(),
            ],
            clean_parent_pid,
        )
        .expect("spawn mitigation probe through clean logical parent");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !result.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }

        let verdict = std::fs::read_to_string(&result).unwrap_or_else(|_| "MISSING".into());
        let _ = std::fs::remove_dir(&junction);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            verdict.trim(),
            "CLEAN_OK",
            "a anterm daemon child must drop the inherited policy and retain access to user-created junctions"
        );
    }
}

mod exe_name_tests {
    use std::path::Path;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::time::{Duration, Instant};

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use super::super::process::{is_zombie, process_alive, process_comm};
    use super::super::process::{
        is_reapable_daemon_name, is_supported_shell, strip_deleted_marker, strip_exe_suffix,
    };

    #[test]
    fn every_legitimate_daemon_name_is_reapable_with_and_without_exe() {
        for name in [
            "anterm-app",
            "anterm-server",
            "anterm",
            "anterm-app.exe",
            "anterm-server.exe",
            "anterm.exe",
        ] {
            assert!(is_reapable_daemon_name(name), "{name} is a daemon of ours");
        }
    }

    #[test]
    fn the_current_executable_name_remains_reapable() {
        let own = std::env::current_exe()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            is_reapable_daemon_name(&own),
            "{own} launched this process and must stay in the set"
        );
    }

    #[test]
    fn foreign_process_names_are_never_reapable() {
        for name in [
            "explorer.exe",
            "sleep",
            "antermd",
            "notanterm",
            "anterm-app2",
            "anterm.",
            "",
        ] {
            assert!(
                !is_reapable_daemon_name(name),
                "{name:?} must be protected from the reap"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_matches_daemon_names_case_insensitively() {
        assert!(is_reapable_daemon_name("ANTERM-APP.EXE"));
        assert!(is_reapable_daemon_name("Anterm-Server"));
        assert!(is_reapable_daemon_name("ANTERM"));
    }

    /// The kernel's " (deleted)" marker on a replaced executable is not part
    /// of its name; treating it as one made an updated-under daemon read as
    /// foreign, which dropped its record without reaping it (#667).
    #[test]
    fn the_deleted_marker_is_not_part_of_a_process_name() {
        assert_eq!(
            strip_deleted_marker("/opt/anterm/anterm-server (deleted)".into()),
            "/opt/anterm/anterm-server"
        );
        assert_eq!(
            strip_deleted_marker("/opt/anterm/anterm-server".into()),
            "/opt/anterm/anterm-server"
        );
        // Only the trailing marker is the kernel's.
        assert_eq!(
            strip_deleted_marker("/tmp/x (deleted)/anterm-server".into()),
            "/tmp/x (deleted)/anterm-server"
        );
    }

    /// A zombie answers `kill(pid, 0)` like the living but holds nothing and
    /// cannot be signalled dead; counting it alive made the reap spend both
    /// kill timeouts on a corpse. The GUI never waits on the daemons it
    /// spawns, so this is the ordinary afterlife of a crashed daemon.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_zombie_is_not_an_alive_process() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id() as libc::pid_t;
        // It has exited; nobody has waited: a zombie, once the exit lands.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !is_zombie(pid) {
            assert!(
                Instant::now() < deadline,
                "the unwaited child must read as a zombie"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            unsafe { libc::kill(pid, 0) == 0 },
            "a zombie still answers signal 0 — that is the trap"
        );
        assert!(
            !process_alive(pid),
            "a corpse is not a process the reap can act on"
        );
        let _ = child.wait();
    }

    /// The comm fallback is what identifies a daemon whose executable path is
    /// unreadable — on macOS `proc_pidpath` fails outright once the binary is
    /// deleted. This process is alive and its own comm must resolve.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_live_process_resolves_its_own_comm_name() {
        let comm = process_comm(std::process::id() as libc::pid_t)
            .expect("this process is alive; its comm name must be readable");
        assert!(!comm.is_empty());
    }

    #[test]
    fn strip_exe_suffix_only_strips_a_trailing_exe() {
        assert_eq!(strip_exe_suffix("anterm-app.exe"), "anterm-app");
        assert_eq!(strip_exe_suffix("anterm-app.EXE"), "anterm-app");
        assert_eq!(strip_exe_suffix("anterm-app"), "anterm-app");
        assert_eq!(strip_exe_suffix(".exe"), "");
        assert_eq!(strip_exe_suffix("exe"), "exe");
    }

    #[test]
    fn supported_shell_detection_matches_shell_basenames_only() {
        assert!(is_supported_shell(Path::new("/opt/homebrew/bin/fish")));
        assert!(is_supported_shell(Path::new("/bin/zsh")));
        assert!(is_supported_shell(Path::new("/usr/bin/bash")));
        assert!(is_supported_shell(Path::new("/opt/homebrew/bin/nu")));
        assert!(is_supported_shell(Path::new("/portable/Nu.EXE")));
        assert!(!is_supported_shell(Path::new(
            "/Applications/kitty.app/kitty"
        )));
        assert!(!is_supported_shell(Path::new("/usr/bin/omp")));
    }
}

#[cfg(unix)]
mod unix_tests {
    use std::io::ErrorKind;
    use std::os::unix::net::UnixStream;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::time::Duration;
    use std::time::Instant;

    use super::super::probe::{
        DialectAnswer, HANDSHAKE_TIMEOUT, VersionProbe, dialect_of, query_daemon_version,
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use super::super::process::{
        ProcessIdentity, process_alive, process_identity, process_path, signal_and_await_exit,
    };
    use super::super::process::recorded_daemon_is_dead_with;
    use crate::daemon::control::CONTROL_VERSION;
    use crate::daemon::protocol::PROTOCOL_VERSION;

    #[test]
    fn a_missing_pidfile_is_not_a_dead_daemon() {
        let dead = |_| false;
        let alive = |_| true;

        // The case the caller has to keep handling itself: no pidfile is no
        // evidence. The daemon may have died between `transport::bind` (which
        // writes daemon.port) and `pidfile::write_current`, or never managed
        // the write at all — a refused connect is then the only signal that
        // the endpoint file is stale, and skipping the cleanup on it leaves
        // the spawn poll paying the refusal delay this whole path exists to
        // avoid.
        assert!(!recorded_daemon_is_dead_with(None, dead));

        // A recorded pid the OS has handed to something else is likewise not
        // evidence of death — it is alive, just not ours. `reap_recorded_daemon`
        // is what checks the executable before killing anything.
        assert!(!recorded_daemon_is_dead_with(Some(4242), alive));

        // Our own pid: an in-process daemon, so certainly not dead.
        assert!(!recorded_daemon_is_dead_with(
            Some(std::process::id()),
            dead
        ));

        // Reserved low pids are never a daemon of ours; refuse to call them
        // dead so nothing downstream reaps them.
        for reserved in [0, 1, 4] {
            assert!(
                !recorded_daemon_is_dead_with(Some(reserved), dead),
                "pid {reserved} must never be treated as our dead daemon"
            );
        }

        // The one case that is evidence: a plausible pid that is gone.
        assert!(recorded_daemon_is_dead_with(Some(4242), dead));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn reap_guard_rejects_a_live_process_of_another_executable() {
        // By path: a PATH with GNU coreutils first resolves `sleep` to `gsleep`.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as libc::pid_t;

        assert!(process_alive(pid), "the sleep child is alive and ours");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let basename = process_path(pid).and_then(|p| p.file_name().map(|n| n.to_os_string()));
            if basename == Some("sleep".into()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "process_path resolves an arbitrary pid, not just our parent \
                 (still {basename:?} after 5s)"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            matches!(process_identity(pid), ProcessIdentity::Foreign),
            "sleep must read as a foreign process — OurDaemon would mean the reap \
             could kill it, Unknown would mean its record is never cleared"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn signal_and_await_exit_observes_the_death_it_caused() {
        // By path: a PATH with GNU coreutils first resolves `sleep` to `gsleep`.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as libc::pid_t;
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });

        assert!(
            signal_and_await_exit(pid, libc::SIGTERM, std::time::Duration::from_secs(5)),
            "the child must be seen exiting within the grace window"
        );
        assert!(!process_alive(pid), "and be gone afterwards");
        reaper.join().unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn process_alive_is_false_once_the_process_is_gone() {
        // By path: a PATH with GNU coreutils first resolves `sleep` to `gsleep`.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as libc::pid_t;
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!process_alive(pid));
    }

    #[test]
    fn version_handshake_reads_a_matching_reply() {
        use crate::daemon::protocol::{ClientMsg, DaemonMsg, DaemonVersion, PROTOCOL_VERSION};

        let (mut client, mut daemon) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let msg = ClientMsg::read(&mut daemon).unwrap();
            assert_eq!(msg, ClientMsg::Version);
            DaemonMsg::Version(DaemonVersion {
                protocol: PROTOCOL_VERSION,
                build: "test".into(),
                features: Vec::new(),
                instance: "inst-test".into(),
            })
            .encode(&mut daemon)
            .unwrap();
        });

        match query_daemon_version(&mut client) {
            VersionProbe::Speaks(got) => {
                assert_eq!(got.protocol, PROTOCOL_VERSION);
                assert_eq!(got.build, "test");
            }
            other => panic!("a live daemon must answer, got {other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn version_handshake_treats_a_hangup_as_legacy() {
        use crate::daemon::protocol::ClientMsg;

        let (mut client, mut daemon) = UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let _ = ClientMsg::read(&mut daemon);
            drop(daemon);
        });

        assert_eq!(query_daemon_version(&mut client), VersionProbe::Legacy);
        server.join().unwrap();
    }

    #[test]
    fn version_handshake_treats_silence_as_unresponsive() {
        let (mut client, daemon) = UnixStream::pair().unwrap();
        let start = Instant::now();
        assert_eq!(
            query_daemon_version(&mut client),
            VersionProbe::Unresponsive
        );
        assert!(start.elapsed() >= HANDSHAKE_TIMEOUT);
        drop(daemon);
    }

    /// A control server on one end of a pair, answering the handshake with
    /// `control_version` and nothing else.
    fn control_peer_speaking(
        version: u32,
        build: &'static str,
    ) -> (UnixStream, std::thread::JoinHandle<()>) {
        use crate::daemon::control::{ControlClientMsg, ControlHelloOk, ControlServerMsg};

        let (client, mut server) = UnixStream::pair().unwrap();
        let joined = std::thread::spawn(move || {
            let _ = ControlClientMsg::read(&mut server);
            let _ = ControlServerMsg::HelloOk(ControlHelloOk {
                control_version: version,
                protocol_version: PROTOCOL_VERSION,
                build: build.to_string(),
                separator: '/',
                home: "/home/test".into(),
                features: Vec::new(),
                instance: "test".into(),
            })
            .encode(&mut server);
        });
        (client, joined)
    }

    #[test]
    fn an_older_control_dialect_is_named_even_though_the_pane_protocol_agrees() {
        let (client, peer) = control_peer_speaking(CONTROL_VERSION - 1, "26.7.7-nightly");
        let DialectAnswer::Refuses(refusal) = dialect_of(&client) else {
            panic!("a dialect a version behind is a mismatch")
        };
        assert_eq!(refusal.peer, CONTROL_VERSION - 1);
        assert_eq!(refusal.ours, CONTROL_VERSION);
        assert_eq!(
            refusal.peer_build, "26.7.7-nightly",
            "the prompt names the build the user has to restart"
        );
        peer.join().unwrap();
    }

    /// A daemon left behind by a *newer* build is just as unreachable, and the
    /// refusal has to carry which side is ahead — the prompt reads the two
    /// numbers to decide whether to tell the user to restart or to update.
    #[test]
    fn a_newer_control_dialect_is_a_mismatch_too_and_says_which_side_is_ahead() {
        let (client, peer) = control_peer_speaking(CONTROL_VERSION + 1, "26.9.0-nightly");
        let DialectAnswer::Refuses(refusal) = dialect_of(&client) else {
            panic!("a dialect a version ahead is a mismatch")
        };
        assert!(
            refusal.peer > refusal.ours,
            "the peer is ahead and the refusal must show it: {refusal:?}"
        );
        peer.join().unwrap();
    }

    #[test]
    fn a_server_of_our_own_dialect_is_not_a_mismatch() {
        let (client, peer) = control_peer_speaking(CONTROL_VERSION, "26.8.2-nightly");
        assert!(
            matches!(dialect_of(&client), DialectAnswer::Agrees),
            "a server that answered with our own number is the one answer that clears the record"
        );
        peer.join().unwrap();
    }

    #[test]
    fn a_control_socket_that_says_nothing_is_left_alone() {
        let (client, peer) = UnixStream::pair().unwrap();
        client.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).unwrap();
        assert!(
            matches!(dialect_of(&client), DialectAnswer::Silent),
            "a server still coming up must be reported as neither the wrong version nor \
             the right one — reading it as agreement is what wipes a live refusal"
        );
        drop(peer);
    }

    #[test]
    fn connect_to_stale_socket_path_fails() {
        let dir = std::env::temp_dir().join(format!("anterm-spawn-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.sock");
        let err = UnixStream::connect(&path).unwrap_err();
        assert!(matches!(
            err.kind(),
            ErrorKind::NotFound | ErrorKind::ConnectionRefused
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

mod mismatch_record_tests {
    use super::super::probe::{
        DialectAnswer, HandoffReturn, MismatchVerdict, VersionProbe, judge_handoff_return,
        judge_probe,
    };
    use super::super::{
        DaemonMismatch, daemon_generation, daemon_generation_moved, land_handoff_return,
        land_probe, local_daemon_is_this_build, note_daemon_mismatch, note_local_daemon,
        settle_daemon_mismatch, take_mismatched_daemon,
    };
    use crate::daemon::control::DialectRefusal;
    use crate::daemon::protocol::{DaemonVersion, PROTOCOL_VERSION};

    fn answering(protocol: u32, build: &str) -> VersionProbe {
        VersionProbe::Speaks(DaemonVersion {
            protocol,
            build: build.to_string(),
            features: Vec::new(),
            instance: "test-instance".to_string(),
        })
    }

    fn agrees() -> DialectAnswer {
        DialectAnswer::Agrees
    }

    fn refuses() -> DialectAnswer {
        DialectAnswer::Refuses(DialectRefusal {
            peer_build: "26.8.4-nightly.202608251825".to_string(),
            peer: 7,
            ours: 8,
        })
    }

    fn says_nothing() -> DialectAnswer {
        DialectAnswer::Silent
    }

    /// The record says what the daemon running *now* is, so a clean probe has
    /// to be able to overturn it — not merely decline to add to it.
    ///
    /// The prompt is armed from `ensure_running`, and `ensure_running` is the
    /// first thing every control-link reconnect attempt does. A mismatched
    /// daemon fails that connect, so the link backs off and retries, arming the
    /// prompt again each time round — including in the seconds the user spends
    /// reading the dialog it already opened. Restart the daemon and it is fine,
    /// but that last arming outlived the restart, and the next window built
    /// took it and asked a second time about a server that no longer exists.
    #[test]
    fn a_probe_that_finds_the_daemon_ours_overturns_an_earlier_mismatch() {
        let verdict = judge_probe(answering(PROTOCOL_VERSION, "1.0.0"), agrees)
            .expect("a daemon that answers the handshake is not a stale endpoint");
        assert!(
            matches!(verdict.mismatch, MismatchVerdict::Clear),
            "a daemon that agrees on both versions must wipe the record, not leave \
             an earlier probe's verdict standing"
        );
    }

    /// The half of the handshake that lives on the other socket. Panes work,
    /// every machine-tree call is refused, and the window opens with no tabs.
    #[test]
    fn a_refused_control_dialect_is_a_mismatch_even_when_the_protocol_agrees() {
        let verdict = judge_probe(answering(PROTOCOL_VERSION, "old"), refuses)
            .expect("the daemon answered; it is not stale");
        assert!(
            matches!(verdict.mismatch, MismatchVerdict::Found(DaemonMismatch::Dialect(r)) if r.peer == 7),
            "the dialect refusal has to reach the prompt, carrying the peer's number"
        );
    }

    /// The other half of the same socket: it did not answer at all.
    ///
    /// Not a mismatch and not a clean bill either. Reading silence as agreement
    /// is how a dialect refusal that a control link had already met gets wiped
    /// by a probe that merely failed to ask, leaving the next window to open
    /// empty with nothing on screen to explain it.
    #[test]
    fn a_silent_control_socket_leaves_an_earlier_verdict_standing() {
        let verdict = judge_probe(answering(PROTOCOL_VERSION, "old"), says_nothing)
            .expect("the pane endpoint answered; it is not stale");
        assert!(
            matches!(verdict.mismatch, MismatchVerdict::Unchanged),
            "a control socket that never answered has agreed to nothing"
        );
        assert!(
            verdict.version.is_some(),
            "the pane protocol still answered, so the daemon is still the one to \
             remember for feature checks"
        );
    }

    /// A build that differs is not by itself a mismatch: its panes still speak
    /// this protocol, and killing them to even up a version string is the
    /// upgrade that costs the user their session for nothing.
    #[test]
    fn a_different_build_on_the_agreed_protocol_is_not_a_mismatch() {
        let verdict = judge_probe(
            answering(PROTOCOL_VERSION, "26.8.4-nightly.202608251825"),
            agrees,
        )
        .expect("the daemon answered; it is not stale");
        assert!(
            matches!(verdict.mismatch, MismatchVerdict::Clear),
            "only the version numbers decide; a build string that differs is left alone"
        );
    }

    #[test]
    fn a_disagreeing_pane_protocol_is_a_mismatch() {
        let verdict = judge_probe(answering(PROTOCOL_VERSION + 1, "newer"), agrees)
            .expect("the daemon answered; it is not stale");
        assert!(
            matches!(&verdict.mismatch, MismatchVerdict::Found(DaemonMismatch::Protocol(Some(v))) if v.protocol == PROTOCOL_VERSION + 1),
            "the prompt needs the peer's protocol number to say what it found"
        );
        assert!(
            verdict.version.is_some(),
            "a mismatched daemon is still the daemon to remember for feature checks"
        );
    }

    #[test]
    fn a_daemon_from_before_versioning_is_a_mismatch_with_nothing_to_report() {
        let verdict = judge_probe(VersionProbe::Legacy, agrees)
            .expect("it answered, just not with a version");
        assert!(matches!(
            verdict.mismatch,
            MismatchVerdict::Found(DaemonMismatch::Protocol(None))
        ));
        assert!(
            verdict.version.is_none(),
            "there is no version to remember from a daemon that reports none"
        );
    }

    /// Not a mismatch — nothing to put to the user. The endpoint is stale and
    /// the caller reaps it.
    #[test]
    fn a_daemon_that_does_not_answer_is_stale_rather_than_mismatched() {
        assert!(judge_probe(VersionProbe::Unresponsive, agrees).is_none());
    }

    /// The dialect costs a second connect, so it is only worth asking once the
    /// pane protocol has agreed — on a protocol mismatch the restart is already
    /// decided.
    #[test]
    fn the_control_dialect_is_only_asked_once_the_pane_protocol_agrees() {
        let asked = std::cell::Cell::new(false);
        let _ = judge_probe(answering(PROTOCOL_VERSION + 1, "newer"), || {
            asked.set(true);
            DialectAnswer::Agrees
        });
        assert!(
            !asked.get(),
            "a daemon already known to be mismatched must not be probed a second time"
        );
    }

    #[test]
    fn a_handoff_that_returns_this_build_is_a_replacement() {
        let outcome = judge_handoff_return(answering(PROTOCOL_VERSION, env!("CARGO_PKG_VERSION")));
        assert!(matches!(outcome, HandoffReturn::Replaced(_)));
    }

    /// The exec did not take. Same process, same old image — and the caller
    /// reports the failure rather than pretending the upgrade happened.
    #[test]
    fn a_handoff_that_returns_the_old_build_is_not_a_replacement() {
        let outcome =
            judge_handoff_return(answering(PROTOCOL_VERSION, "26.8.4-nightly.202608251825"));
        assert!(matches!(outcome, HandoffReturn::CarriedOn(_)));
    }

    #[test]
    fn a_handoff_that_returns_something_mute_is_neither() {
        assert!(matches!(
            judge_handoff_return(VersionProbe::Legacy),
            HandoffReturn::Mute
        ));
        assert!(matches!(
            judge_handoff_return(VersionProbe::Unresponsive),
            HandoffReturn::Mute
        ));
    }

    /// The verdict actually reaching the global record, which is the step the
    /// pure judgement above cannot cover.
    ///
    /// One test for the whole sequence on purpose: the record is process-global
    /// and Rust runs tests in this binary side by side, so splitting these into
    /// separate `#[test]`s would let them overwrite each other's state.
    #[test]
    fn a_clean_verdict_wipes_what_an_earlier_one_recorded() {
        settle_daemon_mismatch(Some(DaemonMismatch::Protocol(None)));
        assert!(
            take_mismatched_daemon().is_some(),
            "a recorded mismatch is there for the next window to take"
        );

        settle_daemon_mismatch(Some(DaemonMismatch::Protocol(None)));
        settle_daemon_mismatch(None);
        assert!(
            take_mismatched_daemon().is_none(),
            "a clean probe after a mismatched one must leave nothing for a window to \
             prompt about — this is the second, bogus dialog"
        );

        // The restart the user actually asked for. The record here is the one
        // the reconnect loop set again while the dialog was on screen; landing
        // the handoff has to take it away, or it outlives the daemon it
        // describes and the next window opened prompts about a dead server.
        note_daemon_mismatch(DaemonMismatch::Protocol(None));
        land_handoff_return(
            daemon_generation(),
            HandoffReturn::Replaced(DaemonVersion {
                protocol: PROTOCOL_VERSION,
                build: env!("CARGO_PKG_VERSION").to_string(),
                features: Vec::new(),
                instance: "replacement".to_string(),
            }),
        )
        .expect("a daemon that came back as this build is a successful handoff");
        assert!(
            take_mismatched_daemon().is_none(),
            "the daemon the prompt described has been replaced; nothing is left to ask"
        );

        // The other restart, the one for a daemon too old to hand off: `stop()`
        // and a fresh spawn, landed through the same verdict the
        // already-running branch uses. Nothing else on that path touches the
        // record, so without this the prompt the user just answered comes back
        // about the daemon they killed.
        note_daemon_mismatch(DaemonMismatch::Protocol(None));
        land_probe(
            daemon_generation(),
            judge_probe(
                answering(PROTOCOL_VERSION, env!("CARGO_PKG_VERSION")),
                agrees,
            )
            .expect("the freshly spawned daemon answered its own handshake"),
        );
        assert!(
            take_mismatched_daemon().is_none(),
            "the daemon the prompt described was stopped and replaced; nothing is left to ask"
        );

        // A verdict about a daemon that has since been replaced is not written
        // at all. This is the reconnect that connected before the handoff and
        // came back after it: without the stamp it re-arms the prompt about a
        // process the user has already replaced, and the fix above closes only
        // the gap it can see.
        let taken_before = daemon_generation();
        daemon_generation_moved();
        land_probe(
            taken_before,
            judge_probe(
                answering(PROTOCOL_VERSION + 1, "the daemon that was"),
                agrees,
            )
            .expect("the outgoing daemon answered, late"),
        );
        assert!(
            take_mismatched_daemon().is_none(),
            "a verdict about a replaced daemon must not reach the prompt"
        );

        // And the same both ways round: a late *clean* verdict about a daemon
        // that has since been replaced must not wipe what the current one left.
        note_daemon_mismatch(DaemonMismatch::Protocol(None));
        let taken_before = daemon_generation();
        daemon_generation_moved();
        land_probe(
            taken_before,
            judge_probe(
                answering(PROTOCOL_VERSION, env!("CARGO_PKG_VERSION")),
                agrees,
            )
            .expect("the outgoing daemon answered, late"),
        );
        assert!(
            take_mismatched_daemon().is_some(),
            "a stale clean verdict says nothing about the daemon running now"
        );

        // A handoff that did not take leaves the daemon exactly as it was, so
        // the record describing it stays — the user still needs the offer.
        note_daemon_mismatch(DaemonMismatch::Protocol(None));
        assert!(
            land_handoff_return(
                daemon_generation(),
                HandoffReturn::CarriedOn(DaemonVersion {
                    protocol: PROTOCOL_VERSION,
                    build: "26.8.4-nightly.202608251825".to_string(),
                    features: Vec::new(),
                    instance: "survivor".to_string(),
                })
            )
            .is_err(),
            "a daemon still on its old build is a handoff that failed"
        );
        assert!(
            take_mismatched_daemon().is_some(),
            "the mismatched daemon is still running, so the prompt still has something to say"
        );

        // The palette's "already up to date" reads both records, and only a
        // daemon known to be this build — with nothing waiting to be put to
        // the user — counts.
        let this_build = |build: &str| DaemonVersion {
            protocol: PROTOCOL_VERSION,
            build: build.to_string(),
            features: Vec::new(),
            instance: "current".to_string(),
        };
        note_local_daemon(Some(this_build(env!("CARGO_PKG_VERSION"))));
        assert!(local_daemon_is_this_build());

        note_daemon_mismatch(DaemonMismatch::Protocol(None));
        assert!(
            !local_daemon_is_this_build(),
            "a mismatch waiting for the user is not a current daemon, whatever build it named"
        );
        take_mismatched_daemon();

        note_local_daemon(Some(this_build("26.1.0")));
        assert!(!local_daemon_is_this_build(), "the in-place update case");
        note_local_daemon(None);
        assert!(
            !local_daemon_is_this_build(),
            "a daemon never probed is not proven current"
        );
    }
}

#[test]
fn stop_for_quit_blocks_respawn_and_reaps_in_flight_spawn() {
    const VAR: &str = "ANTERM_TEST_QUIT_RESPAWN_DIR";
    const SLEEPER_VAR: &str = "ANTERM_TEST_QUIT_SLEEPER";
    if std::env::var_os(SLEEPER_VAR).is_some() {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    if let Some(dir) = std::env::var_os(VAR) {
        crate::config::set_config_dir(PathBuf::from(dir));

        // Simulate a daemon child spawned moments before quit begins, with
        // no pidfile or listening socket yet.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::spawn::tests::stop_for_quit_blocks_respawn_and_reaps_in_flight_spawn",
            ])
            .env(SLEEPER_VAR, "1")
            .spawn()
            .unwrap();
        let child_pid = child.id();
        super::LAST_SPAWNED_PID.store(child_pid, std::sync::atomic::Ordering::SeqCst);

        super::stop_for_quit();

        // Wait briefly for OS process table to reflect termination.
        let gone = super::wait_for_recorded_exit(child_pid, Duration::from_secs(2));
        if !gone {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(
            gone,
            "in-flight spawned process pid={child_pid} survived stop_for_quit"
        );

        // A subsequent ensure_running() (e.g., from LocalLink or RemoteTerminal)
        // must fail fast without waiting for STARTUP_TIMEOUT or spawning.
        let started = std::time::Instant::now();
        let err = ensure_running()
            .expect_err("ensure_running must refuse to respawn after stop_for_quit");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "ensure_running took {:?} instead of failing fast: {err}",
            started.elapsed()
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "daemon::spawn::tests::stop_for_quit_blocks_respawn_and_reaps_in_flight_spawn",
            "--nocapture",
        ])
        .env(VAR, dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
