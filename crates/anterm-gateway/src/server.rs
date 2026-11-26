//! Host-side `iroh` QUIC gateway server (`GatewayServer`).
//!
//! Accepts mutually-authenticated mobile connections over LAN / P2P / `iroh-relay`,
//! enforces per-device [`MobileRole`] permissions on every stream, and bridges
//! directly into the local `anterm-server` daemon (and its `RemoteRouter`).

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anterm_core::agent_transcript::{TranscriptTailer, discover_transcript_path};
use anterm_core::client::PaneClient;
use anterm_core::daemon::protocol::{ClientMsg, DaemonMsg, PROTOCOL_VERSION, WinSize};
use anterm_protocol::cli_agent::CLIAgent;
use anterm_protocol::gateway::{
    ControlStreamOpen, GATEWAY_ALPN, MobileRole, PairReply, PairRequest, PairingBundle,
    PaneStreamOpen, PortTunnelOpen, PortTunnelReply, StreamKind, TranscriptStreamOpen,
};
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, TransportAddr};
use tokio::net::TcpStream;

use crate::client::resolve_relay_mode;
use crate::store::GatewayStore;
use crate::wire;

/// Information needed to tail a pane's structured transcript file.
#[derive(Clone, Debug)]
pub struct PaneTranscriptSource {
    pub agent: Option<CLIAgent>,
    pub session_id: Option<String>,
    pub path: Option<PathBuf>,
}

/// Injectable host backend so `GatewayServer` works against both a real local
/// `anterm-server` daemon and deterministic in-process integration tests.
pub trait GatewayBackend: Send + Sync + 'static {
    fn host_name(&self) -> String;

    /// Connect to the host's control RPC endpoint and return a bidirectional
    /// byte stream speaking `anterm-core`'s control protocol.
    fn open_control_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream>;

    /// Connect to the host's pane daemon endpoint.
    fn open_pane_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream>;

    /// Resolve a pane's transcript metadata for `StreamKind::Transcript`.
    fn pane_transcript(&self, pane_id: u64) -> Option<PaneTranscriptSource>;

    /// Return the set of localhost TCP ports allowed for `PortTunnel` preview.
    fn allowed_preview_ports(&self, pane_id: Option<u64>) -> HashSet<u16>;
}

/// Production backend bridging into the local `anterm-server` Unix/TCP sockets.
#[derive(Default)]
pub struct LocalDaemonBackend;

impl GatewayBackend for LocalDaemonBackend {
    fn host_name(&self) -> String {
        #[cfg(target_os = "macos")]
        if let Ok(out) = std::process::Command::new("/usr/sbin/scutil")
            .args(["--get", "ComputerName"])
            .output()
            && out.status.success()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
        #[cfg(unix)]
        if let Ok(out) = std::process::Command::new("hostname").output()
            && out.status.success()
        {
            let name = String::from_utf8_lossy(&out.stdout)
                .trim()
                .trim_end_matches(".local")
                .to_string();
            if !name.is_empty() {
                return name;
            }
        }
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "anterm-host".to_string())
    }

    fn open_control_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
        #[cfg(unix)]
        {
            let path = anterm_core::host::server::control_socket_path()?;
            anterm_core::daemon::transport::connect_endpoint_at(&path)
        }
        #[cfg(windows)]
        {
            anterm_core::host::server::connect_control()
        }
    }

    fn open_pane_stream(&self) -> io::Result<anterm_core::daemon::transport::Stream> {
        anterm_core::daemon::transport::connect()
    }

    fn pane_transcript(&self, pane_id: u64) -> Option<PaneTranscriptSource> {
        let hello = anterm_core::daemon::control::ControlHello::host_rpc("", "anterm-gateway");
        let client = anterm_core::client::ControlClient::connect(&hello).ok()?;
        let states = match client
            .request(anterm_core::daemon::control::ControlRequest::AgentStates)
            .ok()?
        {
            anterm_core::daemon::control::ReplyOk::AgentStates(states) => states,
            _ => return None,
        };
        let found = states.into_iter().find(|s| s.pane_id == pane_id)?;
        let path = match (found.agent, found.state.session_id.as_deref()) {
            (Some(agent), Some(sid)) => {
                discover_transcript_path(agent, sid, found.state.transcript_path.as_deref())
            }
            _ => found.state.transcript_path.clone(),
        };
        Some(PaneTranscriptSource {
            agent: found.agent,
            session_id: found.state.session_id,
            path,
        })
    }

    fn allowed_preview_ports(&self, pane_id: Option<u64>) -> HashSet<u16> {
        let client = PaneClient::local();
        let mut ports = HashSet::new();
        if let Some(pid) = pane_id {
            if let Ok(procs) = client.procs(pid) {
                for p in procs.ports {
                    ports.insert(p.port);
                }
            }
            return ports;
        }
        if let Ok(panes) = client.list() {
            for pane in panes {
                if let Ok(procs) = client.procs(pane.pane_id) {
                    for p in procs.ports {
                        ports.insert(p.port);
                    }
                }
            }
        }
        ports
    }
}

/// Running host-side `iroh` QUIC gateway.
pub struct GatewayServer {
    endpoint: Endpoint,
    store: GatewayStore,
    backend: Arc<dyn GatewayBackend>,
    accept_task: tokio::task::JoinHandle<()>,
}

impl GatewayServer {
    /// Start listening on an `iroh` QUIC endpoint backed by `store` and `backend`.
    pub async fn start(
        store: GatewayStore,
        backend: Arc<dyn GatewayBackend>,
    ) -> anyhow::Result<Self> {
        let secret_key = store.load_or_create_secret_key()?;
        let cfg = store.snapshot();
        let relay_mode = resolve_relay_mode(cfg.relay_url.as_deref(), cfg.lan_only)?;

        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(secret_key)
            .alpns(vec![GATEWAY_ALPN.to_vec()])
            .relay_mode(relay_mode)
            .bind()
            .await
            .map_err(|e| anyhow::anyhow!("gateway bind failed: {e}"))?;

        let ep = endpoint.clone();
        let st = store.clone();
        let be = Arc::clone(&backend);
        let accept_task = tokio::spawn(async move {
            accept_loop(ep, st, be).await;
        });

        Ok(Self {
            endpoint,
            store,
            backend,
            accept_task,
        })
    }

    pub fn node_id(&self) -> String {
        self.endpoint.id().to_string()
    }

    pub fn lan_addrs(&self) -> Vec<String> {
        let reported: Vec<String> = self
            .endpoint
            .addr()
            .addrs
            .iter()
            .filter_map(|a| match a {
                TransportAddr::Ip(sock) => Some(sock.to_string()),
                _ => None,
            })
            .collect();
        crate::runtime::discover_reachable_lan_addrs(&reported)
    }

    /// Generate a single-use QR pairing bundle for this running gateway.
    pub fn issue_pairing_bundle(
        &self,
        role: MobileRole,
        ttl_secs: Option<u64>,
    ) -> anyhow::Result<PairingBundle> {
        self.store.issue_pairing_bundle(
            self.node_id(),
            self.lan_addrs(),
            self.backend.host_name(),
            role,
            ttl_secs,
        )
    }

    pub fn store(&self) -> &GatewayStore {
        &self.store
    }

    pub async fn shutdown(self) {
        self.accept_task.abort();
        self.endpoint.close().await;
    }
}

async fn accept_loop(endpoint: Endpoint, store: GatewayStore, backend: Arc<dyn GatewayBackend>) {
    while let Some(incoming) = endpoint.accept().await {
        let store = store.clone();
        let backend = Arc::clone(&backend);
        tokio::spawn(async move {
            let conn = match incoming.await {
                Ok(c) => c,
                Err(e) => {
                    log::debug!("gateway incoming handshake failed: {e}");
                    return;
                }
            };
            handle_connection(conn, store, backend).await;
        });
    }
}

async fn handle_connection(
    conn: Connection,
    store: GatewayStore,
    backend: Arc<dyn GatewayBackend>,
) {
    let peer_id = conn.remote_id().to_string();
    loop {
        let (send, recv) = match conn.accept_bi().await {
            Ok(pair) => pair,
            Err(_) => break,
        };
        let peer_id = peer_id.clone();
        let store = store.clone();
        let backend = Arc::clone(&backend);
        tokio::spawn(async move {
            if let Err(e) = dispatch_stream(&peer_id, send, recv, &store, &backend).await {
                log::debug!("gateway stream from {peer_id} ended: {e}");
            }
        });
    }
}

async fn dispatch_stream(
    peer_id: &str,
    mut send: SendStream,
    mut recv: RecvStream,
    store: &GatewayStore,
    backend: &Arc<dyn GatewayBackend>,
) -> anyhow::Result<()> {
    let kind = wire::read_stream_kind(&mut recv).await?;
    if kind == StreamKind::Pair {
        return handle_pair_stream(peer_id, send, recv, store, backend).await;
    }

    // Every non-pairing stream requires an active paired device authorization.
    // Re-checked per stream so revocation applies immediately.
    let Some((role, device_name)) = store.authorize_peer(peer_id) else {
        let _ = wire::write_json_msg(
            &mut send,
            &PortTunnelReply {
                ok: false,
                error: Some("device is not authorized or has been revoked".into()),
            },
        )
        .await;
        let _ = send.finish();
        return Err(anyhow::anyhow!("unauthorized peer {peer_id}"));
    };

    match kind {
        StreamKind::Control => handle_control_stream(role, send, recv, backend).await,
        StreamKind::Pane => handle_pane_stream(role, device_name, send, recv, backend).await,
        StreamKind::PortTunnel => handle_port_tunnel(role, send, recv, backend).await,
        StreamKind::Transcript => handle_transcript_stream(send, recv, backend).await,
        StreamKind::Pair => unreachable!(),
    }
}

async fn handle_pair_stream(
    peer_id: &str,
    mut send: SendStream,
    mut recv: RecvStream,
    store: &GatewayStore,
    backend: &Arc<dyn GatewayBackend>,
) -> anyhow::Result<()> {
    let req: PairRequest = wire::read_json_msg(&mut recv).await?;
    let reply = match store.complete_pairing(peer_id, &req.secret_b64, &req.device_name, &req.platform)
    {
        Ok(granted_role) => PairReply {
            ok: true,
            error: None,
            host_name: Some(backend.host_name()),
            host_os: Some(std::env::consts::OS.to_string()),
            granted_role,
            protocol_version: PROTOCOL_VERSION,
        },
        Err(err) => PairReply {
            ok: false,
            error: Some(err),
            host_name: None,
            host_os: None,
            granted_role: MobileRole::ObserveOnly,
            protocol_version: PROTOCOL_VERSION,
        },
    };
    wire::write_json_msg(&mut send, &reply).await?;
    let _ = send.finish();
    Ok(())
}

async fn handle_control_stream(
    _role: MobileRole,
    mut send: SendStream,
    mut recv: RecvStream,
    backend: &Arc<dyn GatewayBackend>,
) -> anyhow::Result<()> {
    let _open: ControlStreamOpen = wire::read_json_msg(&mut recv).await?;
    let local = backend.open_control_stream()?;
    bridge_blocking_stream(local, &mut send, &mut recv).await
}

async fn handle_pane_stream(
    role: MobileRole,
    device_name: String,
    mut send: SendStream,
    mut recv: RecvStream,
    backend: &Arc<dyn GatewayBackend>,
) -> anyhow::Result<()> {
    let open: PaneStreamOpen = wire::read_json_msg(&mut recv).await?;
    let local_write = backend.open_pane_stream()?;
    let local_read = local_write.try_clone()?;

    // Open the pane using delta-capable ObserveSince (zero impact on PC PTY!).
    {
        let mut w = &local_write;
        ClientMsg::ObserveSince {
            pane_id: open.pane_id,
            size: WinSize {
                cols: 80,
                rows: 24,
                cell_w: 8,
                cell_h: 17,
            },
            since_offset: open.since_offset,
        }
        .encode(&mut w)?;
    }

    let (down_tx, mut down_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let compress = open.compress_zstd;
    let reader_task = tokio::task::spawn_blocking(move || {
        let mut r = local_read;
        while let Ok(msg) = DaemonMsg::read(&mut r) {
            let mut raw = Vec::new();
            if msg.encode(&mut raw).is_err() {
                break;
            }
            if let Ok(encoded) = wire::encode_pane_chunk(&raw, compress)
                && down_tx.blocking_send(encoded).is_err()
            {
                break;
            }
        }
    });

    let pane_id = open.pane_id;
    let up_task = async {
        loop {
            let Ok(raw) = wire::read_bytes_msg(&mut recv).await else {
                break;
            };
            let mut cur = io::Cursor::new(raw);
            let Ok(msg) = ClientMsg::read(&mut cur) else {
                break;
            };
            match msg {
                ClientMsg::Input(bytes) | ClientMsg::SendInput { bytes, .. } => {
                    if role.can_send_input() {
                        let send_bytes = bytes;
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = PaneClient::local().send_input(pane_id, &send_bytes);
                        })
                        .await;
                    }
                }
                ClientMsg::Lease(mut req) => {
                    if role.can_take_lease() {
                        if let anterm_protocol::pane::LeaseRequest::Take { ref mut by, .. } = req
                            && by.trim().is_empty()
                        {
                            *by = device_name.clone();
                        }
                        let mut w = &local_write;
                        if ClientMsg::Lease(req).encode(&mut w).is_err() {
                            break;
                        }
                    }
                }
                ClientMsg::Detach => {
                    let mut w = &local_write;
                    let _ = ClientMsg::Detach.encode(&mut w);
                    break;
                }
                _ => {}
            }
        }
    };

    let down_task = async {
        while let Some(chunk) = down_rx.recv().await {
            if wire::write_bytes_msg(&mut send, &chunk).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = up_task => {}
        _ = down_task => {}
    }

    let _ = local_write.shutdown(std::net::Shutdown::Both);
    drop(local_write);
    let _ = reader_task.await;
    let _ = send.finish();
    Ok(())
}

async fn handle_transcript_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    backend: &Arc<dyn GatewayBackend>,
) -> anyhow::Result<()> {
    let open: TranscriptStreamOpen = wire::read_json_msg(&mut recv).await?;
    let initial_source = backend.pane_transcript(open.pane_id);
    let mut tailer = match initial_source {
        Some(src) => TranscriptTailer::new(open.pane_id, src.agent, src.session_id, src.path),
        None => TranscriptTailer::new(open.pane_id, None, None, None),
    };

    let mut cursor = open.since_seq;
    let first = tailer.poll_since(cursor);
    cursor = first.next_seq;
    wire::write_json_msg(&mut send, &first).await?;

    let mut interval = tokio::time::interval(Duration::from_millis(250));
    loop {
        interval.tick().await;
        if let Some(src) = backend.pane_transcript(open.pane_id) {
            tailer.update_source(src.agent, src.session_id, src.path);
        }
        let batch = tailer.poll_since(cursor);
        if batch.reset || !batch.entries.is_empty() {
            cursor = batch.next_seq;
            if wire::write_json_msg(&mut send, &batch).await.is_err() {
                break;
            }
        }
    }
    Ok(())
}

async fn handle_port_tunnel(
    role: MobileRole,
    mut send: SendStream,
    mut recv: RecvStream,
    backend: &Arc<dyn GatewayBackend>,
) -> anyhow::Result<()> {
    let open: PortTunnelOpen = wire::read_json_msg(&mut recv).await?;
    if !role.can_tunnel_ports() {
        wire::write_json_msg(
            &mut send,
            &PortTunnelReply {
                ok: false,
                error: Some("device role does not allow port preview tunneling".into()),
            },
        )
        .await?;
        let _ = send.finish();
        return Ok(());
    }
    let allowed = backend.allowed_preview_ports(open.pane_id);
    if !allowed.contains(&open.target_port) {
        wire::write_json_msg(
            &mut send,
            &PortTunnelReply {
                ok: false,
                error: Some(format!(
                    "port {} is not an active listening preview port",
                    open.target_port
                )),
            },
        )
        .await?;
        let _ = send.finish();
        return Ok(());
    }

    let mut tcp = match TcpStream::connect(("127.0.0.1", open.target_port)).await {
        Ok(s) => s,
        Err(e) => {
            wire::write_json_msg(
                &mut send,
                &PortTunnelReply {
                    ok: false,
                    error: Some(format!("connect 127.0.0.1:{} failed: {e}", open.target_port)),
                },
            )
            .await?;
            let _ = send.finish();
            return Ok(());
        }
    };

    wire::write_json_msg(
        &mut send,
        &PortTunnelReply {
            ok: true,
            error: None,
        },
    )
    .await?;

    let (mut tcp_r, mut tcp_w) = tcp.split();
    let up = async {
        let _ = tokio::io::copy(&mut recv, &mut tcp_w).await;
        let _ = tokio::io::AsyncWriteExt::shutdown(&mut tcp_w).await;
    };
    let down = async {
        let _ = tokio::io::copy(&mut tcp_r, &mut send).await;
        let _ = send.finish();
    };
    tokio::join!(up, down);
    Ok(())
}

async fn bridge_blocking_stream(
    local: anterm_core::daemon::transport::Stream,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> anyhow::Result<()> {
    use std::io::{Read as _, Write as _};
    let mut local_r = local.try_clone()?;
    let mut local_w = local;

    let (down_tx, mut down_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);
    let (up_tx, mut up_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);

    let r_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match local_r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if down_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let w_handle = tokio::task::spawn_blocking(move || {
        while let Some(chunk) = up_rx.blocking_recv() {
            if local_w.write_all(&chunk).is_err() || local_w.flush().is_err() {
                break;
            }
        }
        let _ = local_w.shutdown(std::net::Shutdown::Both);
    });

    let send_loop = async {
        while let Some(chunk) = down_rx.recv().await {
            if tokio::io::AsyncWriteExt::write_all(send, &chunk)
                .await
                .is_err()
            {
                break;
            }
        }
    };

    let recv_loop = async {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match tokio::io::AsyncReadExt::read(recv, &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if up_tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    };

    tokio::select! {
        _ = send_loop => {}
        _ = recv_loop => {}
    }

    drop(up_tx);
    let _ = send.finish();
    let _ = tokio::join!(r_handle, w_handle);
    Ok(())
}
