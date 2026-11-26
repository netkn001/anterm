use super::client::ClientInner;
use super::codec::{CONTROL_HEADER, encode_body};
use super::*;
use crate::daemon::protocol::{MAX_FRAME, write_frame};
use crate::util::to_json;
use std::collections::HashMap;
use std::io::{self, Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

fn meta() -> Meta {
    Meta {
        is_dir: false,
        is_symlink: false,
        len: 4096,
        mtime: Some(MTime {
            secs: 1_769_000_000,
            nanos: 123_456_789,
        }),
        readonly: false,
    }
}

fn detached_inner() -> Arc<ClientInner> {
    Arc::new(ClientInner {
        writer: Mutex::new(Box::new(Cursor::new(Vec::new()))),
        next_req_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        blobs: Mutex::new(HashMap::new()),
        connected: AtomicBool::new(true),
        last_inbound: Mutex::new(Instant::now()),
        last_rtt_us: AtomicU64::new(0),
        hello: ControlHelloOk {
            control_version: CONTROL_VERSION,
            protocol_version: 0,
            build: "test".into(),
            separator: '/',
            home: "/home/me".into(),
            features: Vec::new(),
            instance: "test-instance".into(),
        },
        shutdown: None,
        reader_done: Mutex::new(false),
        reader_exit: Condvar::new(),
        link_down: Mutex::new(Vec::new()),
    })
}

fn unrepresentable_path() -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.rs"))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt as _;
        PathBuf::from(std::ffi::OsString::from_wide(&[
            0x0043, 0x003a, 0x005c, 0xd800, 0x002e, 0x0072, 0x0073,
        ]))
    }
}

#[test]
fn to_frame_refuses_what_cannot_be_sent_without_writing_it() {
    let unrepresentable = ControlServerMsg::Response {
        req_id: 1,
        reply: ControlReply::Ok(ReplyOk::Hits(vec![SearchHit {
            name: "caf\u{fffd}.rs".into(),
            path: unrepresentable_path(),
            is_dir: false,
            ignored: false,
        }])),
    };
    assert!(
        unrepresentable.to_frame().is_err(),
        "a path that is not valid Unicode must not encode"
    );

    let huge = ControlServerMsg::ResponseBlob {
        req_id: 1,
        reply: ControlReply::Ok(ReplyOk::Meta(meta())),
        blob: vec![0u8; MAX_FRAME + 1],
    };
    let err = huge
        .to_frame()
        .expect_err("an oversize reply must not encode");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn a_frame_too_large_to_send_does_not_condemn_the_link() {
    let inner = detached_inner();
    let oversize = ControlClientMsg::RequestBlob {
        req_id: 1,
        req: ControlRequest::WriteFile {
            path: "/home/me/huge.bin".into(),
        },
        blob: vec![0u8; MAX_FRAME + 1],
    };

    let err = inner
        .send(&oversize)
        .expect_err("an oversize frame must fail");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        inner.connected.load(Ordering::Acquire),
        "a local encode failure marked the connection dead"
    );

    inner.send(&ControlClientMsg::Cancel { req_id: 1 }).unwrap();
}

#[test]
fn a_blob_racing_its_callers_timeout_is_not_retained() {
    for req_id in 1..2_000u64 {
        let inner = detached_inner();
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        inner.pending.lock().unwrap().insert(req_id, tx);

        let giving_up = Arc::clone(&inner);
        let t = std::thread::spawn(move || giving_up.forget(req_id));
        inner.deliver(req_id, ControlReply::Ok(ReplyOk::Unit), vec![7u8; 64]);
        t.join().unwrap();

        assert!(
            inner.blobs.lock().unwrap().is_empty(),
            "a blob outlived the request it belonged to (req_id {req_id})"
        );
    }
}

fn every_request() -> Vec<ControlRequest> {
    vec![
        ControlRequest::Ping,
        ControlRequest::ReadDir {
            dir: "/home/me/proj/src".into(),
            root: Some("/home/me/proj".into()),
        },
        ControlRequest::ReadDir {
            dir: "/tmp".into(),
            root: None,
        },
        ControlRequest::Stat {
            path: "/home/me/.zshrc".into(),
        },
        ControlRequest::Exists {
            path: "/nope".into(),
        },
        ControlRequest::Canonicalize {
            path: "/a/../b".into(),
        },
        ControlRequest::ReadFile {
            path: "/home/me/proj/Cargo.toml".into(),
            max_bytes: 10 * 1024 * 1024,
        },
        ControlRequest::Search {
            roots: vec!["/home/me/proj".into(), "/srv".into()],
            query: "widget".into(),
            limit: 200,
            max_dirs: 2000,
            show_hidden: false,
        },
        ControlRequest::SearchContent {
            roots: vec!["/home/me/proj".into()],
            query: ContentQuery {
                pattern: r"fn \w+".into(),
                case_sensitive: true,
                whole_word: false,
                regex: true,
                show_hidden: false,
            },
            limits: ContentLimits::default(),
        },
        ControlRequest::WriteFile {
            path: "/home/me/proj/src/main.rs".into(),
        },
        ControlRequest::CreateFileNew {
            path: "/home/me/new.txt".into(),
        },
        ControlRequest::CreateDir {
            path: "/home/me/a/b/c".into(),
            recursive: true,
        },
        ControlRequest::Rename {
            from: "/a".into(),
            to: "/b".into(),
        },
        ControlRequest::Remove {
            path: "/home/me/tmp".into(),
            recursive: true,
        },
        ControlRequest::RepoRoot {
            path: "/home/me/proj/src/deep".into(),
        },
        ControlRequest::Git {
            cwd: "/home/me/proj".into(),
            args: vec!["status".into(), "--porcelain".into()],
        },
        ControlRequest::WatchOpen {
            dirs: vec!["/home/me/proj".into()],
        },
        ControlRequest::WatchSet {
            id: 7,
            dirs: vec!["/home/me/proj".into(), "/home/me/proj/src".into()],
        },
        ControlRequest::WatchClose { id: 7 },
        ControlRequest::GuiOpen {
            path: Some("/home/me/proj".into()),
        },
        ControlRequest::TabSetTags {
            tab: TabId::new(),
            tags: vec![crate::tag::TagId::new()],
        },
        ControlRequest::SetTags {
            tags: {
                let mut catalog = crate::tag::SidebarTags::default();
                catalog.create("Ops", crate::tag::TagColor::Blue);
                catalog
            },
        },
        ControlRequest::AgentStates,
        ControlRequest::PaneProcs { pane_id: 7 },
        ControlRequest::Routes,
        ControlRequest::Status,
    ]
}

fn every_reply() -> Vec<ControlReply> {
    vec![
        ControlReply::Ok(ReplyOk::Unit),
        ControlReply::Ok(ReplyOk::Pong),
        ControlReply::Ok(ReplyOk::Entries(vec![
            Entry {
                name: "src".into(),
                is_dir: true,
                is_symlink: false,
                ignored: false,
            },
            Entry {
                name: "target".into(),
                is_dir: true,
                is_symlink: false,
                ignored: true,
            },
            Entry {
                name: "link".into(),
                is_dir: false,
                is_symlink: true,
                ignored: false,
            },
        ])),
        ControlReply::Ok(ReplyOk::Meta(meta())),
        ControlReply::Ok(ReplyOk::Bool(true)),
        ControlReply::Ok(ReplyOk::Bool(false)),
        ControlReply::Ok(ReplyOk::Path("/home/me/proj".into())),
        ControlReply::Ok(ReplyOk::OptPath(Some("/home/me/proj".into()))),
        ControlReply::Ok(ReplyOk::OptPath(None)),
        ControlReply::Ok(ReplyOk::FileMeta { meta: meta() }),
        ControlReply::Ok(ReplyOk::Hits(vec![SearchHit {
            name: "widget.rs".into(),
            path: PathBuf::from("/home/me/proj/src/widget.rs"),
            is_dir: false,
            ignored: false,
        }])),
        ControlReply::Ok(ReplyOk::ContentHits(ContentResults {
            hits: vec![crate::host::ContentHit {
                path: PathBuf::from("/home/me/proj/src/widget.rs"),
                line: 12,
                column: 5,
                text: "fn widget() {".into(),
                ranges: vec![crate::host::ContentRange { start: 3, end: 9 }],
            }],
            truncated: true,
            files_searched: 40,
        })),
        ControlReply::Ok(ReplyOk::Output(Output {
            status: Some(1),
            stdout: b"?? src/new.rs\n".to_vec(),
            stderr: vec![0x00, 0xff, 0xfe, b'\n'],
        })),
        ControlReply::Ok(ReplyOk::WatchId(42)),
        ControlReply::Ok(ReplyOk::AgentStates(vec![PaneAgentState {
            pane_id: 9,
            agent: Some(crate::cli_agent::CLIAgent::Claude),
            state: crate::cli_agent::AgentSessionState {
                status: crate::cli_agent::AgentStatus::Waiting,
                message: Some("needs permission".into()),
                session_id: Some("sess-1".into()),
                launch_argv: Some(vec!["claude".into()]),
                rich: true,
                cwd: Some("/work/api".into()),
                activity: 3,
                turns: 1,
                inferred: false,
                readout: Default::default(),
                since_ms: None,
                transcript_path: None,
            },
        }])),
        ControlReply::Ok(ReplyOk::AgentStates(Vec::new())),
        ControlReply::Ok(ReplyOk::Routes(vec![RouteInfo {
            key: "me@build-box:22".into(),
            kind: "ssh".into(),
            connected: true,
        }])),
        ControlReply::Ok(ReplyOk::Status(ServerStatus {
            pid: 4242,
            uptime_secs: 61,
            panes: 3,
            control_version: CONTROL_VERSION,
            protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
            build: "26.7.5".into(),
            socket: "/run/user/1000/anterm/daemon.sock".into(),
        })),
        ControlReply::Err(WireError::new(WireErrorKind::NotFound, "no such file")),
        ControlReply::Err(WireError::new(
            WireErrorKind::PermissionDenied,
            "permission denied",
        )),
        ControlReply::Err(WireError::new(
            WireErrorKind::AlreadyExists,
            "already exists",
        )),
        ControlReply::Err(WireError::new(WireErrorKind::InvalidInput, "bad path")),
        ControlReply::Err(WireError::new(WireErrorKind::NotADirectory, "not a dir")),
        ControlReply::Err(WireError::new(WireErrorKind::IsADirectory, "is a dir")),
        ControlReply::Err(WireError::new(
            WireErrorKind::DirectoryNotEmpty,
            "not empty",
        )),
        ControlReply::Err(WireError::new(WireErrorKind::FileTooLarge, "too large")),
        ControlReply::Err(WireError::new(WireErrorKind::GitUnavailable, "no git")),
        ControlReply::Err(WireError::new(WireErrorKind::TimedOut, "timed out")),
        ControlReply::Err(WireError::new(WireErrorKind::ConnectionReset, "reset")),
        ControlReply::Err(WireError::new(WireErrorKind::Other, "something else")),
    ]
}

fn every_event() -> Vec<ControlEvent> {
    vec![
        ControlEvent::Watch {
            id: 3,
            paths: vec!["/home/me/proj/src/main.rs".into()],
        },
        ControlEvent::WatchOverflow { id: 3 },
        ControlEvent::PaneExited {
            pane_id: 9,
            code: Some(0),
        },
        ControlEvent::PaneExited {
            pane_id: 9,
            code: None,
        },
        ControlEvent::AgentStatus {
            pane_id: 9,
            json: serde_json::json!({ "state": "thinking" }),
        },
        ControlEvent::GuiOpen {
            path: Some("/home/me/proj".into()),
        },
        ControlEvent::Layout {
            delta: LayoutDelta::TabTagsChanged {
                tab: TabId::new(),
                tags: vec![crate::tag::TagId::new()],
            },
        },
        ControlEvent::Layout {
            delta: LayoutDelta::TagsChanged {
                tags: {
                    let mut catalog = crate::tag::SidebarTags::default();
                    catalog.create("Prod", crate::tag::TagColor::Red);
                    catalog
                },
            },
        },
    ]
}

fn hello() -> ControlHello {
    ControlHello {
        control_version: CONTROL_VERSION,
        client_token: "tok".into(),
        client_hostname: "laptop".into(),
        gui: false,
    }
}

#[test]
fn the_server_instance_is_one_value_per_process() {
    let first = server_instance();
    assert!(!first.is_empty(), "an empty instance means \"unknown\"");
    assert_eq!(first, server_instance());
}

#[test]
fn the_server_start_instant_is_fixed_per_process() {
    let first = server_started();
    assert_eq!(first, server_started(), "uptime needs one fixed origin");
    assert!(first.elapsed() >= Duration::ZERO);
}

#[test]
fn a_hello_without_an_instance_still_decodes() {
    let json = r#"{"control_version":2,"protocol_version":3,"build":"26.7.6",
                   "separator":"/","home":"/home/me"}"#;
    let ok: ControlHelloOk = serde_json::from_str(json).expect("decodes without instance");
    assert_eq!(ok.instance, "");
    assert!(ok.features.is_empty());
}

#[test]
fn an_old_client_hello_defaults_to_a_non_gui_connection() {
    let json = r#"{"control_version":5,"workspace":null,"client_token":"tok",
                   "client_hostname":"laptop"}"#;
    let hello: ControlHello = serde_json::from_str(json).expect("decodes without gui role");
    assert!(!hello.gui);
}

fn hello_ok() -> ControlHelloOk {
    ControlHelloOk {
        control_version: CONTROL_VERSION,
        protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
        build: "26.7.5".into(),
        separator: '/',
        home: "/home/me".into(),
        features: vec![feature::CONTROL.into(), feature::HOST_RPC.into()],
        instance: "test-instance".into(),
    }
}

#[test]
fn client_messages_round_trip() {
    let mut msgs = vec![
        ControlClientMsg::Hello(hello()),
        ControlClientMsg::Hello(ControlHello::host_rpc("tok", "laptop")),
        ControlClientMsg::Cancel { req_id: 1 },
        ControlClientMsg::Cancel { req_id: u64::MAX },
        ControlClientMsg::RequestBlob {
            req_id: 5,
            req: ControlRequest::WriteFile {
                path: "/tmp/x".into(),
            },
            blob: vec![0x00, 0xff, b'h', b'i'],
        },
        ControlClientMsg::RequestBlob {
            req_id: 6,
            req: ControlRequest::WriteFile {
                path: "/tmp/empty".into(),
            },
            blob: Vec::new(),
        },
    ];
    for (i, req) in every_request().into_iter().enumerate() {
        msgs.push(ControlClientMsg::Request {
            req_id: i as u64 + 1,
            req,
        });
    }

    for msg in &msgs {
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let got = ControlClientMsg::read(&mut Cursor::new(&buf)).unwrap();
        assert_eq!(&got, msg, "client message did not survive the round trip");
    }
}

#[test]
fn server_messages_round_trip() {
    let mut msgs = vec![
        ControlServerMsg::HelloOk(hello_ok()),
        ControlServerMsg::ResponseBlob {
            req_id: 9,
            reply: ControlReply::Ok(ReplyOk::FileMeta { meta: meta() }),
            blob: b"file contents\0\xff".to_vec(),
        },
        ControlServerMsg::ResponseBlob {
            req_id: 10,
            reply: ControlReply::Ok(ReplyOk::FileMeta { meta: meta() }),
            blob: Vec::new(),
        },
    ];
    for (i, reply) in every_reply().into_iter().enumerate() {
        msgs.push(ControlServerMsg::Response {
            req_id: i as u64 + 1,
            reply,
        });
    }
    for event in every_event() {
        msgs.push(ControlServerMsg::Event(event));
    }

    for msg in &msgs {
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let got = ControlServerMsg::read(&mut Cursor::new(&buf)).unwrap();
        assert_eq!(&got, msg, "server message did not survive the round trip");
    }
}

#[test]
fn frame_layout_matches_the_contract() {
    let mut buf = Vec::new();
    ControlClientMsg::RequestBlob {
        req_id: 0x0102_0304_0506_0708,
        req: ControlRequest::WriteFile { path: "/x".into() },
        blob: vec![0xaa, 0xbb],
    }
    .encode(&mut buf)
    .unwrap();

    let payload_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    assert_eq!(buf[4], 62, "REQUEST_BLOB is kind 62");
    assert_eq!(
        buf.len(),
        5 + payload_len,
        "frame length covers the payload"
    );

    let payload = &buf[5..];
    assert_eq!(
        &payload[..8],
        &0x0102_0304_0506_0708u64.to_le_bytes(),
        "req_id is 8 bytes, little-endian, first"
    );
    let json_n = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
    let json = &payload[12..12 + json_n];
    assert_eq!(
        serde_json::from_slice::<ControlRequest>(json).unwrap(),
        ControlRequest::WriteFile { path: "/x".into() }
    );
    assert_eq!(&payload[12 + json_n..], &[0xaa, 0xbb], "blob is the tail");
    assert_eq!(
        payload_len,
        12 + json_n + 2,
        "payload_len == 12 + json_n + blob_len"
    );
}

#[test]
fn non_blob_frames_have_no_tail() {
    let mut buf = Vec::new();
    ControlClientMsg::Request {
        req_id: 1,
        req: ControlRequest::Ping,
    }
    .encode(&mut buf)
    .unwrap();
    let payload_len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    let json_n = u32::from_le_bytes(buf[13..17].try_into().unwrap()) as usize;
    assert_eq!(payload_len, 12 + json_n);
}

#[test]
fn cancel_is_twelve_bytes_with_an_empty_json_head() {
    let mut buf = Vec::new();
    ControlClientMsg::Cancel { req_id: 77 }
        .encode(&mut buf)
        .unwrap();
    assert_eq!(u32::from_le_bytes(buf[..4].try_into().unwrap()), 12);
    assert_eq!(buf[4], 63);
    assert_eq!(&buf[5..13], &77u64.to_le_bytes());
    assert_eq!(&buf[13..17], &0u32.to_le_bytes());
    assert_eq!(buf.len(), 17);
}

#[test]
fn a_git_chunk_crosses_the_wire_as_base64() {
    let bytes: Vec<u8> = (0..8192u32).map(|i| b'a' + (i % 26) as u8).collect();
    let event = ControlEvent::GitChunk {
        id: 7,
        bytes: bytes.clone(),
    };
    let json = to_json(&event).unwrap();
    assert!(
        json.len() < bytes.len() * 2,
        "a {}-byte chunk encoded to {} bytes of JSON — the array-of-numbers \
         encoding is back",
        bytes.len(),
        json.len()
    );

    let (k, payload) = ControlServerMsg::Event(event).to_frame().unwrap();
    match ControlServerMsg::from_frame(k, payload).unwrap() {
        ControlServerMsg::Event(ControlEvent::GitChunk { id, bytes: got }) => {
            assert_eq!(id, 7);
            assert_eq!(got, bytes, "the bytes survive the encoding exactly");
        }
        other => panic!("expected a GitChunk, got {other:?}"),
    }
}

#[test]
fn the_git_chunk_ceiling_leaves_room_for_base64_and_the_envelope() {
    let encoded = GIT_STREAM_CHUNK_MAX / 3 * 4 + 4;
    assert!(
        encoded + 4096 < MAX_FRAME,
        "a full chunk encodes to {encoded} bytes against a {MAX_FRAME}-byte frame limit"
    );
    const {
        assert!(
            GIT_STREAM_CHUNK_MAX > GIT_STREAM_CHUNK,
            "the ceiling is a backstop for an oversized line, not the batch target"
        )
    };
}

#[test]
fn events_carry_a_zero_req_id() {
    let mut buf = Vec::new();
    ControlServerMsg::Event(ControlEvent::WatchOverflow { id: 1 })
        .encode(&mut buf)
        .unwrap();
    assert_eq!(buf[4], 63);
    assert_eq!(&buf[5..13], &0u64.to_le_bytes());
}

#[test]
fn short_payload_is_invalid_data() {
    for len in 0..CONTROL_HEADER {
        let payload = vec![0u8; len];
        for k in [kind::REQUEST, kind::REQUEST_BLOB, kind::CANCEL] {
            let e = ControlClientMsg::from_frame(k, payload.clone()).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidData, "kind {k}, len {len}");
        }
        for k in [kind::RESPONSE, kind::RESPONSE_BLOB, kind::EVENT] {
            let e = ControlServerMsg::from_frame(k, payload.clone()).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidData, "kind {k}, len {len}");
        }
    }
}

#[test]
fn oversized_json_len_is_invalid_data_not_a_panic() {
    for json_n in [13u32, 1_000_000, u32::MAX] {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u64.to_le_bytes());
        payload.extend_from_slice(&json_n.to_le_bytes());
        payload.extend_from_slice(b"{}");
        let e = ControlClientMsg::from_frame(kind::REQUEST, payload.clone()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "json_n {json_n}");
        let e = ControlServerMsg::from_frame(kind::RESPONSE_BLOB, payload).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "json_n {json_n}");
    }
}

#[test]
fn trailing_bytes_on_a_non_blob_kind_are_invalid_data() {
    let json = serde_json::to_vec(&ControlRequest::Ping).unwrap();
    let mut payload = encode_body(1, &json, b"extra").unwrap();
    let e = ControlClientMsg::from_frame(kind::REQUEST, payload.clone()).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);

    payload = encode_body(1, &[], b"extra").unwrap();
    let e = ControlClientMsg::from_frame(kind::CANCEL, payload).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);

    let json = serde_json::to_vec(&ControlEvent::WatchOverflow { id: 1 }).unwrap();
    let payload = encode_body(0, &json, b"extra").unwrap();
    let e = ControlServerMsg::from_frame(kind::EVENT, payload).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn event_with_a_nonzero_req_id_is_invalid_data() {
    let json = serde_json::to_vec(&ControlEvent::WatchOverflow { id: 1 }).unwrap();
    let payload = encode_body(9, &json, &[]).unwrap();
    let e = ControlServerMsg::from_frame(kind::EVENT, payload).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn zero_req_id_is_rejected_on_requests_and_responses() {
    let json = serde_json::to_vec(&ControlRequest::Ping).unwrap();
    let payload = encode_body(0, &json, &[]).unwrap();
    for k in [kind::REQUEST, kind::CANCEL] {
        let e = ControlClientMsg::from_frame(k, payload.clone()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "kind {k}");
    }
    let e = ControlClientMsg::from_frame(kind::REQUEST_BLOB, payload).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);

    let json = serde_json::to_vec(&ControlReply::Ok(ReplyOk::Unit)).unwrap();
    let payload = encode_body(0, &json, &[]).unwrap();
    for k in [kind::RESPONSE, kind::RESPONSE_BLOB] {
        let e = ControlServerMsg::from_frame(k, payload.clone()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "kind {k}");
    }

    let mut buf = Vec::new();
    assert!(
        ControlClientMsg::Request {
            req_id: 0,
            req: ControlRequest::Ping
        }
        .encode(&mut buf)
        .is_err()
    );
}

#[test]
fn unknown_kinds_are_invalid_data() {
    for k in [0u8, 1, 13, 40, 50, 59, 64, 69, 200, 255] {
        let e = ControlClientMsg::from_frame(k, vec![0; 32]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "client kind {k}");
        let e = ControlServerMsg::from_frame(k, vec![0; 32]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "server kind {k}");
    }
}

#[test]
fn control_kinds_sit_in_their_own_range() {
    let ours = [
        kind::HELLO,
        kind::REQUEST,
        kind::REQUEST_BLOB,
        kind::CANCEL,
        kind::HELLO_OK,
        kind::RESPONSE,
        kind::RESPONSE_BLOB,
        kind::EVENT,
    ];
    for k in ours {
        assert!((60..=63).contains(&k), "control kind {k} left its range");
        assert_ne!(k, 13, "13 is retired and must never be reused");
    }
    let taken: Vec<u8> = (1..=24).chain(30..=36).chain([40, 50]).collect();
    for k in ours {
        assert!(
            !taken.contains(&k),
            "control kind {k} collides with protocol"
        );
    }
}

#[test]
fn malformed_json_is_invalid_data() {
    let payload = encode_body(1, b"{not json", &[]).unwrap();
    let e = ControlClientMsg::from_frame(kind::REQUEST, payload).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);

    let e = ControlClientMsg::from_frame(kind::HELLO, b"nope".to_vec()).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn oversize_blob_is_refused_at_encode() {
    let blob = vec![0u8; MAX_FRAME];
    let mut sink = Vec::new();
    let e = ControlClientMsg::RequestBlob {
        req_id: 1,
        req: ControlRequest::WriteFile {
            path: "/tmp/huge".into(),
        },
        blob,
    }
    .encode(&mut sink)
    .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn oversize_declared_length_is_refused_at_decode() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&((MAX_FRAME + 1) as u32).to_le_bytes());
    buf.push(kind::REQUEST);
    let e = ControlClientMsg::read(&mut Cursor::new(&buf)).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn a_frame_exactly_at_max_frame_survives() {
    let json = serde_json::to_vec(&ControlRequest::WriteFile {
        path: "/tmp/big".into(),
    })
    .unwrap();
    let blob = vec![0x5a; MAX_FRAME - CONTROL_HEADER - json.len()];
    let msg = ControlClientMsg::RequestBlob {
        req_id: 1,
        req: ControlRequest::WriteFile {
            path: "/tmp/big".into(),
        },
        blob,
    };
    let mut buf = Vec::new();
    msg.encode(&mut buf).unwrap();
    assert_eq!(
        u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize,
        MAX_FRAME
    );
    assert_eq!(ControlClientMsg::read(&mut Cursor::new(&buf)).unwrap(), msg);
}

#[test]
fn error_kinds_round_trip_through_io() {
    use io::ErrorKind as K;
    let pairs = [
        (K::NotFound, WireErrorKind::NotFound),
        (K::PermissionDenied, WireErrorKind::PermissionDenied),
        (K::AlreadyExists, WireErrorKind::AlreadyExists),
        (K::InvalidInput, WireErrorKind::InvalidInput),
        (K::NotADirectory, WireErrorKind::NotADirectory),
        (K::IsADirectory, WireErrorKind::IsADirectory),
        (K::DirectoryNotEmpty, WireErrorKind::DirectoryNotEmpty),
        (K::FileTooLarge, WireErrorKind::FileTooLarge),
        (K::TimedOut, WireErrorKind::TimedOut),
        (K::ConnectionReset, WireErrorKind::ConnectionReset),
    ];
    for (io_kind, wire_kind) in pairs {
        let wire = WireError::from_io(&io::Error::new(io_kind, "boom"));
        assert_eq!(wire.kind, wire_kind, "io {io_kind:?} -> wire");
        assert_eq!(wire.into_io().kind(), io_kind, "wire {wire_kind:?} -> io");
    }
}

#[test]
fn error_kinds_that_collapse_do_so_predictably() {
    use io::ErrorKind as K;
    for k in [K::BrokenPipe, K::UnexpectedEof, K::ConnectionReset] {
        assert_eq!(
            WireError::from_io(&io::Error::new(k, "x")).kind,
            WireErrorKind::ConnectionReset
        );
    }
    assert_eq!(
        WireError::from_io(&io::Error::new(K::InvalidFilename, "x")).kind,
        WireErrorKind::InvalidInput
    );
    assert_eq!(
        WireError::from_io(&io::Error::new(K::WouldBlock, "x")).kind,
        WireErrorKind::Other
    );
    assert_eq!(WireErrorKind::GitUnavailable.to_io_kind(), K::NotFound);
}

#[test]
fn wire_error_message_survives_into_io() {
    let e = WireError::new(WireErrorKind::NotFound, "/home/me/gone: no such file").into_io();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
    assert!(e.to_string().contains("/home/me/gone"));
}

#[test]
fn a_nonzero_git_exit_is_ok_not_err() {
    let reply = ControlReply::Ok(ReplyOk::Output(Output {
        status: Some(128),
        stdout: Vec::new(),
        stderr: b"not a git repository".to_vec(),
    }));
    let mut buf = Vec::new();
    ControlServerMsg::Response { req_id: 1, reply }
        .encode(&mut buf)
        .unwrap();
    match ControlServerMsg::read(&mut Cursor::new(&buf)).unwrap() {
        ControlServerMsg::Response { reply, .. } => {
            let ok = reply
                .into_result()
                .expect("a non-zero exit is not an error");
            match ok {
                ReplyOk::Output(o) => {
                    assert_eq!(o.status, Some(128));
                    assert!(!o.success());
                    assert_eq!(o.stderr_trimmed(), "not a git repository");
                }
                other => panic!("expected Output, got {other:?}"),
            }
        }
        other => panic!("expected Response, got {other:?}"),
    }
}

#[test]
fn output_bytes_ride_as_base64_not_a_number_array() {
    let out = Output {
        status: Some(0),
        stdout: vec![0u8; 3000],
        stderr: Vec::new(),
    };
    let json = serde_json::to_string(&out).unwrap();
    assert!(
        json.contains("\"stdout\":\"AAAA"),
        "stdout should be a base64 string, got: {}",
        &json[..60.min(json.len())]
    );
    assert!(!json.contains("[0,0,0"), "must not be a number array");
    assert!(json.len() < 4200, "base64 payload is {} bytes", json.len());
    assert_eq!(serde_json::from_str::<Output>(&json).unwrap(), out);
}

#[test]
fn deadlines_match_the_contract_table() {
    use ControlRequest as R;
    let s = Duration::from_secs;
    let cases: Vec<(R, Duration)> = vec![
        (R::Ping, s(5)),
        (
            R::ReadDir {
                dir: "/".into(),
                root: None,
            },
            s(5),
        ),
        (
            R::ReadDir {
                dir: "/".into(),
                root: Some("/".into()),
            },
            s(5),
        ),
        (R::Stat { path: "/".into() }, s(5)),
        (R::Exists { path: "/".into() }, s(5)),
        (R::Canonicalize { path: "/".into() }, s(5)),
        (R::RepoRoot { path: "/".into() }, s(5)),
        (R::WatchOpen { dirs: vec![] }, s(5)),
        (
            R::WatchSet {
                id: 1,
                dirs: vec![],
            },
            s(5),
        ),
        (R::WatchClose { id: 1 }, s(5)),
        (
            R::ReadFile {
                path: "/".into(),
                max_bytes: 1,
            },
            s(30),
        ),
        (R::WriteFile { path: "/".into() }, s(30)),
        (R::CreateFileNew { path: "/".into() }, s(10)),
        (
            R::CreateDir {
                path: "/".into(),
                recursive: false,
            },
            s(10),
        ),
        (
            R::Rename {
                from: "/".into(),
                to: "/".into(),
            },
            s(10),
        ),
        (
            R::Remove {
                path: "/".into(),
                recursive: false,
            },
            s(10),
        ),
        (
            R::Git {
                cwd: "/".into(),
                args: vec![],
            },
            s(20),
        ),
        (
            R::Search {
                roots: vec![],
                query: String::new(),
                limit: 1,
                max_dirs: 1,
                show_hidden: true,
            },
            s(20),
        ),
        (
            R::SearchContent {
                roots: vec![],
                query: ContentQuery::default(),
                limits: ContentLimits::default(),
            },
            s(30),
        ),
        (R::GuiOpen { path: None }, s(5)),
        (
            R::TabSetTags {
                tab: TabId::new(),
                tags: vec![],
            },
            s(10),
        ),
        (
            R::SetTags {
                tags: crate::tag::SidebarTags::default(),
            },
            s(10),
        ),
        (R::AgentStates, s(5)),
        (R::PaneProcs { pane_id: 7 }, s(5)),
        (R::Routes, s(5)),
        (R::Status, s(5)),
    ];
    assert_eq!(
        cases.len(),
        every_request().len(),
        "every request variant needs a deadline case"
    );
    for (req, want) in cases {
        assert_eq!(req.deadline(), want, "deadline for {req:?}");
    }
}

#[test]
fn blob_shape_is_known_per_request() {
    for req in every_request() {
        let takes = matches!(req, ControlRequest::WriteFile { .. });
        let returns = matches!(req, ControlRequest::ReadFile { .. });
        assert_eq!(req.takes_blob(), takes, "takes_blob for {req:?}");
        assert_eq!(req.returns_blob(), returns, "returns_blob for {req:?}");
    }
}

#[test]
fn hello_ok_round_trips_with_features() {
    let mut buf = Vec::new();
    ControlServerMsg::HelloOk(hello_ok())
        .encode(&mut buf)
        .unwrap();
    match ControlServerMsg::read(&mut Cursor::new(&buf)).unwrap() {
        ControlServerMsg::HelloOk(ok) => {
            assert_eq!(ok.separator, '/');
            assert_eq!(ok.home, "/home/me");
            assert!(ok.has_feature(feature::CONTROL));
            assert!(ok.has_feature(feature::HOST_RPC));
            assert!(!ok.has_feature("workspace-store"));
        }
        other => panic!("expected HelloOk, got {other:?}"),
    }
}

#[test]
fn hello_ok_without_features_still_decodes() {
    let json = br#"{"control_version":1,"protocol_version":3,"build":"old",
                    "separator":"/","home":"/root"}"#;
    let ok: ControlHelloOk = serde_json::from_slice(json).unwrap();
    assert!(ok.features.is_empty());
    assert!(!ok.has_feature(feature::CONTROL));
}

use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

fn client_with_peer<F>(events: EventSink, serve: F) -> ControlClient
where
    F: FnOnce(TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Hello(h) => assert_eq!(h.control_version, CONTROL_VERSION),
            other => panic!("expected Hello, got {other:?}"),
        }
        ControlServerMsg::HelloOk(hello_ok())
            .encode(&mut sock)
            .unwrap();
        sock.flush().unwrap();
        serve(sock);
    });
    let sock = TcpStream::connect(addr).unwrap();
    ControlClient::over_tcp(sock, &hello(), events).unwrap()
}

fn no_events() -> EventSink {
    Box::new(|_| {})
}

fn reply_to<W: Write>(w: &mut W, req_id: u64, reply: ReplyOk) {
    ControlServerMsg::Response {
        req_id,
        reply: ControlReply::Ok(reply),
    }
    .encode(w)
    .unwrap();
    w.flush().unwrap();
}

#[test]
fn a_request_and_its_reply_cross_a_real_duplex_stream() {
    let client = client_with_peer(no_events(), |mut sock| {
        for _ in 0..3 {
            match ControlClientMsg::read(&mut sock) {
                Ok(ControlClientMsg::Request { req_id, req }) => {
                    let reply = match req {
                        ControlRequest::Ping => ReplyOk::Pong,
                        ControlRequest::Stat { .. } => ReplyOk::Meta(meta()),
                        ControlRequest::Exists { .. } => ReplyOk::Bool(true),
                        other => panic!("unexpected request {other:?}"),
                    };
                    reply_to(&mut sock, req_id, reply);
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
    });

    assert_eq!(client.hello().separator, '/');
    assert_eq!(client.hello().home, "/home/me");
    assert!(client.is_connected());

    assert_eq!(client.call(ControlRequest::Ping).unwrap(), ReplyOk::Pong);
    assert_eq!(
        client
            .call(ControlRequest::Stat {
                path: "/etc/hosts".into()
            })
            .unwrap(),
        ReplyOk::Meta(meta())
    );
    assert_eq!(
        client
            .call(ControlRequest::Exists {
                path: "/etc".into()
            })
            .unwrap(),
        ReplyOk::Bool(true)
    );
}

#[test]
fn a_slow_request_does_not_block_a_fast_one_behind_it() {
    let client = Arc::new(client_with_peer(no_events(), |mut sock| {
        let mut parked_git = None;
        loop {
            match ControlClientMsg::read(&mut sock).unwrap() {
                ControlClientMsg::Request { req_id, req } => match req {
                    ControlRequest::Git { .. } => parked_git = Some(req_id),
                    ControlRequest::ReadDir { .. } => {
                        reply_to(&mut sock, req_id, ReplyOk::Entries(vec![]));
                        thread::sleep(Duration::from_millis(250));
                        reply_to(
                            &mut sock,
                            parked_git.expect("git arrived first"),
                            ReplyOk::Output(Output {
                                status: Some(0),
                                stdout: b"clean".to_vec(),
                                stderr: Vec::new(),
                            }),
                        );
                        return;
                    }
                    other => panic!("unexpected request {other:?}"),
                },
                other => panic!("unexpected message {other:?}"),
            }
        }
    }));

    let (done_tx, done_rx) = mpsc::channel::<&'static str>();

    let slow_client = Arc::clone(&client);
    let slow_done = done_tx.clone();
    let slow = thread::spawn(move || {
        let out = slow_client
            .call(ControlRequest::Git {
                cwd: "/repo".into(),
                args: vec!["status".into()],
            })
            .unwrap();
        slow_done.send("git").unwrap();
        out
    });

    thread::sleep(Duration::from_millis(100));
    let entries = client
        .call(ControlRequest::ReadDir {
            dir: "/repo".into(),
            root: None,
        })
        .unwrap();
    done_tx.send("read_dir").unwrap();

    assert_eq!(entries, ReplyOk::Entries(vec![]));
    assert_eq!(
        done_rx.recv().unwrap(),
        "read_dir",
        "the fast request must finish first even though it was issued second"
    );
    assert_eq!(done_rx.recv().unwrap(), "git");
    assert!(matches!(
        slow.join().unwrap(),
        ReplyOk::Output(o) if o.stdout_trimmed() == "clean"
    ));
}

/// `post` returns while the peer is still sitting on its answer, and that
/// answer, when it does arrive, is not handed to the next caller.
#[test]
fn a_post_does_not_wait_for_its_answer() {
    let (seen_tx, seen_rx) = mpsc::channel::<u64>();
    let (answer_tx, answer_rx) = mpsc::channel::<()>();
    let client = client_with_peer(no_events(), move |mut sock| {
        let closed = match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request {
                req_id,
                req: ControlRequest::WatchClose { id: 7 },
            } => req_id,
            other => panic!("unexpected message {other:?}"),
        };
        seen_tx.send(closed).unwrap();
        answer_rx.recv().unwrap();
        reply_to(&mut sock, closed, ReplyOk::Unit);
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request { req_id, .. } => {
                reply_to(&mut sock, req_id, ReplyOk::Path("/next".into()))
            }
            other => panic!("unexpected message {other:?}"),
        }
    });

    client.post(ControlRequest::WatchClose { id: 7 }).unwrap();
    seen_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the post reached the peer");
    answer_tx.send(()).unwrap();

    let next = client
        .call(ControlRequest::Canonicalize {
            path: "/next".into(),
        })
        .unwrap();
    assert_eq!(next, ReplyOk::Path("/next".into()));
}

#[test]
fn concurrent_callers_each_get_their_own_reply() {
    const N: u64 = 16;
    let client = Arc::new(client_with_peer(no_events(), |mut sock| {
        let mut seen = Vec::new();
        while (seen.len() as u64) < N {
            match ControlClientMsg::read(&mut sock).unwrap() {
                ControlClientMsg::Request { req_id, req } => seen.push((req_id, req)),
                other => panic!("unexpected message {other:?}"),
            }
        }
        for (req_id, req) in seen.into_iter().rev() {
            let path = match req {
                ControlRequest::Canonicalize { path } => path,
                other => panic!("unexpected request {other:?}"),
            };
            reply_to(&mut sock, req_id, ReplyOk::Path(path));
        }
    }));

    let handles: Vec<_> = (0..N)
        .map(|i| {
            let c = Arc::clone(&client);
            thread::spawn(move || {
                let want = format!("/p/{i}");
                let got = c
                    .call(ControlRequest::Canonicalize { path: want.clone() })
                    .unwrap();
                assert_eq!(
                    got,
                    ReplyOk::Path(want),
                    "caller {i} received another caller's reply"
                );
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn a_timeout_cancels_that_request_and_leaves_the_connection_usable() {
    let (saw_cancel_tx, saw_cancel_rx) = mpsc::channel();
    let client = client_with_peer(no_events(), move |mut sock| {
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request { .. } => {}
            other => panic!("unexpected message {other:?}"),
        }
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Cancel { req_id } => saw_cancel_tx.send(req_id).unwrap(),
            other => panic!("expected Cancel, got {other:?}"),
        }
        reply_to(&mut sock, 1, ReplyOk::Path("/late".into()));
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request { req_id, .. } => {
                reply_to(&mut sock, req_id, ReplyOk::Pong)
            }
            other => panic!("unexpected message {other:?}"),
        }
    });

    let e = client
        .call_with_deadline(
            ControlRequest::Canonicalize {
                path: "/slow".into(),
            },
            &[],
            Duration::from_millis(150),
        )
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    assert_eq!(saw_cancel_rx.recv().unwrap(), 1, "cancel names the request");

    assert!(client.is_connected(), "a timeout must not kill the link");
    assert_eq!(
        client.call(ControlRequest::Ping).unwrap(),
        ReplyOk::Pong,
        "the late reply to the abandoned request must not have been \
         mistaken for this one's"
    );
}

#[test]
fn losing_the_link_fails_in_flight_requests_at_once() {
    let client = Arc::new(client_with_peer(no_events(), |mut sock| {
        let _ = ControlClientMsg::read(&mut sock);
        drop(sock);
    }));

    let started = Instant::now();
    let e = client
        .call(ControlRequest::ReadFile {
            path: "/big".into(),
            max_bytes: u64::MAX,
        })
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "should fail on the hangup, not wait out the 30s ReadFile deadline"
    );
    assert!(!client.is_connected());

    let e = client.call(ControlRequest::Ping).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
}

#[test]
fn blobs_ride_in_both_directions() {
    let content: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
    let echoed = content.clone();
    let client = client_with_peer(no_events(), move |mut sock| {
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::RequestBlob { req_id, req, blob } => {
                assert_eq!(
                    req,
                    ControlRequest::WriteFile {
                        path: "/tmp/f".into()
                    }
                );
                assert_eq!(blob, echoed);
                reply_to(&mut sock, req_id, ReplyOk::Meta(meta()));
            }
            other => panic!("expected RequestBlob, got {other:?}"),
        }
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request { req_id, .. } => {
                ControlServerMsg::ResponseBlob {
                    req_id,
                    reply: ControlReply::Ok(ReplyOk::FileMeta { meta: meta() }),
                    blob: echoed.clone(),
                }
                .encode(&mut sock)
                .unwrap();
                sock.flush().unwrap();
            }
            other => panic!("expected Request, got {other:?}"),
        }
    });

    let wrote = client
        .call_with_blob(
            ControlRequest::WriteFile {
                path: "/tmp/f".into(),
            },
            &content,
        )
        .unwrap();
    assert_eq!(wrote, ReplyOk::Meta(meta()));

    let read = client
        .call_full(
            ControlRequest::ReadFile {
                path: "/tmp/f".into(),
                max_bytes: u64::MAX,
            },
            &[],
        )
        .unwrap();
    assert_eq!(read.reply, ReplyOk::FileMeta { meta: meta() });
    assert_eq!(read.blob, content, "the blob is the file's content");
}

#[test]
fn an_error_reply_becomes_a_matching_io_error() {
    let client = client_with_peer(no_events(), |mut sock| {
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request { req_id, .. } => {
                ControlServerMsg::Response {
                    req_id,
                    reply: ControlReply::Err(WireError::new(
                        WireErrorKind::FileTooLarge,
                        "/big is 900 MB, over the 10 MB limit",
                    )),
                }
                .encode(&mut sock)
                .unwrap();
                sock.flush().unwrap();
            }
            other => panic!("unexpected message {other:?}"),
        }
    });

    let e = client
        .call(ControlRequest::ReadFile {
            path: "/big".into(),
            max_bytes: 10 * 1024 * 1024,
        })
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::FileTooLarge);
    assert!(e.to_string().contains("900 MB"));
}

#[test]
fn events_reach_the_sink_interleaved_with_replies() {
    let (tx, rx) = mpsc::channel();
    let sink: EventSink = Box::new(move |e| tx.send(e).unwrap());
    let client = client_with_peer(sink, |mut sock| {
        ControlServerMsg::Event(ControlEvent::Watch {
            id: 1,
            paths: vec!["/a".into()],
        })
        .encode(&mut sock)
        .unwrap();
        match ControlClientMsg::read(&mut sock).unwrap() {
            ControlClientMsg::Request { req_id, .. } => {
                ControlServerMsg::Event(ControlEvent::WatchOverflow { id: 1 })
                    .encode(&mut sock)
                    .unwrap();
                reply_to(&mut sock, req_id, ReplyOk::Pong);
            }
            other => panic!("unexpected message {other:?}"),
        }
        ControlServerMsg::Event(ControlEvent::GuiOpen {
            path: Some("/tmp".into()),
        })
        .encode(&mut sock)
        .unwrap();
        sock.flush().unwrap();
    });

    assert_eq!(client.call(ControlRequest::Ping).unwrap(), ReplyOk::Pong);
    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    assert_eq!(
        got,
        vec![
            ControlEvent::Watch {
                id: 1,
                paths: vec!["/a".into()]
            },
            ControlEvent::WatchOverflow { id: 1 },
            ControlEvent::GuiOpen {
                path: Some("/tmp".into())
            },
        ]
    );
}

#[test]
fn request_ids_start_at_one_and_increase() {
    let (tx, rx) = mpsc::channel();
    let client = client_with_peer(no_events(), move |mut sock| {
        for _ in 0..4 {
            match ControlClientMsg::read(&mut sock).unwrap() {
                ControlClientMsg::Request { req_id, .. } => {
                    tx.send(req_id).unwrap();
                    reply_to(&mut sock, req_id, ReplyOk::Pong);
                }
                other => panic!("unexpected message {other:?}"),
            }
        }
    });
    for _ in 0..4 {
        client.call(ControlRequest::Ping).unwrap();
    }
    let ids: Vec<u64> = (0..4).map(|_| rx.recv().unwrap()).collect();
    assert_eq!(ids, vec![1, 2, 3, 4]);
}

#[test]
fn a_control_version_mismatch_names_both_versions() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let _ = ControlClientMsg::read(&mut sock);
        let mut ok = hello_ok();
        ok.control_version = CONTROL_VERSION + 7;
        ok.build = "from-the-future".into();
        ControlServerMsg::HelloOk(ok).encode(&mut sock).unwrap();
        sock.flush().unwrap();
    });

    let sock = TcpStream::connect(addr).unwrap();
    let e = ControlClient::over_tcp(sock, &hello(), no_events()).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    let msg = e.to_string();
    assert!(msg.contains("from-the-future"), "names the peer: {msg}");
    assert!(
        msg.contains(&format!("v{}", CONTROL_VERSION + 7)),
        "names their version: {msg}"
    );
    assert!(
        msg.contains(&format!("v{CONTROL_VERSION}")),
        "names ours: {msg}"
    );
    assert!(
        is_dialect_refusal(&msg),
        "and the GUI can tell this apart from an unreachable host, which is what \
         decides whether it may offer to replace the far end's binary: {msg}"
    );
}

#[test]
fn other_connect_failures_are_not_dialect_refusals() {
    for other in [
        "Connection refused (os error 61)",
        "ssh: handshake failed: no matching key exchange method",
        "the remote anterm-server did not start: exit status 127",
        "could not resolve the remote home directory: Permission denied",
        "control peer answered the handshake with Bye instead of HELLO_OK",
    ] {
        assert!(!is_dialect_refusal(other), "{other}");
    }
    assert!(is_dialect_refusal(&dialect_refusal("26.7.6", 2, 3)));
}

#[test]
fn a_dialect_refusal_parses_back_into_its_parts() {
    let wrapped = format!("java answered: {}", dialect_refusal("26.7.7-nightly", 4, 5));
    assert_eq!(
        parse_dialect_refusal(&wrapped),
        Some(DialectRefusal {
            peer_build: "26.7.7-nightly".into(),
            peer: 4,
            ours: 5,
        }),
        "the GUI reads the build and both versions back out to say which side is behind"
    );
    for other in [
        "Connection refused (os error 61)",
        "control peer answered the handshake with Bye instead of HELLO_OK",
    ] {
        assert_eq!(parse_dialect_refusal(other), None, "{other}");
    }
}

#[test]
fn a_handshake_answered_with_the_wrong_frame_is_invalid_data() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let _ = ControlClientMsg::read(&mut sock);
        ControlServerMsg::Event(ControlEvent::WatchOverflow { id: 1 })
            .encode(&mut sock)
            .unwrap();
        sock.flush().unwrap();
    });
    let sock = TcpStream::connect(addr).unwrap();
    let e = ControlClient::over_tcp(sock, &hello(), no_events()).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn closing_an_idle_client_returns_promptly() {
    let client = client_with_peer(no_events(), |mut sock| {
        let _ = ControlClientMsg::read(&mut sock);
        thread::sleep(Duration::from_secs(30));
    });
    assert_eq!(client.call(ControlRequest::Ping).unwrap_err().kind(), {
        io::ErrorKind::TimedOut
    });

    let started = Instant::now();
    client.close();
    assert!(
        started.elapsed() < CLOSE_GRACE * 2,
        "close() took {:?}; it must not wait on a peer that will never speak",
        started.elapsed()
    );
}

#[test]
fn dropping_an_idle_client_returns_promptly() {
    let client = client_with_peer(no_events(), |mut sock| {
        let _ = ControlClientMsg::read(&mut sock);
        thread::sleep(Duration::from_secs(30));
    });
    let started = Instant::now();
    drop(client);
    assert!(
        started.elapsed() < CLOSE_GRACE * 2,
        "drop took {:?}",
        started.elapsed()
    );
}

#[test]
fn close_reaps_the_reader_when_the_link_can_be_shut_down() {
    let client = client_with_peer(no_events(), |mut sock| {
        let _ = ControlClientMsg::read(&mut sock);
        thread::sleep(Duration::from_secs(30));
    });
    let started = Instant::now();
    client.close();
    assert!(
        started.elapsed() < CLOSE_GRACE,
        "reader should have been reaped, not waited out: {:?}",
        started.elapsed()
    );
    assert!(!client.is_connected());
}

#[test]
fn a_malformed_frame_tears_the_connection_down() {
    let client = client_with_peer(no_events(), |mut sock| {
        let _ = ControlClientMsg::read(&mut sock);
        write_frame(&mut sock, 64, &[0u8; 16]).unwrap();
        sock.flush().unwrap();
        thread::sleep(Duration::from_secs(2));
    });
    let e = client.call(ControlRequest::Ping).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
    assert!(!client.is_connected());
}
