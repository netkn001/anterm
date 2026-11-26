use std::io::{self, Write};

use super::codec::kind;
use super::*;

const SIZE: WinSize = WinSize {
    cols: 80,
    rows: 24,
    cell_w: 8,
    cell_h: 17,
};

#[test]
fn full_session_round_trips_over_a_real_duplex_stream() {
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let client_msgs = vec![
        ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/work")),
            size: SIZE,
            shell: None,
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        },
        ClientMsg::Resize(SIZE),
        ClientMsg::Input(vec![b'l', b's', b'\r']),
        ClientMsg::Detach,
    ];
    let daemon_msgs = vec![
        DaemonMsg::Spawned { pane_id: 9 },
        DaemonMsg::Snapshot(vec![0x1b, b'[', b'2', b'J']),
        DaemonMsg::Output(b"hello\r\n".to_vec()),
        DaemonMsg::ClipboardWrite(vec![1, 2, 3, 4]),
        DaemonMsg::Prompt {
            active: true,
            at_prompt: true,
            last_exit: Some(0),
        },
        DaemonMsg::Exited { code: Some(0) },
    ];

    let expect_from_client = client_msgs.clone();
    let reply_with = daemon_msgs.clone();
    let daemon = thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let got: Vec<ClientMsg> = (0..expect_from_client.len())
            .map(|_| ClientMsg::read(&mut sock).unwrap())
            .collect();
        for m in &reply_with {
            m.encode(&mut sock).unwrap();
        }
        sock.flush().unwrap();
        got
    });

    let mut sock = TcpStream::connect(addr).unwrap();
    for m in &client_msgs {
        m.encode(&mut sock).unwrap();
    }
    sock.flush().unwrap();
    let got_from_daemon: Vec<DaemonMsg> = (0..daemon_msgs.len())
        .map(|_| DaemonMsg::read(&mut sock).unwrap())
        .collect();

    let got_from_client = daemon.join().unwrap();
    assert_eq!(got_from_client, client_msgs, "daemon decoded client stream");
    assert_eq!(got_from_daemon, daemon_msgs, "client decoded daemon stream");
}

#[test]
fn client_roundtrip() {
    let msgs = vec![
        ClientMsg::Lease(LeaseRequest::Take {
            size: SIZE,
            by: "phone".into(),
        }),
        ClientMsg::Lease(LeaseRequest::Release),
        ClientMsg::Lease(LeaseRequest::Watch),
        ClientMsg::Lease(LeaseRequest::TakeBack),
        ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/tmp/x")),
            size: SIZE,
            shell: None,
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        },
        ClientMsg::Spawn {
            cwd: None,
            size: SIZE,
            shell: None,
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        },
        ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/tmp/x")),
            size: SIZE,
            shell: Some(ShellSpec {
                program: "wsl.exe".into(),
                args: vec!["--distribution".into(), "Ubuntu".into()],
                args_are_anterm_defaults: true,
            }),
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        },
        ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/tmp/x")),
            size: SIZE,
            shell: None,
            owner: Some("bda10e44-02de-44a0-8412-ec1cda2b5f5b".into()),
            restore: None,
            allow_remote_clipboard_write: false,
        },
        ClientMsg::Observe {
            pane_id: 42,
            size: SIZE,
        },
        ClientMsg::ObserveSince {
            pane_id: 42,
            size: SIZE,
            since_offset: Some(1024),
        },
        ClientMsg::ObserveSince {
            pane_id: 42,
            size: SIZE,
            since_offset: None,
        },
        ClientMsg::Attach {
            pane_id: 42,
            size: SIZE,
            allow_remote_clipboard_write: true,
        },
        ClientMsg::Input(vec![0x1b, b'[', b'A', 0, 255]),
        ClientMsg::SendInput {
            pane_id: 42,
            bytes: vec![b'l', b's', b'\r', 0, 255],
        },
        ClientMsg::SendInput {
            pane_id: 7,
            bytes: Vec::new(),
        },
        ClientMsg::AgentEvent {
            pane_id: 7,
            pid: 4242,
            event: r#"{"v":1,"agent":"claude","event":"stop"}"#.into(),
        },
        ClientMsg::Resize(SIZE),
        ClientMsg::ClearScrollback,
        ClientMsg::Detach,
        ClientMsg::Kill { pane_id: 7 },
        ClientMsg::List,
        ClientMsg::Shutdown,
        ClientMsg::EnsureLoopbackForward(LoopbackForwardRequest {
            pane_id: 7,
            remote_host: "127.0.0.1".into(),
            remote_port: 3000,
        }),
        ClientMsg::ListKnownHosts,
        ClientMsg::DeleteKnownHost(KnownHostId {
            host: "example.com".into(),
            key_type: "ssh-ed25519".into(),
            keyblob: "AAAAC3Nz".into(),
        }),
        ClientMsg::SftpList {
            pane_id: 4,
            path: "/home/deploy/项目".into(),
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::Mkdir {
                path: "/tmp/new dir".into(),
            },
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::Rename {
                from: "/a".into(),
                to: "/b".into(),
            },
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::Chmod {
                path: "/x".into(),
                mode: 0o755,
            },
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::Readlink {
                path: "/link".into(),
            },
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::Realpath { path: ".".into() },
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::ReadFile {
                path: "/etc/nginx/nginx.conf".into(),
                max_bytes: 4 * 1024 * 1024,
            },
        },
        ClientMsg::SftpOp {
            pane_id: 4,
            op: SftpOp::WriteFile {
                path: "/home/deploy/笔记.md".into(),
                bytes: vec![0x00, 0xff, b'h', b'i'],
            },
        },
        ClientMsg::SftpTransferStart(SftpTransferSpec {
            pane_id: 4,
            kind: SftpTransferKind::Upload,
            local: PathBuf::from("/local/f"),
            remote: "/remote/f".into(),
            recursive: true,
        }),
        ClientMsg::SftpTransferCancel { job_id: 9 },
        ClientMsg::SftpTransferList { pane_id: 4 },
        ClientMsg::AddForward {
            pane_id: 7,
            rule: SshForwardRule {
                kind: SshForwardKind::Local,
                bind_host: "127.0.0.1".into(),
                bind_port: 8080,
                target_host: "10.0.0.5".into(),
                target_port: 80,
                description: Some("web".into()),
                enabled: true,
            },
        },
        ClientMsg::AddForward {
            pane_id: 7,
            rule: SshForwardRule {
                kind: SshForwardKind::Dynamic,
                bind_host: "127.0.0.1".into(),
                bind_port: 1080,
                target_host: String::new(),
                target_port: 0,
                description: None,
                enabled: false,
            },
        },
        ClientMsg::RemoveForward {
            pane_id: 7,
            forward_id: 3,
        },
        ClientMsg::SetForwardEnabled {
            pane_id: 7,
            forward_id: 3,
            enabled: false,
        },
        ClientMsg::ListForwards { pane_id: 7 },
        ClientMsg::Version,
    ];
    let mut buf = Vec::new();
    for m in &msgs {
        m.encode(&mut buf).unwrap();
    }
    let mut cursor = std::io::Cursor::new(buf);
    for m in &msgs {
        assert_eq!(*m, ClientMsg::read(&mut cursor).unwrap());
    }
}

#[test]
fn daemon_roundtrip() {
    let msgs = vec![
        DaemonMsg::Spawned { pane_id: 1 },
        DaemonMsg::Lease(Some("Thomas's iPhone".into())),
        DaemonMsg::Lease(None),
        DaemonMsg::ReplayCursor {
            offset: 4096,
            mode: ReplayMode::Delta,
        },
        DaemonMsg::ReplayCursor {
            offset: 0,
            mode: ReplayMode::Full,
        },
        DaemonMsg::Size(SIZE),
        DaemonMsg::Snapshot(vec![1, 2, 3, 0, 255]),
        DaemonMsg::Output((0u8..=255).collect()),
        DaemonMsg::Cwd(PathBuf::from("/home/u/dev")),
        DaemonMsg::Prompt {
            active: true,
            at_prompt: false,
            last_exit: Some(130),
        },
        DaemonMsg::Exited { code: Some(0) },
        DaemonMsg::Exited { code: None },
        DaemonMsg::PaneList(vec![
            PaneInfo {
                pane_id: 3,
                cwd: Some(PathBuf::from("/x")),
                title: "zsh".into(),
                osc_title: Some("user@host:~/x".into()),
                alive: true,
                owner: None,
            },
            PaneInfo {
                pane_id: 4,
                cwd: None,
                title: String::new(),
                osc_title: None,
                alive: true,
                owner: Some("ffe038d0-9ad6-40c0-815d-1fcc43c17ec0".into()),
            },
        ]),
        DaemonMsg::InputAck { pane_id: 42 },
        DaemonMsg::RemoteContext(Some(RemoteContext {
            kind: RemoteKind::Ssh,
            argv: vec!["ssh".into(), "-p".into(), "2222".into(), "dev".into()],
            target: "dev".into(),
        })),
        DaemonMsg::RemoteContext(Some(RemoteContext {
            kind: RemoteKind::Wsl,
            argv: Vec::new(),
            target: "Ubuntu-24.04".into(),
        })),
        DaemonMsg::RemoteContext(None),
        DaemonMsg::Agent(Some(crate::cli_agent::CLIAgent::Claude)),
        DaemonMsg::Agent(Some(crate::cli_agent::CLIAgent::Codex)),
        DaemonMsg::Agent(None),
        DaemonMsg::AgentStatus(Some(crate::cli_agent::AgentSessionState {
            status: crate::cli_agent::AgentStatus::Waiting,
            message: Some("Claude needs your permission to use Bash".into()),
            session_id: Some("abc-123".into()),
            launch_argv: Some(vec![
                "claude".into(),
                "--dangerously-skip-permissions".into(),
            ]),
            rich: true,
            cwd: Some("/repo/.claude/worktrees/fix-x".into()),
            activity: 12,
            turns: 4,
            inferred: false,
            readout: Default::default(),
            since_ms: Some(1_760_000_000_000),
            transcript_path: Some("/tmp/claude/sess.jsonl".into()),
        })),
        DaemonMsg::AgentStatus(None),
        DaemonMsg::LoopbackForward(LoopbackForward { local_port: 49152 }),
        DaemonMsg::KnownHostsList(vec![KnownHostEntry {
            host: "example.com".into(),
            marker: Some("@revoked".into()),
            key_type: "ssh-ed25519".into(),
            fingerprint_sha256: "SHA256:abc".into(),
            id: KnownHostId {
                host: "example.com".into(),
                key_type: "ssh-ed25519".into(),
                keyblob: "AAAAC3Nz".into(),
            },
        }]),
        DaemonMsg::SshTestResult(SshTestReport::Authenticated { elapsed_ms: 640 }),
        DaemonMsg::SshTestResult(SshTestReport::NeedsInput {
            need: SshTestNeed::HostKeyDecision,
            elapsed_ms: 91,
        }),
        DaemonMsg::SshTestResult(SshTestReport::Failed {
            reason: "connect to 10.0.0.5:2222 failed: Connection refused".into(),
        }),
        DaemonMsg::SftpEntries(vec![
            SftpEntry {
                name: "src".into(),
                kind: SftpEntryKind::Dir,
                size: 4096,
                mtime: 1_700_000_000,
                permissions: 0o40755,
                target_is_dir: false,
            },
            SftpEntry {
                name: "链接".into(),
                kind: SftpEntryKind::Symlink,
                size: 0,
                mtime: 0,
                permissions: 0o120777,
                target_is_dir: true,
            },
        ]),
        DaemonMsg::SftpOpResult(SftpOpResult::Done),
        DaemonMsg::SftpOpResult(SftpOpResult::Link("/target/path".into())),
        DaemonMsg::SftpOpResult(SftpOpResult::Error("permission denied".into())),
        DaemonMsg::SftpOpResult(SftpOpResult::Stat(SftpEntry {
            name: "file".into(),
            kind: SftpEntryKind::File,
            size: 12,
            mtime: 5,
            permissions: 0o100644,
            target_is_dir: false,
        })),
        DaemonMsg::SftpOpResult(SftpOpResult::File {
            entry: SftpEntry {
                name: "nginx.conf".into(),
                kind: SftpEntryKind::File,
                size: 4,
                mtime: 1_700_000_000,
                permissions: 0o100644,
                target_is_dir: false,
            },
            bytes: vec![0x00, 0xff, 0x80, b'!'],
        }),
        DaemonMsg::SftpTransferStarted { job_id: 3 },
        DaemonMsg::SftpTransferProgress(vec![SftpJobProgress {
            job_id: 3,
            pane_id: 4,
            kind: SftpTransferKind::Download,
            state: SftpJobState::Running,
            current: "big.iso".into(),
            bytes_done: 1024,
            bytes_total: 4096,
            error: None,
            local: "/local".into(),
            remote: "/remote".into(),
        }]),
        DaemonMsg::ForwardList(vec![
            ManagedForward {
                id: 1,
                pane_id: 7,
                kind: SshForwardKind::Local,
                bind_host: "127.0.0.1".into(),
                bind_port: 8080,
                target_host: "10.0.0.5".into(),
                target_port: 80,
                description: Some("web".into()),
                status: ForwardStatus::Listening,
                enabled: true,
            },
            ManagedForward {
                id: 2,
                pane_id: 7,
                kind: SshForwardKind::Remote,
                bind_host: "0.0.0.0".into(),
                bind_port: 9000,
                target_host: "127.0.0.1".into(),
                target_port: 3000,
                description: None,
                status: ForwardStatus::Error("bind refused".into()),
                enabled: false,
            },
        ]),
        DaemonMsg::Version(DaemonVersion {
            protocol: PROTOCOL_VERSION,
            build: "0.15.0".into(),
            features: vec!["control".into(), "host-rpc".into()],
            instance: "inst-a".into(),
        }),
        DaemonMsg::Version(DaemonVersion {
            protocol: PROTOCOL_VERSION,
            build: "0.15.0".into(),
            features: Vec::new(),
            instance: String::new(),
        }),
        DaemonMsg::Error("nope".into()),
    ];
    let mut buf = Vec::new();
    for m in &msgs {
        m.encode(&mut buf).unwrap();
    }
    let mut cursor = std::io::Cursor::new(buf);
    for m in &msgs {
        assert_eq!(*m, DaemonMsg::read(&mut cursor).unwrap());
    }
}

#[test]
fn default_spawn_stays_wire_compatible_with_old_daemons() {
    let msg = ClientMsg::Spawn {
        cwd: Some(PathBuf::from("/work")),
        size: SIZE,
        shell: None,
        owner: None,
        restore: None,
        allow_remote_clipboard_write: false,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(k, kind::SPAWN, "default spawn must use the legacy kind");
    let (cwd, size): (Option<PathBuf>, WinSize) = serde_json::from_slice(&payload).unwrap();
    assert_eq!(cwd, Some(PathBuf::from("/work")));
    assert_eq!(size, SIZE);

    let legacy = serde_json::to_vec(&(Some(PathBuf::from("/old")), SIZE)).unwrap();
    let decoded = ClientMsg::from_frame(kind::SPAWN, legacy).unwrap();
    assert_eq!(
        decoded,
        ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/old")),
            size: SIZE,
            shell: None,
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        }
    );
}

#[test]
fn explicit_shell_spawn_uses_shell_kind() {
    let shell = ShellSpec {
        program: "fish".to_string(),
        args: vec!["-l".to_string()],
        args_are_anterm_defaults: true,
    };
    let msg = ClientMsg::Spawn {
        cwd: Some(PathBuf::from("/work")),
        size: SIZE,
        shell: Some(shell.clone()),
        owner: None,
        restore: None,
        allow_remote_clipboard_write: false,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(k, kind::SPAWN_SHELL);
    let decoded = ClientMsg::from_frame(k, payload).unwrap();
    assert_eq!(
        decoded,
        ClientMsg::Spawn {
            cwd: Some(PathBuf::from("/work")),
            size: SIZE,
            shell: Some(shell),
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        }
    );
}

#[test]
fn owned_spawn_uses_the_owned_kind_and_round_trips() {
    let msg = ClientMsg::Spawn {
        cwd: Some(PathBuf::from("/work")),
        size: SIZE,
        shell: Some(ShellSpec {
            program: "fish".into(),
            args: vec!["-l".into()],
            args_are_anterm_defaults: false,
        }),
        owner: Some("bda10e44-02de-44a0-8412-ec1cda2b5f5b".into()),
        restore: None,
        allow_remote_clipboard_write: true,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(k, kind::SPAWN_OWNED);
    assert_eq!(ClientMsg::from_frame(k, payload).unwrap(), msg);
}

#[test]
fn owned_spawn_payload_tolerates_unknown_and_missing_fields() {
    let payload = serde_json::to_vec(&serde_json::json!({
        "size": {"cols": 80, "rows": 24, "cell_w": 8, "cell_h": 17},
        "some_future_field": true,
    }))
    .unwrap();
    let decoded = ClientMsg::from_frame(kind::SPAWN_OWNED, payload).unwrap();
    assert_eq!(
        decoded,
        ClientMsg::Spawn {
            cwd: None,
            size: WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 17
            },
            shell: None,
            owner: None,
            restore: None,
            allow_remote_clipboard_write: false,
        }
    );
}

#[test]
fn observe_uses_its_own_kind_and_round_trips() {
    let msg = ClientMsg::Observe {
        pane_id: 42,
        size: SIZE,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    let (k, payload) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(k, kind::OBSERVE);
    assert_ne!(k, kind::ATTACH, "observing must never preempt an attach");
    assert_eq!(ClientMsg::from_frame(k, payload).unwrap(), msg);
}

#[test]
fn pane_info_owner_defaults_for_old_daemons() {
    let old = serde_json::json!({"pane_id": 3, "title": "zsh", "alive": true});
    let info: PaneInfo = serde_json::from_value(old).unwrap();
    assert_eq!(info.owner, None);
    assert!(info.alive);
}

#[test]
fn frame_edges() {
    let mut buf = Vec::new();
    write_frame(&mut buf, 3, &[]).unwrap();
    let mut cursor = std::io::Cursor::new(&buf);
    assert_eq!(read_frame(&mut cursor).unwrap(), (3, vec![]));

    let mut bad = Vec::new();
    bad.extend_from_slice(&(u32::MAX).to_le_bytes());
    bad.push(3);
    let mut cursor = std::io::Cursor::new(&bad);
    assert!(read_frame(&mut cursor).is_err());
}

#[test]
fn write_frame_rejects_oversize_payload() {
    let oversize = vec![0u8; MAX_FRAME + 1];
    let mut buf = Vec::new();
    assert!(write_frame(&mut buf, 3, &oversize).is_err());
    assert!(buf.is_empty());
}

/// The pane socket has `TCP_NODELAY` set, so a write is a segment and a
/// segment is a wakeup on the far side. A frame must therefore cost one
/// write, not one for the length, one for the kind and one for the payload
/// (issue #713) — at flood rates that difference is tens of thousands of
/// syscalls a second on each end.
#[test]
fn a_frame_is_one_write_on_a_vectored_writer() {
    #[derive(Default)]
    struct Counting {
        writes: usize,
        bytes: Vec<u8>,
    }
    impl Write for Counting {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
            self.writes += 1;
            let mut n = 0;
            for b in bufs {
                self.bytes.extend_from_slice(b);
                n += b.len();
            }
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut w = Counting::default();
    write_frame(&mut w, kind::OUTPUT, b"a chunk of pty output").expect("write the frame");
    assert_eq!(w.writes, 1, "one frame must cost one write");

    // An empty payload is a frame too — the header still has to land, and
    // the empty second slice must not spin the loop.
    let mut empty = Counting::default();
    write_frame(&mut empty, kind::DETACH, &[]).expect("write the empty frame");
    assert_eq!(empty.writes, 1);

    // Whatever the write count, the bytes on the wire are unchanged: a
    // `read_frame` over them gives back exactly what went in.
    let mut cursor = io::Cursor::new(w.bytes);
    assert_eq!(
        read_frame(&mut cursor).expect("read it back"),
        (kind::OUTPUT, b"a chunk of pty output".to_vec())
    );
}

#[test]
fn from_frame_rejects_unknown_kind() {
    assert!(ClientMsg::from_frame(99, vec![]).is_err());
    assert!(DaemonMsg::from_frame(99, vec![]).is_err());
}

#[test]
fn take_frame_is_resumable_and_mirrors_read_frame() {
    let mut wire = Vec::new();
    write_frame(&mut wire, 3, b"hello").unwrap();
    write_frame(&mut wire, 9, &[]).unwrap();

    let mut buf = Vec::new();
    let mut got = Vec::new();
    for &b in &wire {
        buf.push(b);
        while let Some(frame) = take_frame(&mut buf).unwrap() {
            got.push(frame);
        }
    }
    assert_eq!(got, vec![(3, b"hello".to_vec()), (9, vec![])]);
    assert!(buf.is_empty(), "nothing left over after both frames");

    let mut buf = Vec::new();
    write_frame(&mut buf, 3, b"done").unwrap();
    buf.extend_from_slice(&10u32.to_le_bytes());
    assert_eq!(take_frame(&mut buf).unwrap(), Some((3, b"done".to_vec())));
    assert_eq!(take_frame(&mut buf).unwrap(), None);
    assert_eq!(buf, 10u32.to_le_bytes());

    let mut bad = (u32::MAX).to_le_bytes().to_vec();
    bad.push(3);
    assert!(take_frame(&mut bad).is_err());
}

#[test]
fn read_frame_on_truncated_frame_is_an_error() {
    let mut cut = std::io::Cursor::new(5u32.to_le_bytes().to_vec());
    assert_eq!(
        read_frame(&mut cut).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );

    let mut buf = Vec::new();
    buf.extend_from_slice(&10u32.to_le_bytes());
    buf.push(3);
    buf.extend_from_slice(b"only4");
    let mut cut = std::io::Cursor::new(buf);
    assert_eq!(
        read_frame(&mut cut).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn from_frame_rejects_malformed_json_payloads() {
    assert!(ClientMsg::from_frame(kind::SPAWN, b"not json".to_vec()).is_err());
    assert!(DaemonMsg::from_frame(kind::PANE_LIST, b"{oops".to_vec()).is_err());
}

#[test]
fn read_frame_on_empty_input_is_eof() {
    let mut empty = std::io::Cursor::new(Vec::<u8>::new());
    let err = read_frame(&mut empty).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    let mut empty2 = std::io::Cursor::new(Vec::<u8>::new());
    assert!(ClientMsg::read(&mut empty2).is_err());
}

/// The port probe's verdict travels over the same wire as the ports, and
/// the two ends of that wire are regularly different builds: a remote
/// workspace's panes are answered for by whatever `anterm-server` is
/// installed on the peer. An older one says nothing about the probe, and
/// its silence has to read as "this list is the whole answer" rather than
/// failing the frame or, worse, arriving as a doubt the panel then shows.
#[test]
fn a_procs_answer_without_a_probe_verdict_is_a_complete_one() {
    let old = r#"{"procs":[],"ports":[{"port":3000,"pid":7,"name":"node"}]}"#;
    let procs: PaneProcs = serde_json::from_str(old).unwrap();
    assert_eq!(procs.probe, PortProbe::Ok);
    assert!(procs.probe.is_ok());
    assert_eq!(procs.ports[0].addr, "");

    for probe in [
        PortProbe::Ok,
        PortProbe::Restricted,
        PortProbe::Unavailable("lsof: not found".into()),
    ] {
        let wire = serde_json::to_string(&PaneProcs {
            probe: probe.clone(),
            ..Default::default()
        })
        .unwrap();
        let back: PaneProcs = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.probe, probe, "round trip through {wire}");
    }
}

#[test]
fn pane_info_deserializes_with_defaults() {
    let info: PaneInfo = serde_json::from_str(r#"{"pane_id": 5, "alive": true}"#).unwrap();
    assert_eq!(info.pane_id, 5);
    assert!(info.alive);
    assert_eq!(info.cwd, None);
    assert_eq!(info.title, "");
}

/// The reason `previously_known_as` is a field and not a variant: this
/// prompt is decoded by whatever GUI or `anterm-server` is at the other end,
/// and one of them is regularly older than the build that sent it. A
/// missing field defaults; an unknown variant would fail the whole frame.
#[test]
fn an_unknown_host_prompt_from_an_older_peer_still_decodes() {
    let prompt: AuthPromptKind = serde_json::from_str(
        r#"{"HostKeyUnknown":{"host":"h","port":22,"algorithm":"ssh-ed25519","fingerprint_sha256":"SHA256:x"}}"#,
    )
    .unwrap();
    assert_eq!(
        prompt,
        AuthPromptKind::HostKeyUnknown {
            host: "h".into(),
            port: 22,
            algorithm: "ssh-ed25519".into(),
            fingerprint_sha256: "SHA256:x".into(),
            previously_known_as: None,
        }
    );
}

fn sample_native_spec() -> NativeSshSpec {
    let mut passphrases = std::collections::HashMap::new();
    passphrases.insert("~/.ssh/id_ed25519".to_string(), "topsecret".to_string());
    NativeSshSpec {
        host: "example.com".into(),
        port: 2222,
        user: "deploy".into(),
        auth_mode: SshAuthMode::Auto,
        identity_files: vec!["~/.ssh/id_ed25519".into()],
        agent_forward: true,
        password: Some("hunter2".into()),
        key_passphrases: Some(passphrases),
        proxy: SshProxy::Socks {
            host: "127.0.0.1".into(),
            port: 1080,
        },
        jump: Some(Box::new(NativeSshSpec {
            host: "bastion".into(),
            port: 22,
            user: "jump".into(),
            auth_mode: SshAuthMode::Agent,
            identity_files: vec![],
            agent_forward: false,
            password: Some("jumppass".into()),
            key_passphrases: None,
            proxy: SshProxy::None,
            jump: None,
            forwards: vec![],
            keepalive_interval_s: None,
            keepalive_count_max: None,
            connect_timeout_s: None,
            algorithms: SshAlgorithms::default(),
            x11: false,
            term: "xterm-256color".into(),
            verify_host_keys: true,
            skip_banner: false,
            shell_integration: true,
            remote_clipboard_write: false,
            login_script: vec![],
            display_name: None,
            profile_id: None,
        })),
        forwards: vec![SshForwardRule {
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: 8000,
            target_host: "127.0.0.1".into(),
            target_port: 80,
            description: Some("web".into()),
            enabled: true,
        }],
        keepalive_interval_s: Some(30),
        keepalive_count_max: Some(3),
        connect_timeout_s: Some(20),
        algorithms: SshAlgorithms {
            cipher: vec!["aes256-ctr".into()],
            ..Default::default()
        },
        x11: true,
        term: "xterm-256color".into(),
        verify_host_keys: true,
        skip_banner: false,
        shell_integration: true,
        remote_clipboard_write: true,
        login_script: vec!["tmux attach".into()],
        display_name: Some("prod-web".into()),
        profile_id: Some("uuid-1".into()),
    }
}

#[test]
fn native_ssh_spec_serde_round_trips() {
    let spec = sample_native_spec();
    let json = serde_json::to_string(&spec).unwrap();
    let back: NativeSshSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(spec, back);
}

#[test]
fn native_ssh_spec_tolerates_minimal_json() {
    let spec: NativeSshSpec =
        serde_json::from_str(r#"{"host":"h","port":22,"user":"u","auth_mode":"auto"}"#)
            .unwrap();
    assert_eq!(spec.term, "xterm-256color");
    assert!(spec.verify_host_keys);
    assert!(spec.shell_integration);
    assert_eq!(spec.password, None);
    assert!(spec.jump.is_none());
}

#[test]
fn secrets_are_redacted_in_debug_output() {
    let spec = sample_native_spec();
    let dbg = format!("{spec:?}");
    assert!(!dbg.contains("hunter2"), "password leaked: {dbg}");
    assert!(!dbg.contains("topsecret"), "passphrase leaked: {dbg}");
    assert!(!dbg.contains("jumppass"), "jump password leaked: {dbg}");
    assert!(dbg.contains("<redacted>"));

    assert_eq!(
        format!("{:?}", AuthResponse::Secret("pw".into())),
        "Secret(<redacted>)"
    );
    assert_eq!(
        format!("{:?}", AuthResponse::Secrets(vec!["a".into(), "b".into()])),
        "Secrets(<2 redacted>)"
    );
}

#[test]
fn without_secrets_strips_password_and_passphrases_recursively() {
    let clean = sample_native_spec().without_secrets();
    assert_eq!(clean.password, None);
    assert!(clean.key_passphrases.is_none());
    assert_eq!(clean.jump.as_ref().unwrap().password, None);
    assert_eq!(clean.host, "example.com");
    assert_eq!(clean.login_script, vec!["tmux attach".to_string()]);
}

#[test]
fn native_ssh_messages_round_trip() {
    let client_msgs = vec![
        ClientMsg::SpawnNativeSsh {
            cwd: Some(PathBuf::from("/work")),
            size: SIZE,
            spec: Box::new(sample_native_spec()),
        },
        ClientMsg::AuthResponse {
            request_id: 7,
            response: AuthResponse::Secret("pw".into()),
        },
        ClientMsg::AuthResponse {
            request_id: 8,
            response: AuthResponse::HostKeyDecision {
                accept: true,
                remember: true,
            },
        },
        ClientMsg::TestSsh {
            spec: Box::new(sample_native_spec()),
        },
    ];
    let mut buf = Vec::new();
    for m in &client_msgs {
        m.encode(&mut buf).unwrap();
    }
    let mut cur = std::io::Cursor::new(buf);
    for m in &client_msgs {
        assert_eq!(*m, ClientMsg::read(&mut cur).unwrap());
    }

    let daemon_msgs = vec![
        DaemonMsg::AuthPrompt {
            request_id: 1,
            prompt: AuthPromptKind::HostKeyChanged {
                host: "h".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:new".into(),
                old_fingerprint_sha256: "SHA256:old".into(),
            },
        },
        DaemonMsg::AuthPrompt {
            request_id: 2,
            prompt: AuthPromptKind::KeyboardInteractive {
                name: "2FA".into(),
                instructions: "enter code".into(),
                prompts: vec![KiPrompt {
                    text: "Code:".into(),
                    echo: true,
                }],
                stored_rejected: true,
            },
        },
        DaemonMsg::AuthPrompt {
            request_id: 3,
            prompt: AuthPromptKind::KeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                comment: String::new(),
                rejected: true,
            },
        },
        DaemonMsg::SshStatus {
            phase: SshPhase::Failed {
                reason: "nope".into(),
            },
        },
    ];
    let mut buf = Vec::new();
    for m in &daemon_msgs {
        m.encode(&mut buf).unwrap();
    }
    let mut cur = std::io::Cursor::new(buf);
    for m in &daemon_msgs {
        assert_eq!(*m, DaemonMsg::read(&mut cur).unwrap());
    }
}

/// `rejected` rides on a struct variant of an externally tagged enum that
/// crosses both daemon↔GUI and GUI↔remote `anterm-server`, so it has to
/// survive a peer that predates it in *either* direction. That is why
/// `PROTOCOL_VERSION` did not move for it: the remote handshake gates on
/// it, and bumping would turn every older server away over a field it can
/// safely ignore.
#[test]
fn a_rejected_passphrase_flag_decodes_from_a_peer_that_never_sends_it() {
    let old = r#"{"KeyPassphrase":{"key_path":"~/.ssh/id_ed25519","comment":""}}"#;
    assert_eq!(
        serde_json::from_str::<AuthPromptKind>(old).unwrap(),
        AuthPromptKind::KeyPassphrase {
            key_path: "~/.ssh/id_ed25519".into(),
            comment: String::new(),
            rejected: false,
        }
    );

    // The other direction: a peer that predates the flag is handed one
    // set, and must read the prompt rather than reject the frame.
    let new = serde_json::to_string(&AuthPromptKind::KeyPassphrase {
        key_path: "/k".into(),
        comment: "work laptop".into(),
        rejected: true,
    })
    .unwrap();
    #[derive(Deserialize)]
    enum LegacyPromptKind {
        KeyPassphrase { key_path: String, comment: String },
    }
    let LegacyPromptKind::KeyPassphrase { key_path, comment } =
        serde_json::from_str::<LegacyPromptKind>(&new).unwrap();
    assert_eq!(key_path, "/k");
    assert_eq!(comment, "work laptop");
}

/// `stored_rejected` gets the same treatment as `rejected` above, for the
/// same reason and with the same `PROTOCOL_VERSION` left alone.
#[test]
fn a_stored_rejected_flag_decodes_from_a_peer_that_never_sends_it() {
    let old = r#"{"KeyboardInteractive":{"name":"2FA","instructions":"","prompts":[]}}"#;
    assert_eq!(
        serde_json::from_str::<AuthPromptKind>(old).unwrap(),
        AuthPromptKind::KeyboardInteractive {
            name: "2FA".into(),
            instructions: String::new(),
            prompts: vec![],
            stored_rejected: false,
        }
    );

    let new = serde_json::to_string(&AuthPromptKind::KeyboardInteractive {
        name: "2FA".into(),
        instructions: "code".into(),
        prompts: vec![],
        stored_rejected: true,
    })
    .unwrap();
    #[derive(Deserialize)]
    enum LegacyPromptKind {
        KeyboardInteractive { name: String, instructions: String },
    }
    let LegacyPromptKind::KeyboardInteractive { name, instructions } =
        serde_json::from_str::<LegacyPromptKind>(&new).unwrap();
    assert_eq!(name, "2FA");
    assert_eq!(instructions, "code");
}

#[test]
fn native_ssh_spawn_uses_new_kind_byte() {
    let msg = ClientMsg::SpawnNativeSsh {
        cwd: None,
        size: SIZE,
        spec: Box::new(sample_native_spec()),
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    let (k, _) = read_frame(&mut std::io::Cursor::new(&buf)).unwrap();
    assert_eq!(k, kind::SPAWN_NATIVE_SSH);
}

#[test]
fn a_version_reply_without_features_still_decodes() {
    let legacy = br#"{"protocol":2,"build":"26.7.4"}"#;
    let v: DaemonVersion = serde_json::from_slice(legacy).unwrap();
    assert_eq!(v.protocol, 2);
    assert_eq!(v.build, "26.7.4");
    assert!(v.features.is_empty());
    assert!(!v.has_feature(crate::pane::feature::CONTROL));
}

#[test]
fn a_version_reply_with_unknown_fields_still_decodes() {
    let future = br#"{"protocol":4,"build":"99.0.0","features":["control"],
                      "something_new":{"a":1}}"#;
    let v: DaemonVersion = serde_json::from_slice(future).unwrap();
    assert_eq!(v.protocol, 4);
    assert!(v.has_feature(crate::pane::feature::CONTROL));
}

#[test]
fn the_local_daemon_does_not_claim_the_control_dialect() {
    let v = DaemonVersion::current();
    assert_eq!(v.protocol, PROTOCOL_VERSION);
    assert!(
        !v.has_feature(crate::pane::feature::CONTROL),
        "the session daemon must not advertise a dialect it cannot serve"
    );
}

/// `enabled` crosses the same peers as `rejected` above and gets the same
/// treatment: a rule or a forward from a peer that predates the switch is
/// a live one, and a peer that predates it reads a frame carrying it.
#[test]
fn a_forward_enabled_flag_decodes_from_a_peer_that_never_sends_it() {
    let rule: SshForwardRule = serde_json::from_str(
        r#"{"kind":"local","bind_host":"127.0.0.1","bind_port":8080,
            "target_host":"srv","target_port":80}"#,
    )
    .unwrap();
    assert!(rule.enabled);

    let managed: ManagedForward = serde_json::from_str(
        r#"{"id":1,"pane_id":7,"kind":"local","bind_host":"127.0.0.1",
            "bind_port":8080,"status":"listening"}"#,
    )
    .unwrap();
    assert!(managed.enabled);

    #[derive(Deserialize)]
    struct LegacyManagedForward {
        id: u64,
        status: ForwardStatus,
    }
    let off = serde_json::to_string(&ManagedForward {
        enabled: false,
        status: ForwardStatus::Error("disabled".into()),
        ..managed
    })
    .unwrap();
    let legacy: LegacyManagedForward = serde_json::from_str(&off).unwrap();
    assert_eq!(legacy.id, 1);
    assert_eq!(legacy.status, ForwardStatus::Error("disabled".into()));
}
