//! Stateful UniFFI session objects bridging SwiftUI/Metal on iOS to
//! `anterm-gateway` (`iroh` QUIC) and `anterm-client-core` (`HeadlessTerminal`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anterm_client_core::HeadlessTerminal;
use anterm_core::daemon::control::{ControlEvent, ControlRequest, PaneAgentState, ReplyOk};
use anterm_core::machine::{Machine, PaneRecord, TabId};
use anterm_gateway::client::{
    GatewayClient, GatewayConnection, GatewayControlClient, GatewayPaneStream,
};
use anterm_gateway::{
    MobileRole, PairRequest, PairingBundle, PaneStreamOpen, PortTunnelOpen, TranscriptStreamOpen,
};
use anterm_protocol::cli_agent::AgentStatus;
use anterm_protocol::pane::{ClientMsg, LeaseRequest, WinSize};
use anterm_protocol::util::{unix_now, write_atomic_private};
use iroh::SecretKey;
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::types::{
    FfiAgentStatus, FfiFileEntry, FfiGitChangedFile, FfiGitDiffSnapshot, FfiMobileRole,
    FfiPairedHost, FfiPaneSummary, FfiQuickApproval, FfiReflowLine, FfiRemoteRoute, FfiTabSummary,
    FfiTranscriptBatch, FfiTranscriptEntry, FfiWorkspaceEvent, FfiWorkspaceEventKind,
    FfiWorkspaceSnapshot, MobileError, approval_bytes, parse_cli_agent,
};

const DEVICE_KEY_FILE: &str = "mobile_device.key";
const DEVICE_NAME_FILE: &str = "mobile_device_name.txt";
const PAIRED_HOSTS_FILE: &str = "paired_hosts.json";

/// Root mobile client owned by the iOS app process.
#[derive(uniffi::Object)]
pub struct MobileClient {
    storage_dir: PathBuf,
    device_name: Mutex<String>,
    secret_key: SecretKey,
    rt: Arc<tokio::runtime::Runtime>,
    hosts: Mutex<Vec<FfiPairedHost>>,
}

#[uniffi::export]
impl MobileClient {
    /// Initialize (or reload) the mobile client identity and paired host list
    /// stored under `storage_dir`.
    #[uniffi::constructor]
    pub fn new(storage_dir: String, device_name: String) -> Result<Arc<Self>, MobileError> {
        let dir = PathBuf::from(&storage_dir);
        std::fs::create_dir_all(&dir)?;
        let secret_key = load_or_create_device_key(&dir)?;
        let hosts = load_paired_hosts(&dir).unwrap_or_default();
        let saved_name = std::fs::read_to_string(dir.join(DEVICE_NAME_FILE))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let initial_name = saved_name.unwrap_or_else(|| {
            let trimmed = device_name.trim();
            if trimmed.is_empty() {
                "Anterm iOS".to_string()
            } else {
                trimmed.chars().take(64).collect()
            }
        });
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .map_err(|e| MobileError::msg(format!("tokio runtime failed: {e}")))?;

        Ok(Arc::new(Self {
            storage_dir: dir,
            device_name: Mutex::new(initial_name),
            secret_key,
            rt: Arc::new(rt),
            hosts: Mutex::new(hosts),
        }))
    }

    /// Public Ed25519 `iroh` NodeId of this mobile device.
    pub fn device_node_id(&self) -> String {
        self.secret_key.public().to_string()
    }

    /// Current display name advertised by this mobile device during pairing.
    pub fn device_name(&self) -> String {
        self.device_name
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Update and persist this mobile device's display name (used when pairing
    /// with desktop hosts, since iOS 16+ redacts `UIDevice.current.name`).
    pub fn set_device_name(&self, name: String) -> Result<String, MobileError> {
        let cleaned: String = name.trim().chars().take(64).collect();
        if cleaned.is_empty() {
            return Err(MobileError::msg("device name cannot be empty"));
        }
        write_atomic_private(&self.storage_dir.join(DEVICE_NAME_FILE), cleaned.as_bytes())?;
        let mut guard = self.device_name.lock().unwrap_or_else(|e| e.into_inner());
        *guard = cleaned.clone();
        Ok(cleaned)
    }

    /// Return all hosts paired with this mobile device.
    pub fn list_paired_hosts(&self) -> Vec<FfiPairedHost> {
        self.hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Rename a previously paired desktop/server host locally so multiple
    /// computers are easy to tell apart.
    pub fn rename_paired_host(
        &self,
        host_node_id: String,
        new_name: String,
    ) -> Result<FfiPairedHost, MobileError> {
        let cleaned: String = new_name.trim().chars().take(64).collect();
        if cleaned.is_empty() {
            return Err(MobileError::msg("host name cannot be empty"));
        }
        let mut guard = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        let Some(host) = guard.iter_mut().find(|h| h.host_node_id == host_node_id) else {
            return Err(MobileError::msg(format!(
                "unknown paired host: {host_node_id}"
            )));
        };
        host.host_name = cleaned;
        let updated = host.clone();
        save_paired_hosts(&self.storage_dir, &guard)?;
        Ok(updated)
    }

    /// Remove a previously paired host from local storage.
    pub fn remove_paired_host(&self, host_node_id: String) -> Result<(), MobileError> {
        let mut guard = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|h| h.host_node_id != host_node_id);
        save_paired_hosts(&self.storage_dir, &guard)?;
        Ok(())
    }

    /// Pair with a desktop/server host by scanning or pasting its
    /// `anterm://pair/v1?...` QR URI.
    pub fn pair_with_uri(&self, uri: String) -> Result<FfiPairedHost, MobileError> {
        let bundle = PairingBundle::from_uri(&uri).map_err(|e| MobileError::msg(e.to_string()))?;
        if bundle.is_expired(unix_now()) {
            return Err(MobileError::msg("pairing QR code has expired"));
        }

        let secret_key = self.secret_key.clone();
        let device_name = self.device_name();
        let relay = bundle.relay_url.clone();
        let lan_only = relay.is_none();

        let bundle_for_pair = bundle.clone();
        let reply = self.rt.block_on(async move {
            let client = GatewayClient::bind(secret_key, relay.as_deref(), lan_only).await?;
            let req = PairRequest {
                secret_b64: bundle_for_pair.secret_b64.clone(),
                device_name,
                platform: "ios".to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                requested_role: MobileRole::Interactive,
            };
            let reply = client.pair(&bundle_for_pair, req).await?;
            client.close().await;
            Ok::<_, anyhow::Error>(reply)
        })?;

        if !reply.ok {
            return Err(MobileError::msg(
                reply.error.unwrap_or_else(|| "pairing rejected".into()),
            ));
        }

        let paired = FfiPairedHost {
            host_node_id: bundle.node_id,
            host_name: reply
                .host_name
                .filter(|n| !n.is_empty())
                .unwrap_or(bundle.host_name),
            relay_url: bundle.relay_url,
            direct_addrs: bundle.lan_addrs,
            role: reply.granted_role.into(),
            paired_at: unix_now(),
        };

        let mut guard = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|h| h.host_node_id != paired.host_node_id);
        guard.push(paired.clone());
        save_paired_hosts(&self.storage_dir, &guard)?;
        Ok(paired)
    }

    /// Connect to a paired host over E2EE `iroh` QUIC.
    pub fn connect_host(&self, host_node_id: String) -> Result<Arc<MobileHostSession>, MobileError> {
        let host = self
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|h| h.host_node_id == host_node_id)
            .cloned()
            .ok_or_else(|| MobileError::msg(format!("unknown paired host: {host_node_id}")))?;

        let secret_key = self.secret_key.clone();
        let relay = host.relay_url.clone();
        let lan_addrs = host.direct_addrs.clone();
        let lan_only = relay.is_none();
        let node_id = host.host_node_id.clone();

        let (client, conn) = self.rt.block_on(async move {
            let client = GatewayClient::bind(secret_key, relay.as_deref(), lan_only).await?;
            let conn = client.connect(&node_id, &lan_addrs, relay.as_deref()).await?;
            Ok::<_, anyhow::Error>((client, conn))
        })?;

        Ok(Arc::new(MobileHostSession {
            host,
            device_name: self.device_name(),
            rt: Arc::clone(&self.rt),
            client: Mutex::new(Some(client)),
            conn,
            control: Mutex::new(None),
            connected: AtomicBool::new(true),
        }))
    }
}

/// Active E2EE QUIC session to one paired host.
#[derive(uniffi::Object)]
pub struct MobileHostSession {
    host: FfiPairedHost,
    device_name: String,
    rt: Arc<tokio::runtime::Runtime>,
    client: Mutex<Option<GatewayClient>>,
    conn: GatewayConnection,
    control: Mutex<Option<Arc<GatewayControlClient>>>,
    connected: AtomicBool,
}

impl MobileHostSession {
    fn ensure_control(&self) -> Result<Arc<GatewayControlClient>, MobileError> {
        {
            let guard = self.control.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = guard.as_ref() {
                return Ok(Arc::clone(existing));
            }
        }
        let conn = self.conn.clone();
        let dev = self.device_name.clone();
        let client = self
            .rt
            .block_on(async move { conn.open_control_client(&dev, None).await })?;
        let arc = Arc::new(client);
        let mut guard = self.control.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Arc::clone(&arc));
        Ok(arc)
    }

    fn run_git(&self, ctrl: &GatewayControlClient, cwd: &str, args: &[&str]) -> anyhow::Result<String> {
        let cwd_s = cwd.to_string();
        let arg_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.rt.block_on(async move {
            let reply = ctrl
                .request(ControlRequest::Git {
                    cwd: cwd_s,
                    args: arg_vec,
                })
                .await?;
            match reply {
                ReplyOk::Output(out) => Ok(String::from_utf8_lossy(&out.stdout).to_string()),
                other => anyhow::bail!("unexpected git reply: {other:?}"),
            }
        })
    }
}

#[uniffi::export]
impl MobileHostSession {
    pub fn host_name(&self) -> String {
        self.host.host_name.clone()
    }

    pub fn host_node_id(&self) -> String {
        self.host.host_node_id.clone()
    }

    pub fn role(&self) -> FfiMobileRole {
        self.host.role
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Fetch the live desktop workspace snapshot (tabs, split panes, tags,
    /// coding-agent statuses, active listening ports, and SSH/WSL routes).
    pub fn fetch_workspace(&self) -> Result<FfiWorkspaceSnapshot, MobileError> {
        let ctrl = self.ensure_control()?;
        let (machine, agent_states, routes) = self.rt.block_on(async {
            let machine = match ctrl.request(ControlRequest::MachineGet).await? {
                ReplyOk::MachineTree(m) => *m,
                other => anyhow::bail!("unexpected MachineGet reply: {other:?}"),
            };
            let agents = match ctrl.request(ControlRequest::AgentStates).await {
                Ok(ReplyOk::AgentStates(states)) => states,
                _ => Vec::new(),
            };
            let routes = match ctrl.request(ControlRequest::Routes).await {
                Ok(ReplyOk::Routes(routes)) => routes,
                _ => Vec::new(),
            };
            Ok::<_, anyhow::Error>((machine, agents, routes))
        })?;

        let mut ports_by_pane: HashMap<u64, Vec<u16>> = HashMap::new();
        for pane in machine.panes.iter().filter(|p| p.live) {
            if let Ok(ReplyOk::PaneProcs(procs)) = self.rt.block_on(
                ctrl.request(ControlRequest::PaneProcs { pane_id: pane.id }),
            ) {
                let mut ports: Vec<u16> = procs.ports.into_iter().map(|p| p.port).collect();
                ports.sort_unstable();
                ports.dedup();
                ports_by_pane.insert(pane.id, ports);
            }
        }

        Ok(build_workspace_snapshot(
            &self.host.host_name,
            machine,
            agent_states,
            ports_by_pane,
            routes,
        ))
    }

    /// Wait up to `timeout_ms` for the next live desktop control event
    /// (`Layout`, `LayoutResync`, `AgentStatus`, or `PaneExited`).
    pub fn poll_workspace_event(&self, timeout_ms: u64) -> Option<FfiWorkspaceEvent> {
        let ctrl = self.ensure_control().ok()?;
        let ev = self.rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(timeout_ms), ctrl.next_event())
                .await
                .ok()
                .flatten()
        })?;
        match ev {
            ControlEvent::Layout { .. } | ControlEvent::LayoutResync => Some(FfiWorkspaceEvent {
                kind: FfiWorkspaceEventKind::LayoutChanged,
                pane_id: None,
                detail: None,
            }),
            ControlEvent::AgentStatus { pane_id, json } => Some(FfiWorkspaceEvent {
                kind: FfiWorkspaceEventKind::AgentStatusChanged,
                pane_id: Some(pane_id),
                detail: Some(json.to_string()),
            }),
            ControlEvent::PaneExited { pane_id, code } => Some(FfiWorkspaceEvent {
                kind: FfiWorkspaceEventKind::PaneExited,
                pane_id: Some(pane_id),
                detail: code.map(|c| format!("exit {c}")),
            }),
            _ => None,
        }
    }

    /// Explicit Handoff: ask the desktop host to switch its active tab to `tab_id`.
    /// Regular tab navigation on mobile never invokes this unless the user taps Handoff.
    pub fn handoff_focus_desktop_tab(&self, tab_id: String) -> Result<(), MobileError> {
        let tab: TabId = serde_json::from_value(serde_json::Value::String(tab_id))
            .map_err(|e| MobileError::msg(format!("invalid tab_id: {e}")))?;
        let ctrl = self.ensure_control()?;
        self.rt.block_on(async move {
            ctrl.request(ControlRequest::SetActiveTab { tab }).await?;
            Ok::<_, anyhow::Error>(())
        })?;
        Ok(())
    }

    /// List entries inside `dir_path` on the paired host (directories first).
    pub fn list_directory(&self, dir_path: String) -> Result<Vec<FfiFileEntry>, MobileError> {
        let ctrl = self.ensure_control()?;
        let base = dir_path.clone();
        let entries = self.rt.block_on(async move {
            match ctrl
                .request(ControlRequest::ReadDir {
                    dir: dir_path,
                    root: None,
                })
                .await?
            {
                ReplyOk::Entries(entries) => Ok::<_, anyhow::Error>(entries),
                other => anyhow::bail!("unexpected ReadDir reply: {other:?}"),
            }
        })?;

        let mut out: Vec<FfiFileEntry> = entries
            .into_iter()
            .map(|e| {
                let full = Path::new(&base).join(&e.name);
                FfiFileEntry {
                    name: e.name,
                    path: full.display().to_string(),
                    is_dir: e.is_dir,
                    is_symlink: e.is_symlink,
                    ignored: e.ignored,
                }
            })
            .collect();
        out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    /// Read a UTF-8 text file from the paired host (capped at `max_bytes`).
    pub fn read_file_text(&self, file_path: String, max_bytes: u32) -> Result<String, MobileError> {
        let ctrl = self.ensure_control()?;
        let (_, blob) = self.rt.block_on(async move {
            ctrl.request_with_blob_reply(ControlRequest::ReadFile {
                path: file_path,
                max_bytes: u64::from(max_bytes.max(1024)),
            })
            .await
        })?;
        Ok(String::from_utf8_lossy(&blob).into_owned())
    }

    /// Fuzzy-search files under `root_dir` for mobile `@file` mentions.
    pub fn search_files(
        &self,
        root_dir: String,
        query: String,
        limit: u32,
    ) -> Result<Vec<FfiFileEntry>, MobileError> {
        let ctrl = self.ensure_control()?;
        let hits = self.rt.block_on(async move {
            match ctrl
                .request(ControlRequest::Search {
                    roots: vec![root_dir],
                    query,
                    limit: u64::from(limit.clamp(1, 200)),
                    max_dirs: 2000,
                    show_hidden: false,
                })
                .await?
            {
                ReplyOk::Hits(hits) => Ok::<_, anyhow::Error>(hits),
                other => anyhow::bail!("unexpected Search reply: {other:?}"),
            }
        })?;
        Ok(hits
            .into_iter()
            .map(|h| FfiFileEntry {
                name: h.name,
                path: h.path.display().to_string(),
                is_dir: h.is_dir,
                is_symlink: false,
                ignored: h.ignored,
            })
            .collect())
    }

    /// Fetch live Git branch, changed file list, and unified diff for `cwd`
    /// (optionally filtered to `file_path`).
    pub fn fetch_git_diff(
        &self,
        cwd: String,
        file_path: Option<String>,
    ) -> Result<FfiGitDiffSnapshot, MobileError> {
        let ctrl = self.ensure_control()?;
        let repo_root = self
            .rt
            .block_on(ctrl.request(ControlRequest::RepoRoot { path: cwd.clone() }))
            .ok()
            .and_then(|r| match r {
                ReplyOk::OptPath(p) => p,
                _ => None,
            })
            .unwrap_or(cwd);

        let branch = self
            .run_git(&ctrl, &repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_default()
            .trim()
            .to_string();

        let porcelain = self
            .run_git(&ctrl, &repo_root, &["status", "--porcelain=v1"])
            .unwrap_or_default();
        let files = parse_porcelain_files(&porcelain);

        let diff_args: Vec<&str> = match file_path.as_deref().filter(|s| !s.is_empty()) {
            Some(path) => vec!["diff", "HEAD", "--", path],
            None => vec!["diff", "HEAD"],
        };
        let mut unified_diff = self
            .run_git(&ctrl, &repo_root, &diff_args)
            .unwrap_or_default();
        if unified_diff.trim().is_empty() {
            // Fallback for repos without an initial commit yet or unstaged-only diff.
            let fallback: Vec<&str> = match file_path.as_deref().filter(|s| !s.is_empty()) {
                Some(path) => vec!["diff", "--", path],
                None => vec!["diff"],
            };
            unified_diff = self
                .run_git(&ctrl, &repo_root, &fallback)
                .unwrap_or_default();
        }

        Ok(FfiGitDiffSnapshot {
            repo_root,
            branch,
            files,
            unified_diff,
        })
    }

    /// Upload a photo/screenshot/attachment from iOS into the host's private temp
    /// directory and return its host path for attaching to an Agent prompt.
    pub fn upload_attachment(
        &self,
        filename: String,
        mime: String,
        data: Vec<u8>,
    ) -> Result<String, MobileError> {
        let ctrl = self.ensure_control()?;
        let (reply, _) = self.rt.block_on(async move {
            ctrl.request_with_blob(
                ControlRequest::UploadTempFile { filename, mime },
                data,
            )
            .await
        })?;
        match reply {
            ReplyOk::Path(path) => Ok(path),
            other => Err(MobileError::msg(format!(
                "unexpected UploadTempFile reply: {other:?}"
            ))),
        }
    }

    /// Query active localhost listening ports inside `pane_id`.
    pub fn query_pane_ports(&self, pane_id: u64) -> Result<Vec<u16>, MobileError> {
        let ctrl = self.ensure_control()?;
        let reply = self
            .rt
            .block_on(ctrl.request(ControlRequest::PaneProcs { pane_id }))?;
        match reply {
            ReplyOk::PaneProcs(procs) => {
                let mut ports: Vec<u16> = procs.ports.into_iter().map(|p| p.port).collect();
                ports.sort_unstable();
                ports.dedup();
                Ok(ports)
            }
            other => Err(MobileError::msg(format!(
                "unexpected PaneProcs reply: {other:?}"
            ))),
        }
    }

    /// Open a zero-PC-interference pane observation stream (`ObserveSince`).
    pub fn open_pane(
        &self,
        pane_id: u64,
        resume_offset: u64,
    ) -> Result<Arc<MobilePaneSession>, MobileError> {
        let conn = self.conn.clone();
        let since_offset = (resume_offset > 0).then_some(resume_offset);
        let mut stream: GatewayPaneStream = self.rt.block_on(async move {
            let mut s = conn
                .open_pane(PaneStreamOpen {
                    pane_id,
                    route_key: None,
                    since_offset,
                    compress_zstd: true,
                })
                .await?;
            s.send_msg(&ClientMsg::ObserveSince {
                pane_id,
                size: WinSize {
                    cols: 100,
                    rows: 30,
                    cell_w: 0,
                    cell_h: 0,
                },
                since_offset,
            })
            .await?;
            Ok::<_, anyhow::Error>(s)
        })?;

        let term = Arc::new(Mutex::new(HeadlessTerminal::new(100, 30)));
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientMsg>();
        let alive = Arc::new(AtomicBool::new(true));

        let term_bg = Arc::clone(&term);
        let alive_bg = Arc::clone(&alive);
        let handle = self.rt.spawn(async move {
            loop {
                tokio::select! {
                    maybe_out = rx.recv() => {
                        match maybe_out {
                            Some(msg) => {
                                if stream.send_msg(&msg).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    incoming = stream.recv_msg() => {
                        match incoming {
                            Ok(daemon_msg) => {
                                let mut guard = term_bg.lock().unwrap_or_else(|e| e.into_inner());
                                guard.apply_daemon_msg(&daemon_msg);
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
            alive_bg.store(false, Ordering::Relaxed);
        });

        Ok(Arc::new(MobilePaneSession {
            pane_id,
            device_name: self.device_name.clone(),
            term,
            tx,
            alive,
            has_lease: AtomicBool::new(false),
            task: Mutex::new(Some(handle)),
        }))
    }

    /// Open a Layer 1 native coding-agent transcript stream (`StreamKind::Transcript`).
    pub fn open_transcript(
        &self,
        pane_id: u64,
        since_seq: u64,
    ) -> Result<Arc<MobileTranscriptSession>, MobileError> {
        let conn = self.conn.clone();
        let mut stream = self.rt.block_on(async move {
            conn.open_transcript(TranscriptStreamOpen { pane_id, since_seq })
                .await
        })?;

        let (tx, rx) = mpsc::unbounded_channel::<FfiTranscriptBatch>();
        let cached = Arc::new(Mutex::new(Vec::<FfiTranscriptEntry>::new()));
        let cached_bg = Arc::clone(&cached);
        let handle = self.rt.spawn(async move {
            while let Ok(batch) = stream.next_batch().await {
                let ffi_batch: FfiTranscriptBatch = batch.into();
                {
                    let mut guard = cached_bg.lock().unwrap_or_else(|e| e.into_inner());
                    if ffi_batch.reset {
                        guard.clear();
                    }
                    guard.extend(ffi_batch.entries.iter().cloned());
                }
                if tx.send(ffi_batch).is_err() {
                    break;
                }
            }
        });

        Ok(Arc::new(MobileTranscriptSession {
            rt: Arc::clone(&self.rt),
            rx: Mutex::new(rx),
            cached,
            task: Mutex::new(Some(handle)),
        }))
    }

    /// Start a local loopback HTTP/WebSocket proxy on iOS (`127.0.0.1:<ephemeral>`)
    /// tunneling directly to `remote_port` on the desktop over E2EE QUIC so
    /// `WKWebView` previews localhost dev servers with zero VPN.
    pub fn start_preview_proxy(
        &self,
        remote_port: u16,
        pane_id: Option<u64>,
    ) -> Result<Arc<MobilePreviewProxy>, MobileError> {
        let conn = self.conn.clone();
        let (local_port, handle) = self.rt.block_on(async move {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
            let local_port = listener.local_addr()?.port();
            let handle = tokio::spawn(async move {
                while let Ok((mut tcp, _peer)) = listener.accept().await {
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        let Ok((mut q_send, mut q_recv)) = conn
                            .open_port_tunnel(PortTunnelOpen {
                                target_port: remote_port,
                                pane_id,
                            })
                            .await
                        else {
                            return;
                        };
                        let (mut tcp_read, mut tcp_write) = tcp.split();
                        let up = async {
                            let _ = tokio::io::copy(&mut tcp_read, &mut q_send).await;
                            let _ = q_send.finish();
                        };
                        let down = async {
                            let _ = tokio::io::copy(&mut q_recv, &mut tcp_write).await;
                            let _ = tcp_write.shutdown().await;
                        };
                        tokio::join!(up, down);
                    });
                }
            });
            Ok::<_, anyhow::Error>((local_port, handle))
        })?;

        Ok(Arc::new(MobilePreviewProxy {
            local_port,
            remote_port,
            task: Mutex::new(Some(handle)),
        }))
    }

    pub fn disconnect(&self) {
        self.connected.store(false, Ordering::Relaxed);
        if let Some(ctrl) = self.control.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = self.rt.spawn(async move {
                ctrl.close().await;
            });
        }
        self.conn.close();
        if let Some(client) = self.client.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = self.rt.spawn(async move {
                client.close().await;
            });
        }
    }
}

/// Active mobile pane session (Layer 2 zero-PC-impact reflow & Layer 3 opt-in Metal).
#[derive(uniffi::Object)]
pub struct MobilePaneSession {
    pane_id: u64,
    device_name: String,
    term: Arc<Mutex<HeadlessTerminal>>,
    tx: mpsc::UnboundedSender<ClientMsg>,
    alive: Arc<AtomicBool>,
    has_lease: AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl MobilePaneSession {
    pub fn pane_id(&self) -> u64 {
        self.pane_id
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn consumed_offset(&self) -> u64 {
        self.term
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .consumed_offset()
    }

    pub fn has_size_lease(&self) -> bool {
        self.has_lease.load(Ordering::Relaxed)
    }

    /// Pack the visible terminal grid into a compact 24-byte header + 16-byte/cell
    /// binary frame ready for direct Metal vertex/instance buffer upload.
    pub fn pack_metal_frame(&self) -> Vec<u8> {
        self.term
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pack_frame()
    }

    /// Produce Layer 2 zero-PC-interference logical-line reflowed rows.
    pub fn reflow_lines(&self, target_cols: u16, max_lines: u32) -> Vec<FfiReflowLine> {
        let guard = self.term.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .reflow_lines(usize::from(target_cols.max(20)), max_lines as usize)
            .into_iter()
            .enumerate()
            .map(|(i, line)| FfiReflowLine {
                text: line.text,
                is_wrapped_continuation: line.is_continuation,
                row_index: i as u32,
            })
            .collect()
    }

    pub fn scroll_display(&self, delta: i32) {
        self.term
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .scroll_display(delta);
    }

    pub fn send_bytes(&self, bytes: Vec<u8>) -> Result<(), MobileError> {
        if bytes.is_empty() {
            return Ok(());
        }
        {
            let mut guard = self.term.lock().unwrap_or_else(|e| e.into_inner());
            guard.observe_local_input(&bytes);
        }
        self.tx
            .send(ClientMsg::Input(bytes))
            .map_err(|_| MobileError::msg("pane stream closed"))
    }

    pub fn send_text(&self, text: String) -> Result<(), MobileError> {
        self.send_bytes(text.into_bytes())
    }

    pub fn send_quick_approval(
        &self,
        agent: Option<String>,
        action: FfiQuickApproval,
    ) -> Result<(), MobileError> {
        let parsed = agent.as_deref().and_then(parse_cli_agent);
        let bytes = approval_bytes(parsed, action);
        self.send_bytes(bytes)
    }

    /// Layer 3 explicit opt-in: acquire temporary PTY column lease for fullscreen TUIs.
    pub fn take_size_lease(&self, cols: u16, rows: u16) -> Result<(), MobileError> {
        let size = WinSize {
            cols: cols.max(20),
            rows: rows.max(6),
            cell_w: 0,
            cell_h: 0,
        };
        self.tx
            .send(ClientMsg::Lease(LeaseRequest::Take {
                size,
                by: self.device_name.clone(),
            }))
            .map_err(|_| MobileError::msg("pane stream closed"))?;
        self.has_lease.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Explicitly release temporary PTY size lease back to the desktop.
    pub fn release_size_lease(&self) -> Result<(), MobileError> {
        if self.has_lease.swap(false, Ordering::Relaxed) {
            let _ = self.tx.send(ClientMsg::Lease(LeaseRequest::Release));
        }
        Ok(())
    }

    pub fn close(&self) {
        let _ = self.release_size_lease();
        self.alive.store(false, Ordering::Relaxed);
        if let Some(handle) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            handle.abort();
        }
    }
}

/// Active Layer 1 coding-agent structured transcript stream.
#[derive(uniffi::Object)]
pub struct MobileTranscriptSession {
    rt: Arc<tokio::runtime::Runtime>,
    rx: Mutex<mpsc::UnboundedReceiver<FfiTranscriptBatch>>,
    cached: Arc<Mutex<Vec<FfiTranscriptEntry>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl MobileTranscriptSession {
    /// Wait up to `timeout_ms` for the next incremental transcript batch.
    pub fn poll_batch(&self, timeout_ms: u64) -> Option<FfiTranscriptBatch> {
        let mut rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
        self.rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(timeout_ms), rx.recv())
                .await
                .ok()
                .flatten()
        })
    }

    /// Return all cached transcript entries accumulated so far.
    pub fn cached_entries(&self) -> Vec<FfiTranscriptEntry> {
        self.cached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn close(&self) {
        if let Some(handle) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            handle.abort();
        }
    }
}

/// Loopback TCP proxy forwarding iOS `WKWebView` traffic into a desktop `PortTunnel`.
#[derive(uniffi::Object)]
pub struct MobilePreviewProxy {
    local_port: u16,
    remote_port: u16,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl MobilePreviewProxy {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub fn remote_port(&self) -> u16 {
        self.remote_port
    }

    pub fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    pub fn stop(&self) {
        if let Some(handle) = self.task.lock().unwrap_or_else(|e| e.into_inner()).take() {
            handle.abort();
        }
    }
}

fn load_or_create_device_key(dir: &Path) -> Result<SecretKey, MobileError> {
    let path = dir.join(DEVICE_KEY_FILE);
    if let Ok(bytes) = std::fs::read(&path)
        && bytes.len() == 32
    {
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&bytes);
        return Ok(SecretKey::from_bytes(&raw));
    }
    let key = SecretKey::generate();
    write_atomic_private(&path, &key.to_bytes())?;
    Ok(key)
}

fn load_paired_hosts(dir: &Path) -> Option<Vec<FfiPairedHost>> {
    let raw = std::fs::read_to_string(dir.join(PAIRED_HOSTS_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_paired_hosts(dir: &Path, hosts: &[FfiPairedHost]) -> Result<(), MobileError> {
    let json = serde_json::to_vec_pretty(hosts)
        .map_err(|e| MobileError::msg(format!("failed to serialize hosts: {e}")))?;
    write_atomic_private(&dir.join(PAIRED_HOSTS_FILE), &json)?;
    Ok(())
}

fn build_workspace_snapshot(
    host_name: &str,
    machine: Machine,
    agent_states: Vec<PaneAgentState>,
    ports_by_pane: HashMap<u64, Vec<u16>>,
    routes: Vec<anterm_core::daemon::control::RouteInfo>,
) -> FfiWorkspaceSnapshot {
    let panes_by_id: HashMap<u64, PaneRecord> =
        machine.panes.into_iter().map(|p| (p.id, p)).collect();
    let agents_by_id: HashMap<u64, PaneAgentState> =
        agent_states.into_iter().map(|a| (a.pane_id, a)).collect();

    let active_tab_id = machine.active_tab.map(|t| t.to_string());
    let mut tabs = Vec::with_capacity(machine.tabs.len());
    let mut waiting_panes = Vec::new();

    for tab in machine.tabs {
        let tab_id_str = tab.id.to_string();
        let tag_label = tab
            .tags
            .first()
            .and_then(|tid| machine.tags.get(*tid))
            .map(|def| def.name.clone());
        let pane_ids = tab.root.pane_ids();
        let mut pane_summaries = Vec::with_capacity(pane_ids.len());

        for pid in pane_ids {
            let rec = panes_by_id.get(&pid);
            let live_agent = agents_by_id.get(&pid);

            let agent_enum = live_agent
                .and_then(|a| a.agent)
                .or_else(|| rec.and_then(|r| r.agent.as_ref().map(|f| f.agent)));

            let agent_name = agent_enum.map(|a| format!("{a:?}").to_lowercase());
            let (agent_status, agent_detail, has_transcript) = match live_agent {
                Some(a) => {
                    let st = match a.state.status {
                        AgentStatus::Working => FfiAgentStatus::Working,
                        AgentStatus::Waiting => FfiAgentStatus::Waiting,
                        AgentStatus::Idle | AgentStatus::Done => FfiAgentStatus::Idle,
                    };
                    let has_tx = a.state.transcript_path.is_some() || agent_enum.is_some();
                    (st, a.state.message.clone(), has_tx)
                }
                None => (FfiAgentStatus::Idle, None, agent_enum.is_some()),
            };

            let cwd = live_agent
                .and_then(|a| a.state.cwd.as_ref().map(|p| p.display().to_string()))
                .or_else(|| rec.and_then(|r| r.cwd.clone()));

            let title = rec
                .and_then(|r| r.osc_title.clone().filter(|t| !t.trim().is_empty()))
                .or_else(|| rec.map(|r| r.title.clone()).filter(|t| !t.trim().is_empty()))
                .unwrap_or_else(|| format!("Pane #{pid}"));

            let summary = FfiPaneSummary {
                pane_id: pid,
                title,
                cwd,
                cols: 100,
                rows: 30,
                agent: agent_name,
                agent_status,
                agent_detail,
                has_transcript,
                listening_ports: ports_by_pane.get(&pid).cloned().unwrap_or_default(),
            };
            if summary.agent_status == FfiAgentStatus::Waiting {
                waiting_panes.push(summary.clone());
            }
            pane_summaries.push(summary);
        }

        let tab_title = tab
            .name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| pane_summaries.first().map(|p| p.title.clone()))
            .unwrap_or_else(|| "Terminal".to_string());
        let active_pane_id = pane_summaries.first().map(|p| p.pane_id);
        let desktop_focused = active_tab_id.as_deref() == Some(tab_id_str.as_str());

        tabs.push(FfiTabSummary {
            tab_id: tab_id_str,
            title: tab_title,
            tag: tag_label,
            desktop_focused,
            hibernated: tab.hibernated,
            active_pane_id,
            panes: pane_summaries,
        });
    }

    let ffi_routes = routes
        .into_iter()
        .map(|r| {
            let host_label = r.host().unwrap_or(&r.key).to_string();
            FfiRemoteRoute {
                key: r.key,
                kind: r.kind,
                connected: r.connected,
                host_label,
            }
        })
        .collect();

    FfiWorkspaceSnapshot {
        host_name: host_name.to_string(),
        desktop_active_tab_id: active_tab_id,
        tabs,
        waiting_panes,
        routes: ffi_routes,
    }
}

fn parse_porcelain_files(porcelain: &str) -> Vec<FfiGitChangedFile> {
    let mut out = Vec::new();
    for line in porcelain.lines() {
        if line.len() < 4 {
            continue;
        }
        let bytes = line.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        let path = line[3..].trim().to_string();
        if path.is_empty() {
            continue;
        }
        let status = format!("{x}{y}").trim().to_string();
        let staged = x != ' ' && x != '?';
        out.push(FfiGitChangedFile {
            path,
            status: if status.is_empty() { "M".into() } else { status },
            staged,
        });
    }
    out
}
