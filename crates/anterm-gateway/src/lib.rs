//! End-to-end encrypted `iroh` QUIC mobile gateway & client for `anterm`.
//!
//! - **Desktop / Server (`feature = "host"`)**: runs [`GatewayServer`] exposing
//!   multiplexed E2EE QUIC streams (`Pair`, `Control`, `Pane`, `PortTunnel`,
//!   `Transcript`) bridged to the local `anterm-server`.
//! - **Mobile (`default-features = false`)**: uses [`GatewayClient`] to scan QR
//!   pairing bundles, establish mutual-Ed25519 QUIC sessions over LAN / P2P /
//!   `iroh-relay`, and open low-latency streams without VPN/TUN privileges.

#[cfg(feature = "host")]
pub mod cli;
pub mod client;
#[cfg(feature = "host")]
pub mod runtime;
#[cfg(feature = "host")]
pub mod server;
pub mod store;
pub mod wire;

pub use anterm_protocol::gateway::{
    ControlStreamOpen, DEFAULT_PAIRING_TTL_SECS, GATEWAY_ALPN, MobileGatewayStoreData, MobileRole,
    PairReply, PairRequest, PairedDeviceRecord, PairingBundle, PaneStreamOpen, PortTunnelOpen,
    PortTunnelReply, PushEndpoint, StreamKind, TranscriptStreamOpen,
};
pub use client::{GatewayClient, GatewayConnection, GatewayControlClient};
#[cfg(feature = "host")]
pub use runtime::{
    GatewayRuntime, GatewayRuntimeStatus, is_desktop_recently_active, record_desktop_activity,
};
#[cfg(feature = "host")]
pub use server::{GatewayBackend, GatewayServer, LocalDaemonBackend};
pub use store::{GatewayStore, qr_matrix, render_qr_ascii};

#[cfg(all(test, feature = "host"))]
mod tests;
