//! Leaf wire contracts for `anterm-gateway`: ALPN, QUIC stream multiplex tags,
//! QR pairing bundle URI encoding/decoding, paired device permissions, and
//! stream open headers.

use std::fmt;

use serde::{Deserialize, Serialize};

/// QUIC ALPN identifier for `anterm-gateway` connections over `iroh`.
pub const GATEWAY_ALPN: &[u8] = b"anterm/gw/v1";

/// URI prefix encoded into pairing QR codes shown on desktop or `anterm pair`.
pub const PAIRING_URI_PREFIX: &str = "anterm://pair/v1";

/// Default lifetime of a single-use pairing QR code (5 minutes).
pub const DEFAULT_PAIRING_TTL_SECS: u64 = 5 * 60;

/// Maximum allowed size for an uploaded mobile attachment (16 MiB).
pub const MAX_MOBILE_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

/// First byte written by the client on each QUIC bidirectional stream to select
/// the gateway sub-protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StreamKind {
    /// Initial single-use QR pairing handshake (allowed for unpaired peers).
    Pair = 0x01,
    /// Bridged control RPC + real-time `LayoutDelta` / `AgentStatus` stream.
    Control = 0x02,
    /// Individual pane stream (`ObserveSince` + optional `SizeLease` + zstd).
    Pane = 0x03,
    /// Localhost dev-server TCP tunnel for in-app mobile Web Preview.
    PortTunnel = 0x04,
    /// Structured coding-agent transcript stream (zero PTY resize impact).
    Transcript = 0x05,
}

impl StreamKind {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Pair),
            0x02 => Some(Self::Control),
            0x03 => Some(Self::Pane),
            0x04 => Some(Self::PortTunnel),
            0x05 => Some(Self::Transcript),
            _ => None,
        }
    }

    pub fn as_byte(self) -> u8 {
        self as u8
    }
}

/// Granular authorization tier granted to a paired mobile device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileRole {
    /// Read-only visibility into workspace tree, transcripts, and pane output.
    ObserveOnly,
    /// Default: read + send input/approvals + opt-in size lease + diff/preview.
    #[default]
    Interactive,
    /// Interactive plus file writes, temp uploads, tab/pane creation & closing.
    Full,
}

impl MobileRole {
    pub fn can_send_input(self) -> bool {
        matches!(self, Self::Interactive | Self::Full)
    }

    pub fn can_take_lease(self) -> bool {
        matches!(self, Self::Interactive | Self::Full)
    }

    pub fn can_tunnel_ports(self) -> bool {
        matches!(self, Self::Interactive | Self::Full)
    }

    pub fn can_upload_attachments(self) -> bool {
        matches!(self, Self::Interactive | Self::Full)
    }

    pub fn can_write_files(self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn can_manage_tabs(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Ephemeral pairing payload serialized into a QR code URI.
///
/// Format:
/// `anterm://pair/v1?n=<node_id>&s=<secret_b64>&r=<relay>&a=<lan_csv>&h=<host>&exp=<unix>`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingBundle {
    /// Desktop `iroh` EndpointId / NodeId (public Ed25519 identity).
    pub node_id: String,
    /// Base64url-encoded 256-bit ephemeral pairing secret.
    pub secret_b64: String,
    /// Optional `iroh-relay` HTTPS URL configured by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    /// Optional LAN `ip:port` socket addresses for zero-relay local fast path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lan_addrs: Vec<String>,
    /// Human-readable desktop hostname ("Futsing's MacBook Pro").
    pub host_name: String,
    /// Expiration timestamp in Unix seconds.
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingUriError {
    InvalidScheme,
    MissingField(&'static str),
    InvalidField(&'static str),
}

impl fmt::Display for PairingUriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidScheme => write!(f, "URI must start with {PAIRING_URI_PREFIX}?"),
            Self::MissingField(k) => write!(f, "missing required pairing field '{k}'"),
            Self::InvalidField(k) => write!(f, "invalid pairing field '{k}'"),
        }
    }
}

impl std::error::Error for PairingUriError {}

impl PairingBundle {
    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix > self.expires_at
    }

    pub fn to_uri(&self) -> String {
        let mut out = format!(
            "{PAIRING_URI_PREFIX}?n={}&s={}&h={}&exp={}",
            pct_encode(&self.node_id),
            pct_encode(&self.secret_b64),
            pct_encode(&self.host_name),
            self.expires_at
        );
        if let Some(relay) = self.relay_url.as_deref().filter(|r| !r.trim().is_empty()) {
            out.push_str("&r=");
            out.push_str(&pct_encode(relay.trim()));
        }
        if !self.lan_addrs.is_empty() {
            let joined = self.lan_addrs.join(",");
            out.push_str("&a=");
            out.push_str(&pct_encode(&joined));
        }
        out
    }

    pub fn from_uri(uri: &str) -> Result<Self, PairingUriError> {
        let trimmed = uri.trim();
        let query = trimmed
            .strip_prefix(PAIRING_URI_PREFIX)
            .and_then(|rest| rest.strip_prefix('?'))
            .ok_or(PairingUriError::InvalidScheme)?;

        let mut node_id = None;
        let mut secret_b64 = None;
        let mut relay_url = None;
        let mut lan_addrs = Vec::new();
        let mut host_name = None;
        let mut expires_at = None;

        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let decoded = pct_decode(v).ok_or(PairingUriError::InvalidField(match k {
                "n" => "n",
                "s" => "s",
                "r" => "r",
                "a" => "a",
                "h" => "h",
                "exp" => "exp",
                _ => "query",
            }))?;
            match k {
                "n" if !decoded.is_empty() => node_id = Some(decoded),
                "s" if !decoded.is_empty() => secret_b64 = Some(decoded),
                "r" if !decoded.is_empty() => relay_url = Some(decoded),
                "a" if !decoded.is_empty() => {
                    lan_addrs = decoded
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "h" if !decoded.is_empty() => host_name = Some(decoded),
                "exp" => {
                    let parsed = decoded
                        .parse::<u64>()
                        .map_err(|_| PairingUriError::InvalidField("exp"))?;
                    expires_at = Some(parsed);
                }
                _ => {}
            }
        }

        Ok(Self {
            node_id: node_id.ok_or(PairingUriError::MissingField("n"))?,
            secret_b64: secret_b64.ok_or(PairingUriError::MissingField("s"))?,
            relay_url,
            lan_addrs,
            host_name: host_name.ok_or(PairingUriError::MissingField("h"))?,
            expires_at: expires_at.ok_or(PairingUriError::MissingField("exp"))?,
        })
    }
}

/// Client request sent on [`StreamKind::Pair`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub secret_b64: String,
    pub device_name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub requested_role: MobileRole,
}

/// Host reply sent on [`StreamKind::Pair`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_os: Option<String>,
    #[serde(default)]
    pub granted_role: MobileRole,
    #[serde(default)]
    pub protocol_version: u32,
}

/// Opening metadata sent on [`StreamKind::Control`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlStreamOpen {
    pub device_name: String,
    #[serde(default)]
    pub app_version: String,
    /// Optional remote machine route key (e.g. `me@prod:22`) when inspecting a
    /// routed SSH/WSL host through the desktop daemon; `None` means local host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_key: Option<String>,
}

/// Opening metadata sent on [`StreamKind::Pane`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneStreamOpen {
    pub pane_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_key: Option<String>,
    /// Resume from this monotonic byte offset in `ReplayRing` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_offset: Option<u64>,
    /// Request zstd frame compression on downstream pane frames.
    #[serde(default)]
    pub compress_zstd: bool,
}

/// Opening metadata sent on [`StreamKind::PortTunnel`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortTunnelOpen {
    /// Localhost port on the host machine (must match an active listening port).
    pub target_port: u16,
    /// Optional pane that owns the process listening on `target_port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<u64>,
}

/// Reply sent on [`StreamKind::PortTunnel`] before raw TCP bytes flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortTunnelReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Opening metadata sent on [`StreamKind::Transcript`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptStreamOpen {
    pub pane_id: u64,
    /// Last sequence number already received by the client (0 for full history).
    #[serde(default)]
    pub since_seq: u64,
}

/// Push notification registration stored per paired mobile device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushEndpoint {
    /// `"apns"` or `"fcm"`.
    pub provider: String,
    /// Device push token.
    pub token: String,
    /// Base64-encoded 32-byte symmetric key used to encrypt push payload before
    /// sending to the stateless push relay.
    pub e2ee_key_b64: String,
}

/// Persisted authorization record for one paired mobile device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDeviceRecord {
    pub node_id: String,
    pub name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub role: MobileRole,
    pub paired_at: u64,
    #[serde(default)]
    pub last_seen_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_endpoint: Option<PushEndpoint>,
}

/// Gateway configuration stored in `<config_dir>/mobile_devices.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileGatewayStoreData {
    #[serde(default)]
    pub enabled: bool,
    /// Custom `iroh-relay` HTTPS URL (`None` = default or LAN-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    /// When true, restrict gateway to LAN / direct addresses only (no relay).
    #[serde(default)]
    pub lan_only: bool,
    /// Keep the desktop awake while an active agent or mobile session runs.
    #[serde(default = "default_true")]
    pub prevent_sleep_while_active: bool,
    /// Suppress mobile push notifications while the desktop user is actively
    /// interacting with the PC window.
    #[serde(default = "default_true")]
    pub silence_push_when_desktop_active: bool,
    #[serde(default)]
    pub devices: Vec<PairedDeviceRecord>,
}

fn default_true() -> bool {
    true
}

impl Default for MobileGatewayStoreData {
    fn default() -> Self {
        Self {
            enabled: false,
            relay_url: None,
            lan_only: false,
            prevent_sleep_while_active: true,
            silence_push_when_desktop_active: true,
            devices: Vec::new(),
        }
    }
}

fn pct_encode(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

fn pct_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_kind_round_trips_all_variants() {
        for kind in [
            StreamKind::Pair,
            StreamKind::Control,
            StreamKind::Pane,
            StreamKind::PortTunnel,
            StreamKind::Transcript,
        ] {
            assert_eq!(StreamKind::from_byte(kind.as_byte()), Some(kind));
        }
        assert_eq!(StreamKind::from_byte(0x00), None);
        assert_eq!(StreamKind::from_byte(0x99), None);
    }

    #[test]
    fn pairing_bundle_uri_round_trips_unicode_and_urls() {
        let bundle = PairingBundle {
            node_id: "abc123def456".into(),
            secret_b64: "s3cr3t-_token".into(),
            relay_url: Some("https://relay.anterm.dev:443/iroh?v=1".into()),
            lan_addrs: vec!["192.168.1.42:49200".into(), "[fe80::1]:49200".into()],
            host_name: "Futsing 的 MacBook Pro".into(),
            expires_at: 1_800_000_000,
        };
        let uri = bundle.to_uri();
        assert!(uri.starts_with("anterm://pair/v1?"));
        let parsed = PairingBundle::from_uri(&uri).unwrap();
        assert_eq!(parsed, bundle);
        assert!(!parsed.is_expired(1_799_999_999));
        assert!(parsed.is_expired(1_800_000_001));
    }

    #[test]
    fn pairing_bundle_rejects_bad_scheme_or_missing_fields() {
        assert_eq!(
            PairingBundle::from_uri("https://example.com"),
            Err(PairingUriError::InvalidScheme)
        );
        assert_eq!(
            PairingBundle::from_uri("anterm://pair/v1?n=abc&h=mac&exp=123"),
            Err(PairingUriError::MissingField("s"))
        );
        assert_eq!(
            PairingBundle::from_uri("anterm://pair/v1?n=abc&s=xyz&h=mac&exp=notanumber"),
            Err(PairingUriError::InvalidField("exp"))
        );
    }

    #[test]
    fn mobile_role_capabilities_match_permission_tiers() {
        assert!(!MobileRole::ObserveOnly.can_send_input());
        assert!(!MobileRole::ObserveOnly.can_take_lease());
        assert!(!MobileRole::ObserveOnly.can_tunnel_ports());

        assert!(MobileRole::Interactive.can_send_input());
        assert!(MobileRole::Interactive.can_take_lease());
        assert!(MobileRole::Interactive.can_tunnel_ports());
        assert!(!MobileRole::Interactive.can_write_files());
        assert!(!MobileRole::Interactive.can_manage_tabs());

        assert!(MobileRole::Full.can_write_files());
        assert!(MobileRole::Full.can_manage_tabs());
    }
}
