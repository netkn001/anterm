//! Managed host-side runtime controller for embedding [`GatewayServer`] inside
//! `anterm-app` (desktop GUI) and syncing live state with **Settings → Mobile**.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
#[cfg(target_os = "macos")]
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anterm_protocol::gateway::{MobileRole, PairingBundle};
use anterm_protocol::util::unix_now;

use crate::server::{GatewayServer, LocalDaemonBackend};
use crate::store::GatewayStore;

static GLOBAL_RUNTIME: OnceLock<GatewayRuntime> = OnceLock::new();
static LAST_DESKTOP_ACTIVITY_SECS: AtomicU64 = AtomicU64::new(0);

/// Record a user keyboard/pointer interaction on the desktop app so presence-aware
/// mobile push notifications can remain silent while the user is actively at the PC.
pub fn record_desktop_activity() {
    LAST_DESKTOP_ACTIVITY_SECS.store(unix_now(), Ordering::Relaxed);
}

/// Returns true when desktop user input was observed within the last `window_secs`.
pub fn is_desktop_recently_active(window_secs: u64) -> bool {
    let last = LAST_DESKTOP_ACTIVITY_SECS.load(Ordering::Relaxed);
    if last == 0 {
        return false;
    }
    unix_now().saturating_sub(last) <= window_secs
}

/// Snapshot of the embedded desktop gateway's live state for Settings UI display.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GatewayRuntimeStatus {
    pub running: bool,
    pub node_id: Option<String>,
    pub lan_addrs: Vec<String>,
    pub relay_url: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RunningFingerprint {
    lan_only: bool,
    relay_url: Option<String>,
    prevent_sleep: bool,
}

struct RunningState {
    server: Option<GatewayServer>,
    fingerprint: Option<RunningFingerprint>,
    lan_addrs: Vec<String>,
    node_id: Option<String>,
    last_error: Option<String>,
    sleep_guard: Option<SleepGuard>,
}

impl Default for RunningState {
    fn default() -> Self {
        Self {
            server: None,
            fingerprint: None,
            lan_addrs: Vec::new(),
            node_id: None,
            last_error: None,
            sleep_guard: None,
        }
    }
}

/// Cross-platform idle-sleep prevention guard (uses `caffeinate -i -w <pid>` on macOS).
struct SleepGuard {
    #[cfg(target_os = "macos")]
    child: Option<Child>,
}

impl SleepGuard {
    fn acquire() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            let pid = std::process::id().to_string();
            let child = Command::new("/usr/bin/caffeinate")
                .args(["-i", "-w", &pid])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            Some(Self { child: Some(child) })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Some(Self {})
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Process-wide controller that starts, reloads, and stops the host [`GatewayServer`]
/// on a dedicated background Tokio runtime without ever blocking GPUI.
pub struct GatewayRuntime {
    rt: OnceLock<anyhow::Result<tokio::runtime::Runtime, String>>,
    state: Mutex<RunningState>,
}

impl GatewayRuntime {
    pub fn global() -> &'static Self {
        GLOBAL_RUNTIME.get_or_init(|| Self {
            rt: OnceLock::new(),
            state: Mutex::new(RunningState::default()),
        })
    }

    fn runtime(&self) -> anyhow::Result<&tokio::runtime::Runtime> {
        let slot = self.rt.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .thread_name("anterm-gateway")
                .build()
                .map_err(|e| format!("failed to build gateway runtime: {e}"))
        });
        match slot {
            Ok(rt) => Ok(rt),
            Err(msg) => Err(anyhow::anyhow!("{msg}")),
        }
    }

    /// Return current live gateway status without touching the network.
    pub fn status(&self) -> GatewayRuntimeStatus {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        GatewayRuntimeStatus {
            running: guard.server.is_some(),
            node_id: guard.node_id.clone(),
            lan_addrs: guard.lan_addrs.clone(),
            relay_url: guard
                .fingerprint
                .as_ref()
                .and_then(|f| f.relay_url.clone()),
            last_error: guard.last_error.clone(),
        }
    }

    /// Non-blocking launch helper invoked by `anterm-app::run()`: starts the
    /// gateway automatically if `mobile_devices.json` has `enabled: true`.
    pub fn sync_default_background(&'static self) {
        let _ = std::thread::Builder::new()
            .name("anterm-gateway-boot".into())
            .spawn(move || {
                if let Ok(store) = GatewayStore::open_default()
                    && store.snapshot().enabled
                {
                    let status = self.sync_store(&store);
                    if let Some(err) = status.last_error {
                        log::warn!("mobile gateway auto-start failed: {err}");
                    } else if status.running {
                        log::info!(
                            "mobile gateway active (node={:?}, lan={:?})",
                            status.node_id,
                            status.lan_addrs
                        );
                    }
                }
            });
    }

    /// Synchronize the running [`GatewayServer`] with the persisted configuration
    /// inside `store`, starting, restarting, or stopping as required.
    pub fn sync_store(&self, store: &GatewayStore) -> GatewayRuntimeStatus {
        let snap = store.snapshot();
        let desired = RunningFingerprint {
            lan_only: snap.lan_only,
            relay_url: snap.relay_url.clone(),
            prevent_sleep: snap.prevent_sleep_while_active,
        };

        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !snap.enabled {
            if let Some(server) = guard.server.take()
                && let Ok(rt) = self.runtime()
            {
                rt.block_on(server.shutdown());
            }
            guard.fingerprint = None;
            guard.lan_addrs.clear();
            guard.node_id = None;
            guard.last_error = None;
            guard.sleep_guard = None;
            return GatewayRuntimeStatus::default();
        }

        // Update sleep guard even if network config didn't change.
        if guard.server.is_some()
            && guard.fingerprint.as_ref().is_some_and(|fp| {
                fp.lan_only == desired.lan_only && fp.relay_url == desired.relay_url
            })
        {
            if desired.prevent_sleep && guard.sleep_guard.is_none() {
                guard.sleep_guard = SleepGuard::acquire();
            } else if !desired.prevent_sleep {
                guard.sleep_guard = None;
            }
            guard.fingerprint = Some(desired);
            return GatewayRuntimeStatus {
                running: true,
                node_id: guard.node_id.clone(),
                lan_addrs: guard.lan_addrs.clone(),
                relay_url: snap.relay_url,
                last_error: None,
            };
        }

        // Need fresh start or restart.
        let rt = match self.runtime() {
            Ok(rt) => rt,
            Err(err) => {
                guard.last_error = Some(err.to_string());
                return GatewayRuntimeStatus {
                    running: false,
                    node_id: None,
                    lan_addrs: Vec::new(),
                    relay_url: snap.relay_url,
                    last_error: guard.last_error.clone(),
                };
            }
        };

        if let Some(old) = guard.server.take() {
            rt.block_on(old.shutdown());
        }

        match rt.block_on(GatewayServer::start(
            store.clone(),
            Arc::new(LocalDaemonBackend),
        )) {
            Ok(server) => {
                let node_id = server.node_id();
                let lan_addrs = server.lan_addrs();
                guard.node_id = Some(node_id.clone());
                guard.lan_addrs = lan_addrs.clone();
                guard.fingerprint = Some(desired.clone());
                guard.last_error = None;
                guard.sleep_guard = if desired.prevent_sleep {
                    SleepGuard::acquire()
                } else {
                    None
                };
                guard.server = Some(server);
                GatewayRuntimeStatus {
                    running: true,
                    node_id: Some(node_id),
                    lan_addrs,
                    relay_url: snap.relay_url,
                    last_error: None,
                }
            }
            Err(err) => {
                guard.last_error = Some(err.to_string());
                guard.sleep_guard = None;
                GatewayRuntimeStatus {
                    running: false,
                    node_id: None,
                    lan_addrs: Vec::new(),
                    relay_url: snap.relay_url,
                    last_error: guard.last_error.clone(),
                }
            }
        }
    }

    /// Ensure the gateway is running and mint a live single-use [`PairingBundle`]
    /// populated with reachable LAN socket addresses so phones on the same Wi-Fi
    /// pair directly without requiring a relay.
    pub fn issue_live_pairing_bundle(
        &self,
        store: &GatewayStore,
        role: MobileRole,
        ttl_secs: Option<u64>,
    ) -> anyhow::Result<PairingBundle> {
        if !store.snapshot().enabled {
            store.update_config(|cfg| cfg.enabled = true)?;
        }
        let status = self.sync_store(store);
        if !status.running {
            return Err(anyhow::anyhow!(
                "{}",
                status
                    .last_error
                    .unwrap_or_else(|| "mobile gateway is not running".into())
            ));
        }

        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(server) = guard.server.as_ref() else {
            return Err(anyhow::anyhow!("mobile gateway failed to start"));
        };
        server.issue_pairing_bundle(role, ttl_secs)
    }
}

/// Augment `iroh`'s reported direct socket addresses with the host's primary
/// outbound LAN IPv4 address when `iroh` only reported wildcard or loopback
/// addresses, deduplicated and ordered LAN-first.
pub(crate) fn discover_reachable_lan_addrs(reported: &[String]) -> Vec<String> {
    let mut parsed: Vec<SocketAddr> = reported
        .iter()
        .filter_map(|s| s.parse::<SocketAddr>().ok())
        .collect();

    let port = parsed
        .iter()
        .find(|a| a.port() != 0)
        .map(SocketAddr::port);

    if let (Some(port), Some(lan_ip)) = (port, probe_primary_lan_ipv4()) {
        let candidate = SocketAddr::new(IpAddr::V4(lan_ip), port);
        if !parsed.contains(&candidate) {
            parsed.insert(0, candidate);
        }
    }

    // Prefer concrete non-loopback IPv4 LAN addresses first, then others, keeping
    // loopback as fallback for same-machine simulator testing.
    parsed.sort_by_key(|a| match a.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => 0,
        IpAddr::V6(v6) if !v6.is_loopback() && !v6.is_unspecified() => 1,
        IpAddr::V4(v4) if v4.is_loopback() => 2,
        _ => 3,
    });
    parsed.dedup();
    parsed
        .into_iter()
        .filter(|a| !a.ip().is_unspecified())
        .map(|a| a.to_string())
        .collect()
}

fn probe_primary_lan_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // UDP connect sends zero packets on the wire; the OS routing table picks the
    // default outbound LAN interface IP.
    sock.connect((Ipv4Addr::new(192, 0, 2, 1), 80)).ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}
