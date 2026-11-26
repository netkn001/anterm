use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anterm_protocol::cli_agent::CLIAgent;
use anterm_protocol::gateway::{
    MobileRole, PairRequest, PairingBundle, PortTunnelOpen, TranscriptStreamOpen,
};
use anterm_protocol::transcript::TranscriptEntryKind;
use iroh::SecretKey;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use crate::client::GatewayClient;
use crate::server::{GatewayBackend, GatewayServer, PaneTranscriptSource};
use crate::store::{GatewayStore, render_qr_ascii};

struct MockBackend {
    transcript_path: Mutex<Option<PathBuf>>,
    preview_ports: Mutex<HashSet<u16>>,
}

impl GatewayBackend for MockBackend {
    fn host_name(&self) -> String {
        "Test-MacBook-Pro".into()
    }

    fn open_control_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not used in mock backend",
        ))
    }

    fn open_pane_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not used in mock backend",
        ))
    }

    fn pane_transcript(&self, _pane_id: u64) -> Option<PaneTranscriptSource> {
        let path = self
            .transcript_path
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Some(PaneTranscriptSource {
            agent: Some(CLIAgent::Claude),
            session_id: Some("sess-e2e".into()),
            path,
        })
    }

    fn allowed_preview_ports(&self, _pane_id: Option<u64>) -> HashSet<u16> {
        self.preview_ports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[tokio::test]
async fn end_to_end_iroh_pairing_transcript_port_tunnel_and_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let store = GatewayStore::open(dir.path().to_path_buf());
    store
        .update_config(|c| {
            c.enabled = true;
            c.lan_only = true;
        })
        .unwrap();

    // Prepare a real Claude JSONL transcript file on disk.
    let transcript_file = dir.path().join("sess-e2e.jsonl");
    std::fs::write(
        &transcript_file,
        concat!(
            r#"{"type":"user","message":{"content":"Build mobile gateway"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[{"type":"text","text":"Gateway is live."}]}}"#,
            "\n"
        ),
    )
    .unwrap();

    // Start a mock localhost dev server for PortTunnel Web Preview verification.
    let dev_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dev_port = dev_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = dev_listener.accept().await {
            let mut req = [0u8; 256];
            let _ = sock.read(&mut req).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\n\r\nhello-anterm!\n")
                .await;
        }
    });

    let mut ports = HashSet::new();
    ports.insert(dev_port);
    let backend = Arc::new(MockBackend {
        transcript_path: Mutex::new(Some(transcript_file.clone())),
        preview_ports: Mutex::new(ports),
    });

    let server = GatewayServer::start(store.clone(), backend).await.unwrap();
    let bundle = server
        .issue_pairing_bundle(MobileRole::Interactive, Some(300))
        .unwrap();

    // Verify QR URI round-trip & ASCII QR generation.
    let uri = bundle.to_uri();
    let parsed_bundle = PairingBundle::from_uri(&uri).unwrap();
    assert_eq!(parsed_bundle, bundle);
    let ascii = render_qr_ascii(&uri).unwrap();
    assert!(ascii.contains('█'));

    let phone_key = SecretKey::generate();
    let phone = GatewayClient::bind(phone_key, None, true).await.unwrap();
    let phone_id = phone.node_id();

    // 1. Before pairing, opening a PortTunnel must be rejected.
    let unauth_conn = phone
        .connect(&server.node_id(), &server.lan_addrs(), None)
        .await
        .unwrap();
    let err = unauth_conn
        .open_port_tunnel(PortTunnelOpen {
            target_port: dev_port,
            pane_id: None,
        })
        .await
        .expect_err("unpaired phone must be denied");
    assert!(err.to_string().contains("not authorized"));
    unauth_conn.close();

    // 2. Pair using QR bundle.
    let reply = phone
        .pair(
            &parsed_bundle,
            PairRequest {
                secret_b64: parsed_bundle.secret_b64.clone(),
                device_name: "Futsing's iPhone".into(),
                platform: "ios".into(),
                app_version: "0.1.0".into(),
                requested_role: MobileRole::Interactive,
            },
        )
        .await
        .unwrap();
    assert!(reply.ok);
    assert_eq!(reply.host_name.as_deref(), Some("Test-MacBook-Pro"));
    assert_eq!(reply.granted_role, MobileRole::Interactive);

    // 3. Reconnect as paired device and stream structured agent transcript!
    let conn = phone
        .connect(&server.node_id(), &server.lan_addrs(), None)
        .await
        .unwrap();
    let mut tstream = conn
        .open_transcript(TranscriptStreamOpen {
            pane_id: 1,
            since_seq: 0,
        })
        .await
        .unwrap();
    let batch1 = tstream.next_batch().await.unwrap();
    assert!(batch1.available);
    assert_eq!(batch1.entries.len(), 2);
    assert!(matches!(
        &batch1.entries[1].kind,
        TranscriptEntryKind::AssistantText { markdown, .. } if markdown == "Gateway is live."
    ));

    // Append a live follow-up turn on disk and verify incremental push over QUIC!
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript_file)
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Turn 2 landed!"}}]}}}}"#
        )
        .unwrap();
    }
    let batch2 = tokio::time::timeout(std::time::Duration::from_secs(3), tstream.next_batch())
        .await
        .expect("incremental batch should arrive promptly")
        .unwrap();
    assert_eq!(batch2.entries.len(), 1);
    assert_eq!(batch2.entries[0].seq, 3);
    assert!(matches!(
        &batch2.entries[0].kind,
        TranscriptEntryKind::AssistantText { markdown, .. } if markdown == "Turn 2 landed!"
    ));

    // 4. Open localhost dev-server PortTunnel over the same QUIC connection!
    let (mut t_send, mut t_recv) = conn
        .open_port_tunnel(PortTunnelOpen {
            target_port: dev_port,
            pane_id: None,
        })
        .await
        .unwrap();
    t_send.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
    let mut http_resp = vec![0u8; 256];
    let n = t_recv.read(&mut http_resp).await.unwrap().unwrap();
    let body = String::from_utf8_lossy(&http_resp[..n]);
    assert!(body.contains("hello-anterm!"), "got {body}");

    // 5. Revoke device on host and verify immediate denial on next stream!
    assert!(store.revoke_device(&phone_id).unwrap());
    let revoked_err = conn
        .open_port_tunnel(PortTunnelOpen {
            target_port: dev_port,
            pane_id: None,
        })
        .await
        .expect_err("revoked device must be rejected immediately");
    assert!(revoked_err.to_string().contains("revoked"));

    conn.close();
    phone.close().await;
    server.shutdown().await;
}
