//! Persistent gateway identity, paired device authorization store
//! (`mobile_devices.json`), single-use QR pairing sessions, and QR rendering.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anterm_protocol::gateway::{
    DEFAULT_PAIRING_TTL_SECS, MobileGatewayStoreData, MobileRole, PairedDeviceRecord,
    PairingBundle, PushEndpoint,
};
use anterm_protocol::util::{unix_now, write_atomic_private};
use base64::Engine as _;
use iroh::SecretKey;
use qrcode::QrCode;

pub const MOBILE_DEVICES_FILE: &str = "mobile_devices.json";
pub const MOBILE_KEY_FILE: &str = "mobile_node.key";

#[derive(Clone, Debug)]
struct ActivePairing {
    secret_b64: String,
    expires_at: u64,
    granted_role: MobileRole,
}

/// Thread-safe persistence & authorization store for a host's mobile gateway.
#[derive(Clone)]
pub struct GatewayStore {
    inner: Arc<Mutex<StoreInner>>,
}

struct StoreInner {
    dir: PathBuf,
    data: MobileGatewayStoreData,
    active_pairings: HashMap<String, ActivePairing>,
}

impl GatewayStore {
    /// Open or initialize the store inside `config_dir`.
    pub fn open(config_dir: PathBuf) -> Self {
        let data = load_store_data(&config_dir).unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(StoreInner {
                dir: config_dir,
                data,
                active_pairings: HashMap::new(),
            })),
        }
    }

    /// Open using `anterm-core`'s active config directory.
    pub fn open_default() -> anyhow::Result<Self> {
        let dir = anterm_core::config::config_dir_path()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve anterm config directory"))?;
        Ok(Self::open(dir))
    }

    /// Load or generate and persist the host's Ed25519 `iroh::SecretKey`
    /// (`0600` private file inside `config_dir`).
    pub fn load_or_create_secret_key(&self) -> anyhow::Result<SecretKey> {
        let dir = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.dir.clone()
        };
        load_or_create_key_in(&dir)
    }

    pub fn snapshot(&self) -> MobileGatewayStoreData {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .data
            .clone()
    }

    pub fn update_config(
        &self,
        f: impl FnOnce(&mut MobileGatewayStoreData),
    ) -> anyhow::Result<MobileGatewayStoreData> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard.data);
        save_store_data(&guard.dir, &guard.data)?;
        Ok(guard.data.clone())
    }

    /// Mint a fresh single-use pairing bundle (default 5-minute TTL).
    pub fn issue_pairing_bundle(
        &self,
        node_id: String,
        lan_addrs: Vec<String>,
        host_name: String,
        role: MobileRole,
        ttl_secs: Option<u64>,
    ) -> anyhow::Result<PairingBundle> {
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw)
            .map_err(|e| anyhow::anyhow!("OS RNG unavailable for pairing secret: {e}"))?;
        let secret_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let now = unix_now();
        let expires_at = now.saturating_add(ttl_secs.unwrap_or(DEFAULT_PAIRING_TTL_SECS));

        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.active_pairings.retain(|_, p| p.expires_at >= now);
        guard.active_pairings.insert(
            secret_b64.clone(),
            ActivePairing {
                secret_b64: secret_b64.clone(),
                expires_at,
                granted_role: role,
            },
        );
        let relay_url = if guard.data.lan_only {
            None
        } else {
            guard.data.relay_url.clone()
        };

        Ok(PairingBundle {
            node_id,
            secret_b64,
            relay_url,
            lan_addrs,
            host_name,
            expires_at,
        })
    }

    /// Validate a single-use pairing secret and persist the device if valid.
    pub fn complete_pairing(
        &self,
        device_node_id: &str,
        secret_b64: &str,
        device_name: &str,
        platform: &str,
    ) -> Result<MobileRole, String> {
        let now = unix_now();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.active_pairings.retain(|_, p| p.expires_at >= now);

        let matched_key = guard
            .active_pairings
            .iter()
            .find(|(_, p)| constant_time_eq(p.secret_b64.as_bytes(), secret_b64.as_bytes()))
            .map(|(k, _)| k.clone());

        let Some(key) = matched_key else {
            return Err("pairing code is invalid or expired".into());
        };
        let Some(active) = guard.active_pairings.remove(&key) else {
            return Err("pairing code was already used".into());
        };
        if now > active.expires_at {
            return Err("pairing code has expired".into());
        }

        let name = if device_name.trim().is_empty() {
            "Mobile Device".to_string()
        } else {
            device_name.trim().chars().take(64).collect()
        };

        if let Some(existing) = guard
            .data
            .devices
            .iter_mut()
            .find(|d| d.node_id == device_node_id)
        {
            existing.name = name;
            existing.platform = platform.to_string();
            existing.role = active.granted_role;
            existing.last_seen_at = now;
        } else {
            guard.data.devices.push(PairedDeviceRecord {
                node_id: device_node_id.to_string(),
                name,
                platform: platform.to_string(),
                role: active.granted_role,
                paired_at: now,
                last_seen_at: now,
                push_endpoint: None,
            });
        }

        if let Err(e) = save_store_data(&guard.dir, &guard.data) {
            return Err(format!("could not save paired device: {e}"));
        }
        Ok(active.granted_role)
    }

    /// Look up an authorized peer's role and device name, updating `last_seen_at`.
    pub fn authorize_peer(&self, device_node_id: &str) -> Option<(MobileRole, String)> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let dev = guard
            .data
            .devices
            .iter_mut()
            .find(|d| d.node_id == device_node_id)?;
        dev.last_seen_at = unix_now();
        let res = (dev.role, dev.name.clone());
        let _ = save_store_data(&guard.dir, &guard.data);
        Some(res)
    }

    pub fn set_device_role(&self, device_node_id: &str, role: MobileRole) -> anyhow::Result<bool> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(dev) = guard
            .data
            .devices
            .iter_mut()
            .find(|d| d.node_id == device_node_id)
        else {
            return Ok(false);
        };
        dev.role = role;
        save_store_data(&guard.dir, &guard.data)?;
        Ok(true)
    }

    pub fn rename_device(&self, device_node_id: &str, new_name: &str) -> anyhow::Result<bool> {
        let cleaned: String = new_name.trim().chars().take(64).collect();
        if cleaned.is_empty() {
            anyhow::bail!("device name cannot be empty");
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(dev) = guard
            .data
            .devices
            .iter_mut()
            .find(|d| d.node_id == device_node_id)
        else {
            return Ok(false);
        };
        dev.name = cleaned;
        save_store_data(&guard.dir, &guard.data)?;
        Ok(true)
    }

    pub fn set_push_endpoint(
        &self,
        device_node_id: &str,
        endpoint: Option<PushEndpoint>,
    ) -> anyhow::Result<bool> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(dev) = guard
            .data
            .devices
            .iter_mut()
            .find(|d| d.node_id == device_node_id)
        else {
            return Ok(false);
        };
        dev.push_endpoint = endpoint;
        save_store_data(&guard.dir, &guard.data)?;
        Ok(true)
    }

    pub fn revoke_device(&self, device_node_id: &str) -> anyhow::Result<bool> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.data.devices.len();
        guard.data.devices.retain(|d| d.node_id != device_node_id);
        let removed = guard.data.devices.len() != before;
        if removed {
            save_store_data(&guard.dir, &guard.data)?;
        }
        Ok(removed)
    }
}

fn load_store_data(dir: &Path) -> Option<MobileGatewayStoreData> {
    let raw = std::fs::read_to_string(dir.join(MOBILE_DEVICES_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_store_data(dir: &Path, data: &MobileGatewayStoreData) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(data)?;
    write_atomic_private(&dir.join(MOBILE_DEVICES_FILE), &bytes)?;
    Ok(())
}

pub fn load_or_create_key_in(dir: &Path) -> anyhow::Result<SecretKey> {
    let key_path = dir.join(MOBILE_KEY_FILE);
    if let Ok(raw) = std::fs::read(&key_path)
        && raw.len() == 32
    {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&raw);
        return Ok(SecretKey::from_bytes(&arr));
    }
    let key = SecretKey::generate();
    write_atomic_private(&key_path, &key.to_bytes())?;
    Ok(key)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Encode `payload` into a boolean QR module matrix (`width × width`, true = dark).
/// Used directly by GPUI Settings UI to render crisp vector QR squares.
pub fn qr_matrix(payload: &str) -> anyhow::Result<(usize, Vec<bool>)> {
    let code = QrCode::new(payload.as_bytes())
        .map_err(|e| anyhow::anyhow!("QR encoding failed: {e}"))?;
    let width = code.width();
    let modules = code
        .to_colors()
        .into_iter()
        .map(|c| matches!(c, qrcode::Color::Dark))
        .collect();
    Ok((width, modules))
}

/// Render `payload` as compact UTF-8 half-block QR art for CLI (`anterm pair`).
pub fn render_qr_ascii(payload: &str) -> anyhow::Result<String> {
    let (w, modules) = qr_matrix(payload)?;
    let quiet = 2usize;
    let total = w + quiet * 2;
    let is_dark = |x: usize, y: usize| -> bool {
        if x < quiet || y < quiet || x >= quiet + w || y >= quiet + w {
            return false;
        }
        modules[(y - quiet) * w + (x - quiet)]
    };

    let mut out = String::new();
    let mut y = 0;
    while y < total {
        for x in 0..total {
            let top = is_dark(x, y);
            let bot = if y + 1 < total {
                is_dark(x, y + 1)
            } else {
                false
            };
            let ch = match (top, bot) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            };
            out.push(ch);
        }
        out.push('\n');
        y += 2;
    }
    Ok(out)
}
