//! UniFFI bridge crate for Anterm Mobile (iOS SwiftUI/Metal & future Android).
//!
//! Exposes a clean, strongly typed cross-language API over:
//! - `anterm-gateway` (`iroh` QUIC E2EE pairing & multiplexed streams)
//! - `anterm-client-core` (headless VT grid, Metal binary frames, zero-PC-impact reflow,
//!   predictive echo, and shared coding-agent composer plans)
//! - `anterm-protocol` (wire framing, transcript schema, RBAC roles)

uniffi::setup_scaffolding!();

pub mod session;
pub mod types;

use anterm_client_core::composer::{compose_message, mention, submit_plan};
use anterm_protocol::gateway::PairingBundle;

pub use session::{
    MobileClient, MobileHostSession, MobilePaneSession, MobilePreviewProxy,
    MobileTranscriptSession,
};
pub use types::{
    FfiAgentStatus, FfiFileEntry, FfiGitChangedFile, FfiGitDiffSnapshot, FfiMobileRole,
    FfiPairedHost, FfiPairingInfo, FfiPaneSummary, FfiQuickApproval, FfiReflowLine,
    FfiRemoteRoute, FfiTabSummary, FfiTranscriptBatch, FfiTranscriptEntry,
    FfiTranscriptEntryKind, FfiWorkspaceEvent, FfiWorkspaceEventKind, FfiWorkspaceSnapshot,
    MobileError,
};

/// Parse an `anterm://pair/v1?...` QR code URI without performing any network I/O.
#[uniffi::export]
pub fn parse_pairing_uri(uri: String) -> Result<FfiPairingInfo, MobileError> {
    let bundle = PairingBundle::from_uri(&uri).map_err(|e| MobileError::msg(e.to_string()))?;
    Ok(FfiPairingInfo::from(&bundle))
}

/// Format a prompt + optional file attachments for a coding agent and return the
/// exact PTY byte steps to transmit.
#[uniffi::export]
pub fn build_agent_submit_bytes(
    agent: String,
    text: String,
    attached_paths: Vec<String>,
    bracketed_paste: bool,
) -> Vec<Vec<u8>> {
    let Some(parsed) = types::parse_cli_agent(&agent) else {
        let mut out = text.into_bytes();
        out.push(b'\r');
        return vec![out];
    };
    let composed = compose_message(parsed, &text, &attached_paths, None);
    submit_plan(parsed, &composed, bracketed_paste)
        .into_iter()
        .map(|s| s.bytes)
        .collect()
}

/// Format a single `@path` mention using the exact desktop spelling for `agent`.
#[uniffi::export]
pub fn format_agent_mention(agent: String, path: String) -> String {
    mention(types::parse_cli_agent(&agent), &path, None)
}

/// Produce byte sequences for quick approval buttons when an agent is waiting.
#[uniffi::export]
pub fn build_quick_approval_bytes(agent: Option<String>, action: FfiQuickApproval) -> Vec<u8> {
    let parsed = agent.as_deref().and_then(types::parse_cli_agent);
    types::approval_bytes(parsed, action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use anterm_gateway::server::{GatewayBackend, GatewayServer, PaneTranscriptSource};
    use anterm_gateway::{GatewayStore, MobileRole};
    use anterm_protocol::cli_agent::CLIAgent;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    struct TestBackend {
        transcript_path: PathBuf,
        ports: HashSet<u16>,
    }

    impl GatewayBackend for TestBackend {
        fn host_name(&self) -> String {
            "ffi-test-mac".into()
        }

        fn open_control_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "mock"))
        }

        fn open_pane_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "mock"))
        }

        fn pane_transcript(&self, _pane_id: u64) -> Option<PaneTranscriptSource> {
            Some(PaneTranscriptSource {
                agent: Some(CLIAgent::Claude),
                session_id: Some("sess-ffi".into()),
                path: Some(self.transcript_path.clone()),
            })
        }

        fn allowed_preview_ports(&self, _pane_id: Option<u64>) -> HashSet<u16> {
            self.ports.clone()
        }
    }

    #[test]
    fn ffi_client_pairs_streams_transcript_and_proxies_localhost_http() {
        let host_dir = tempfile::tempdir().unwrap();
        let mobile_dir = tempfile::tempdir().unwrap();
        let transcript_file = host_dir.path().join("claude.jsonl");
        std::fs::write(
            &transcript_file,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Hello from Claude via UniFFI\"}]}}\n",
        )
        .unwrap();

        let host_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let (uri, http_port, server_holder) = host_rt.block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let http_port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nANTERM-OK")
                        .await;
                }
            });

            let store = GatewayStore::open(host_dir.path().to_path_buf());
            store
                .update_config(|c| {
                    c.enabled = true;
                    c.lan_only = true;
                })
                .unwrap();
            let mut ports = HashSet::new();
            ports.insert(http_port);
            let backend = Arc::new(TestBackend {
                transcript_path: transcript_file.clone(),
                ports,
            });
            let server = GatewayServer::start(store, backend).await.unwrap();
            let bundle = server
                .issue_pairing_bundle(MobileRole::Interactive, Some(300))
                .unwrap();
            (bundle.to_uri(), http_port, Mutex::new(Some(server)))
        });

        // 1. Parse URI without network
        let parsed = parse_pairing_uri(uri.clone()).unwrap();
        assert_eq!(parsed.host_name, "ffi-test-mac");
        assert!(!parsed.is_expired);

        // 2. Pair via UniFFI MobileClient
        let client =
            MobileClient::new(mobile_dir.path().display().to_string(), "iPhone 17 Pro".into())
                .unwrap();
        let paired = client.pair_with_uri(uri).unwrap();
        assert_eq!(paired.host_name, "ffi-test-mac");
        assert_eq!(paired.role, FfiMobileRole::Interactive);
        assert_eq!(client.list_paired_hosts().len(), 1);

        // Verify local device rename & host rename persist cleanly
        let renamed_dev = client.set_device_name("Futsing iPhone 14 Plus".into()).unwrap();
        assert_eq!(renamed_dev, "Futsing iPhone 14 Plus");
        assert_eq!(client.device_name(), "Futsing iPhone 14 Plus");
        let renamed_host = client
            .rename_paired_host(paired.host_node_id.clone(), "Office Mac Studio".into())
            .unwrap();
        assert_eq!(renamed_host.host_name, "Office Mac Studio");
        assert_eq!(client.list_paired_hosts()[0].host_name, "Office Mac Studio");

        // 3. Connect and read Layer 1 TranscriptBatch
        let session = client.connect_host(paired.host_node_id.clone()).unwrap();
        assert!(session.is_connected());
        let transcript = session.open_transcript(1, 0).unwrap();
        let batch = transcript.poll_batch(3000).expect("received transcript batch");
        assert_eq!(batch.entries.len(), 1);
        assert!(batch.entries[0].body.contains("Hello from Claude via UniFFI"));
        transcript.close();

        // 4. Open local WKWebView preview proxy -> tunnels to host localhost HTTP server!
        let proxy = session.start_preview_proxy(http_port, Some(1)).unwrap();
        let mut tcp = std::net::TcpStream::connect(("127.0.0.1", proxy.local_port())).unwrap();
        use std::io::{Read as _, Write as _};
        tcp.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        tcp.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("ANTERM-OK"), "got response: {resp}");
        proxy.stop();

        session.disconnect();
        if let Some(server) = server_holder.lock().unwrap().take() {
            host_rt.block_on(server.shutdown());
        }
    }

    #[cfg(unix)]
    #[test]
    fn ffi_control_stream_syncs_workspace_handoff_files_git_diff_and_upload() {
        use anterm_core::daemon::control::PaneAgentState;
        use anterm_core::host::local::LocalHost;
        use anterm_core::host::server::{PaneDirectory, Services, serve_with};
        use anterm_core::machine::{MachineStore, PaneSeed};
        use anterm_protocol::cli_agent::{AgentSessionState, AgentStatus};
        use anterm_protocol::pane::{PaneInfo, PaneProcs, PortEntry};

        let host_dir = tempfile::tempdir().unwrap();
        let mobile_dir = tempfile::tempdir().unwrap();
        let repo_dir = tempfile::tempdir().unwrap();

        // Create a real git repo with one modified file so `fetch_git_diff` runs real git!
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo_dir.path())
                .env("GIT_AUTHOR_NAME", "Anterm")
                .env("GIT_AUTHOR_EMAIL", "dev@anterm.dev")
                .env("GIT_COMMITTER_NAME", "Anterm")
                .env("GIT_COMMITTER_EMAIL", "dev@anterm.dev")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init"]);
        std::fs::write(repo_dir.path().join("hello.rs"), "fn main() {}\n").unwrap();
        run(&["add", "hello.rs"]);
        run(&["commit", "-m", "init"]);
        std::fs::write(
            repo_dir.path().join("hello.rs"),
            "fn main() {\n    println!(\"mobile\");\n}\n",
        )
        .unwrap();

        // Seed a real MachineStore with 2 tabs so we can test workspace fetch & Handoff!
        let machine_store = Arc::new(MachineStore::open(host_dir.path().join("machine.json")));
        let tab1 = machine_store
            .tab_create(
                None,
                PaneSeed {
                    pane: 101,
                    cwd: Some(repo_dir.path().display().to_string()),
                    ssh_spec: None,
                    agent: None,
                    shell: None,
                },
                None,
                None,
            )
            .unwrap();
        let tab2 = machine_store
            .tab_create(
                None,
                PaneSeed {
                    pane: 202,
                    cwd: Some(repo_dir.path().display().to_string()),
                    ssh_spec: None,
                    agent: None,
                    shell: None,
                },
                None,
                None,
            )
            .unwrap();
        machine_store.set_active_tab(tab1.id, None).unwrap();

        struct MockPanes {
            cwd: PathBuf,
        }
        impl PaneDirectory for MockPanes {
            fn pane_count(&self) -> u64 {
                2
            }
            fn panes(&self) -> Vec<PaneInfo> {
                Vec::new()
            }
            fn agent_states(&self) -> Vec<PaneAgentState> {
                vec![PaneAgentState {
                    pane_id: 101,
                    agent: Some(CLIAgent::Claude),
                    state: AgentSessionState {
                        status: AgentStatus::Waiting,
                        message: Some("Allow running cargo test?".into()),
                        cwd: Some(self.cwd.clone()),
                        ..AgentSessionState::default()
                    },
                }]
            }
            fn pane_procs(&self, pane_id: u64) -> PaneProcs {
                if pane_id == 202 {
                    PaneProcs {
                        ports: vec![PortEntry {
                            port: 5173,
                            pid: 999,
                            name: "vite".into(),
                            addr: "127.0.0.1".into(),
                        }],
                        ..PaneProcs::default()
                    }
                } else {
                    PaneProcs::default()
                }
            }
            fn hibernate_pane(&self, _pane_id: u64) {}
            fn close_pane(&self, _pane_id: u64) {}
        }

        struct LiveControlBackend {
            services: Services,
        }
        impl GatewayBackend for LiveControlBackend {
            fn host_name(&self) -> String {
                "macbook-pro".into()
            }
            fn open_control_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
                let (client_end, server_end) = std::os::unix::net::UnixStream::pair()?;
                let services = self.services.clone();
                std::thread::spawn(move || {
                    let _ = serve_with(server_end, LocalHost::shared(), services);
                });
                Ok(client_end)
            }
            fn open_pane_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
                Err(io::Error::new(io::ErrorKind::Unsupported, "mock"))
            }
            fn pane_transcript(&self, _pane_id: u64) -> Option<PaneTranscriptSource> {
                None
            }
            fn allowed_preview_ports(&self, _pane_id: Option<u64>) -> HashSet<u16> {
                HashSet::new()
            }
        }

        let services = Services {
            machine: Some(Arc::clone(&machine_store)),
            attachments: Arc::new(Default::default()),
            panes: Some(Arc::new(MockPanes {
                cwd: repo_dir.path().to_path_buf(),
            })),
        };

        let host_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let (uri, server_holder) = host_rt.block_on(async {
            let store = GatewayStore::open(host_dir.path().to_path_buf());
            store
                .update_config(|c| {
                    c.enabled = true;
                    c.lan_only = true;
                })
                .unwrap();
            let backend = Arc::new(LiveControlBackend { services });
            let server = GatewayServer::start(store, backend).await.unwrap();
            let bundle = server
                .issue_pairing_bundle(MobileRole::Full, Some(300))
                .unwrap();
            (bundle.to_uri(), Mutex::new(Some(server)))
        });

        let client = MobileClient::new(
            mobile_dir.path().display().to_string(),
            "iPhone 17 Pro".into(),
        )
        .unwrap();
        let paired = client.pair_with_uri(uri).unwrap();
        let session = client.connect_host(paired.host_node_id).unwrap();

        // 1. Live workspace snapshot (tabs, active tab, Waiting agent, listening ports)
        let ws = session.fetch_workspace().unwrap();
        assert_eq!(ws.tabs.len(), 2);
        assert_eq!(ws.desktop_active_tab_id, Some(tab1.id.to_string()));
        assert_eq!(ws.waiting_panes.len(), 1);
        assert_eq!(ws.waiting_panes[0].pane_id, 101);
        assert_eq!(ws.waiting_panes[0].agent_status, FfiAgentStatus::Waiting);
        assert_eq!(
            ws.waiting_panes[0].agent_detail.as_deref(),
            Some("Allow running cargo test?")
        );
        let tab2_summary = ws.tabs.iter().find(|t| t.tab_id == tab2.id.to_string()).unwrap();
        assert_eq!(tab2_summary.panes[0].listening_ports, vec![5173]);

        // 2. Explicit Handoff switches PC active tab to tab2
        session.handoff_focus_desktop_tab(tab2.id.to_string()).unwrap();
        let ws2 = session.fetch_workspace().unwrap();
        assert_eq!(ws2.desktop_active_tab_id, Some(tab2.id.to_string()));

        // 3. File listing, search, and read
        let repo_str = repo_dir.path().display().to_string();
        let entries = session.list_directory(repo_str.clone()).unwrap();
        assert!(entries.iter().any(|e| e.name == "hello.rs" && !e.is_dir));
        let hits = session.search_files(repo_str.clone(), "hello".into(), 10).unwrap();
        assert!(hits.iter().any(|h| h.name == "hello.rs"));
        let contents = session
            .read_file_text(repo_dir.path().join("hello.rs").display().to_string(), 65536)
            .unwrap();
        assert!(contents.contains("println!(\"mobile\")"));

        // 4. Live Git diff
        let diff = session.fetch_git_diff(repo_str, None).unwrap();
        assert!(diff.files.iter().any(|f| f.path == "hello.rs"));
        assert!(diff.unified_diff.contains("println!(\"mobile\")"));

        // 5. Attachment upload (UploadTempFile)
        let uploaded_path = session
            .upload_attachment("screenshot.png".into(), "image/png".into(), b"PNGDATA".to_vec())
            .unwrap();
        assert_eq!(std::fs::read(&uploaded_path).unwrap(), b"PNGDATA");
        let _ = std::fs::remove_file(&uploaded_path);

        session.disconnect();
        if let Some(server) = server_holder.lock().unwrap().take() {
            host_rt.block_on(server.shutdown());
        }
    }
}
