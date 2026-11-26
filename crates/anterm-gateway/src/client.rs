//! Mobile-side (and test-side) `iroh` QUIC client for `anterm-gateway`.
//!
//! Compiles identically on iOS (`aarch64-apple-ios`), Android, macOS, Linux,
//! and Windows with zero local-daemon dependencies.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anterm_core::daemon::control::{
    ControlClientMsg, ControlEvent, ControlHello, ControlHelloOk, ControlReply, ControlRequest,
    ControlServerMsg, ReplyOk,
};
use anterm_protocol::gateway::{
    ControlStreamOpen, GATEWAY_ALPN, PairReply, PairRequest, PairingBundle, PaneStreamOpen,
    PortTunnelOpen, PortTunnelReply, StreamKind, TranscriptStreamOpen,
};
use anterm_protocol::pane::{ClientMsg, DaemonMsg, MAX_FRAME};
use anterm_protocol::transcript::TranscriptBatch;
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::wire;

/// Client-side `iroh` endpoint managing E2EE connections to desktop/server hosts.
pub struct GatewayClient {
    endpoint: Endpoint,
}

impl GatewayClient {
    /// Bind a client endpoint. Pass `relay_url: None` + `lan_only: true` for
    /// zero-relay LAN/direct operation, or provide a custom `iroh-relay` HTTPS URL.
    pub async fn bind(
        secret_key: SecretKey,
        relay_url: Option<&str>,
        lan_only: bool,
    ) -> anyhow::Result<Self> {
        let mut builder = Endpoint::builder(presets::Minimal).secret_key(secret_key);
        let mode = resolve_relay_mode(relay_url, lan_only)?;
        builder = builder.relay_mode(mode);
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| anyhow::anyhow!("iroh client bind failed: {e}"))?;
        Ok(Self { endpoint })
    }

    pub fn node_id(&self) -> String {
        self.endpoint.id().to_string()
    }

    /// Perform single-use QR pairing against the host named in `bundle`.
    pub async fn pair(
        &self,
        bundle: &PairingBundle,
        mut req: PairRequest,
    ) -> anyhow::Result<PairReply> {
        if req.secret_b64.is_empty() {
            req.secret_b64 = bundle.secret_b64.clone();
        }
        let addr = build_endpoint_addr(
            &bundle.node_id,
            &bundle.lan_addrs,
            bundle.relay_url.as_deref(),
        )?;
        let conn = self
            .endpoint
            .connect(addr, GATEWAY_ALPN)
            .await
            .map_err(|e| anyhow::anyhow!("pair connect failed: {e}"))?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("pair stream open failed: {e}"))?;
        wire::write_stream_kind(&mut send, StreamKind::Pair).await?;
        wire::write_json_msg(&mut send, &req).await?;
        let _ = send.finish();
        let reply: PairReply = wire::read_json_msg(&mut recv).await?;
        conn.close(0u32.into(), b"paired");
        Ok(reply)
    }

    /// Open a persistent E2EE QUIC connection to an already-paired host.
    pub async fn connect(
        &self,
        node_id: &str,
        lan_addrs: &[String],
        relay_url: Option<&str>,
    ) -> anyhow::Result<GatewayConnection> {
        let addr = build_endpoint_addr(node_id, lan_addrs, relay_url)?;
        let conn = self
            .endpoint
            .connect(addr, GATEWAY_ALPN)
            .await
            .map_err(|e| anyhow::anyhow!("gateway connect failed: {e}"))?;
        Ok(GatewayConnection { conn })
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

/// An authenticated, multiplexed QUIC connection to one `anterm-gateway` host.
#[derive(Clone)]
pub struct GatewayConnection {
    conn: Connection,
}

impl GatewayConnection {
    pub fn remote_node_id(&self) -> String {
        self.conn.remote_id().to_string()
    }

    /// Open a raw bidirectional control stream (`StreamKind::Control`).
    pub async fn open_control(
        &self,
        open: ControlStreamOpen,
    ) -> anyhow::Result<(SendStream, RecvStream)> {
        let (mut send, recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("control stream open failed: {e}"))?;
        wire::write_stream_kind(&mut send, StreamKind::Control).await?;
        wire::write_json_msg(&mut send, &open).await?;
        Ok((send, recv))
    }

    /// Open a high-level async [`GatewayControlClient`] speaking `anterm-core`'s
    /// control protocol (RPC requests + blob replies + real-time push events).
    pub async fn open_control_client(
        &self,
        device_name: &str,
        route_key: Option<String>,
    ) -> anyhow::Result<GatewayControlClient> {
        let (send, recv) = self
            .open_control(ControlStreamOpen {
                device_name: device_name.to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                route_key,
            })
            .await?;
        GatewayControlClient::handshake(send, recv, device_name).await
    }

    /// Open a pane observation/interaction stream (`StreamKind::Pane`).
    pub async fn open_pane(&self, open: PaneStreamOpen) -> anyhow::Result<GatewayPaneStream> {
        let (mut send, recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("pane stream open failed: {e}"))?;
        wire::write_stream_kind(&mut send, StreamKind::Pane).await?;
        wire::write_json_msg(&mut send, &open).await?;
        Ok(GatewayPaneStream { send, recv })
    }

    /// Open a structured coding-agent transcript stream (`StreamKind::Transcript`).
    pub async fn open_transcript(
        &self,
        open: TranscriptStreamOpen,
    ) -> anyhow::Result<GatewayTranscriptStream> {
        let (mut send, recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("transcript stream open failed: {e}"))?;
        wire::write_stream_kind(&mut send, StreamKind::Transcript).await?;
        wire::write_json_msg(&mut send, &open).await?;
        let _ = send.finish();
        Ok(GatewayTranscriptStream { recv })
    }

    /// Open a localhost dev-server TCP tunnel (`StreamKind::PortTunnel`).
    pub async fn open_port_tunnel(
        &self,
        open: PortTunnelOpen,
    ) -> anyhow::Result<(SendStream, RecvStream)> {
        let (mut send, mut recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| anyhow::anyhow!("port tunnel open failed: {e}"))?;
        wire::write_stream_kind(&mut send, StreamKind::PortTunnel).await?;
        wire::write_json_msg(&mut send, &open).await?;
        let reply: PortTunnelReply = wire::read_json_msg(&mut recv).await?;
        if !reply.ok {
            return Err(anyhow::anyhow!(
                "{}",
                reply.error.unwrap_or_else(|| "port tunnel refused".into())
            ));
        }
        Ok((send, recv))
    }

    pub fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
    }
}

/// Framed async handle for one pane stream over QUIC.
pub struct GatewayPaneStream {
    send: SendStream,
    recv: RecvStream,
}

impl GatewayPaneStream {
    pub async fn send_msg(&mut self, msg: &ClientMsg) -> anyhow::Result<()> {
        let mut raw = Vec::new();
        msg.encode(&mut raw)?;
        wire::write_bytes_msg(&mut self.send, &raw).await?;
        Ok(())
    }

    pub async fn recv_msg(&mut self) -> anyhow::Result<DaemonMsg> {
        let chunk = wire::read_bytes_msg(&mut self.recv).await?;
        let raw = wire::decode_pane_chunk(&chunk)?;
        let mut cursor = std::io::Cursor::new(raw);
        let msg = DaemonMsg::read(&mut cursor)?;
        Ok(msg)
    }

    pub fn split(self) -> (SendStream, RecvStream) {
        (self.send, self.recv)
    }
}

/// Async stream of incremental [`TranscriptBatch`] frames.
pub struct GatewayTranscriptStream {
    recv: RecvStream,
}

impl GatewayTranscriptStream {
    pub async fn next_batch(&mut self) -> anyhow::Result<TranscriptBatch> {
        let batch: TranscriptBatch = wire::read_json_msg(&mut self.recv).await?;
        Ok(batch)
    }
}

type PendingReply = oneshot::Sender<anyhow::Result<(ReplyOk, Vec<u8>)>>;

/// Async, multiplexed control-plane client over a QUIC [`StreamKind::Control`]
/// stream. Compiles identically on iOS and desktop with zero local-daemon deps.
pub struct GatewayControlClient {
    hello_ok: ControlHelloOk,
    next_id: AtomicU64,
    out_tx: mpsc::UnboundedSender<Vec<u8>>,
    pending: Arc<Mutex<HashMap<u64, PendingReply>>>,
    events_rx: Mutex<mpsc::UnboundedReceiver<ControlEvent>>,
    io_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl GatewayControlClient {
    async fn handshake(
        mut send: SendStream,
        mut recv: RecvStream,
        device_name: &str,
    ) -> anyhow::Result<Self> {
        let hello = ControlHello::host_rpc("", device_name);
        write_control_frame(&mut send, &ControlClientMsg::Hello(hello)).await?;
        let hello_ok = match read_control_frame(&mut recv).await? {
            ControlServerMsg::HelloOk(ok) => ok,
            other => anyhow::bail!("expected ControlServerMsg::HelloOk, got {other:?}"),
        };

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel::<ControlEvent>();
        let pending: Arc<Mutex<HashMap<u64, PendingReply>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_bg = Arc::clone(&pending);

        let handle = tokio::spawn(async move {
            let write_loop = async {
                while let Some(raw) = out_rx.recv().await {
                    if send.write_all(&raw).await.is_err() {
                        break;
                    }
                }
                let _ = send.finish();
            };

            let read_loop = async {
                while let Ok(msg) = read_control_frame(&mut recv).await {
                    match msg {
                        ControlServerMsg::Event(ev) => {
                            let _ = ev_tx.send(ev);
                        }
                        ControlServerMsg::Response { req_id, reply } => {
                            if let Some(tx) = pending_bg.lock().await.remove(&req_id) {
                                let _ = tx.send(reply_to_result(reply, Vec::new()));
                            }
                        }
                        ControlServerMsg::ResponseBlob {
                            req_id,
                            reply,
                            blob,
                        } => {
                            if let Some(tx) = pending_bg.lock().await.remove(&req_id) {
                                let _ = tx.send(reply_to_result(reply, blob));
                            }
                        }
                        ControlServerMsg::HelloOk(_) => {}
                    }
                }
                let mut map = pending_bg.lock().await;
                for (_, tx) in map.drain() {
                    let _ = tx.send(Err(anyhow::anyhow!("control stream closed")));
                }
            };

            tokio::select! {
                _ = write_loop => {}
                _ = read_loop => {}
            }
        });

        Ok(Self {
            hello_ok,
            next_id: AtomicU64::new(1),
            out_tx,
            pending,
            events_rx: Mutex::new(ev_rx),
            io_task: Mutex::new(Some(handle)),
        })
    }

    pub fn hello(&self) -> &ControlHelloOk {
        &self.hello_ok
    }

    pub async fn request(&self, req: ControlRequest) -> anyhow::Result<ReplyOk> {
        let (ok, _) = self.call_internal(req, None).await?;
        Ok(ok)
    }

    pub async fn request_with_blob_reply(
        &self,
        req: ControlRequest,
    ) -> anyhow::Result<(ReplyOk, Vec<u8>)> {
        self.call_internal(req, None).await
    }

    pub async fn request_with_blob(
        &self,
        req: ControlRequest,
        blob: Vec<u8>,
    ) -> anyhow::Result<(ReplyOk, Vec<u8>)> {
        self.call_internal(req, Some(blob)).await
    }

    pub async fn next_event(&self) -> Option<ControlEvent> {
        self.events_rx.lock().await.recv().await
    }

    pub async fn close(&self) {
        if let Some(handle) = self.io_task.lock().await.take() {
            handle.abort();
        }
    }

    async fn call_internal(
        &self,
        req: ControlRequest,
        blob: Option<Vec<u8>>,
    ) -> anyhow::Result<(ReplyOk, Vec<u8>)> {
        let req_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = match blob {
            Some(blob) => ControlClientMsg::RequestBlob { req_id, req, blob },
            None => ControlClientMsg::Request { req_id, req },
        };
        let encoded = encode_control_frame(&msg)?;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(req_id, tx);
        if self.out_tx.send(encoded).is_err() {
            self.pending.lock().await.remove(&req_id);
            anyhow::bail!("control stream closed");
        }
        rx.await
            .map_err(|_| anyhow::anyhow!("control stream dropped before reply"))?
    }
}

fn reply_to_result(reply: ControlReply, blob: Vec<u8>) -> anyhow::Result<(ReplyOk, Vec<u8>)> {
    match reply {
        ControlReply::Ok(ok) => Ok((ok, blob)),
        ControlReply::Err(err) => Err(anyhow::anyhow!("{}", err.msg)),
    }
}

fn encode_control_frame(msg: &ControlClientMsg) -> anyhow::Result<Vec<u8>> {
    let (kind, payload) = msg.to_frame()?;
    let len = u32::try_from(payload.len())
        .map_err(|_| anyhow::anyhow!("control frame exceeds u32 length"))?;
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&payload);
    Ok(out)
}

async fn write_control_frame(send: &mut SendStream, msg: &ControlClientMsg) -> anyhow::Result<()> {
    let raw = encode_control_frame(msg)?;
    send.write_all(&raw).await?;
    Ok(())
}

async fn read_control_frame(recv: &mut RecvStream) -> anyhow::Result<ControlServerMsg> {
    let mut header = [0u8; 5];
    recv.read_exact(&mut header).await?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let kind = header[4];
    if len > MAX_FRAME {
        anyhow::bail!("control server frame {len} exceeds MAX_FRAME ({MAX_FRAME})");
    }
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload).await?;
    let msg = ControlServerMsg::from_frame(kind, payload)?;
    Ok(msg)
}

pub(crate) fn resolve_relay_mode(relay_url: Option<&str>, lan_only: bool) -> anyhow::Result<RelayMode> {
    if lan_only {
        return Ok(RelayMode::Disabled);
    }
    match relay_url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => {
            let parsed = RelayUrl::from_str(url)
                .map_err(|e| anyhow::anyhow!("invalid relay URL '{url}': {e}"))?;
            Ok(RelayMode::Custom(RelayMap::from(parsed)))
        }
        None => Ok(RelayMode::Disabled),
    }
}

pub(crate) fn build_endpoint_addr(
    node_id: &str,
    lan_addrs: &[String],
    relay_url: Option<&str>,
) -> anyhow::Result<EndpointAddr> {
    let id = EndpointId::from_str(node_id)
        .map_err(|e| anyhow::anyhow!("invalid endpoint id '{node_id}': {e}"))?;
    let mut addrs = Vec::new();
    for raw in lan_addrs {
        if let Ok(sock) = raw.parse::<SocketAddr>() {
            addrs.push(TransportAddr::Ip(sock));
        }
    }
    if let Some(url) = relay_url.map(str::trim).filter(|u| !u.is_empty())
        && let Ok(parsed) = RelayUrl::from_str(url)
    {
        addrs.push(TransportAddr::Relay(parsed));
    }
    Ok(EndpointAddr::from_parts(id, addrs))
}
