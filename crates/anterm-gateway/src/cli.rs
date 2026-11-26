//! Blocking CLI entrypoints for running the mobile gateway and terminal QR
//! pairing flow from `anterm-server`.

use std::sync::Arc;

use anterm_protocol::gateway::MobileRole;

use crate::server::{GatewayServer, LocalDaemonBackend};
use crate::store::{GatewayStore, render_qr_ascii};

/// Options for `anterm-server pair`.
#[derive(Clone, Debug)]
pub struct PairCliOptions {
    pub role: MobileRole,
    pub ttl_secs: u64,
    pub lan_only: bool,
    pub relay_url: Option<String>,
}

/// Parse a CLI role string (`interactive`, `observe_only`, `full`).
pub fn parse_role(raw: &str) -> anyhow::Result<MobileRole> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "interactive" | "default" => Ok(MobileRole::Interactive),
        "observe" | "observe_only" | "observe-only" | "readonly" | "read-only" => {
            Ok(MobileRole::ObserveOnly)
        }
        "full" | "admin" => Ok(MobileRole::Full),
        other => anyhow::bail!(
            "unknown role '{other}' (expected interactive, observe_only, or full)"
        ),
    }
}

/// Start the mobile gateway, print a single-use pairing QR code to stdout, and
/// serve incoming mobile QUIC connections until interrupted.
pub fn run_pair_cli(opts: PairCliOptions) -> anyhow::Result<()> {
    if let Err(err) = anterm_core::daemon::spawn::ensure_running() {
        eprintln!("anterm-server: warning: could not start local daemon ({err})");
    }

    let store = GatewayStore::open_default()?;
    store.update_config(|cfg| {
        cfg.enabled = true;
        if opts.lan_only {
            cfg.lan_only = true;
        }
        if let Some(ref relay) = opts.relay_url {
            cfg.relay_url = Some(relay.clone());
        }
    })?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let server = GatewayServer::start(store, Arc::new(LocalDaemonBackend)).await?;
        let bundle = server.issue_pairing_bundle(opts.role, Some(opts.ttl_secs))?;
        let uri = bundle.to_uri();
        let ascii = render_qr_ascii(&uri)?;

        println!("\nAnterm Mobile Pairing (role: {:?}, expires in {}s)", opts.role, opts.ttl_secs);
        println!("Host: {} ({})\n", bundle.host_name, bundle.node_id);
        println!("{ascii}");
        println!("Pairing URI:\n{uri}\n");
        println!("Scan the QR code in the Anterm iOS app. Press Ctrl+C to stop gateway...");

        tokio::signal::ctrl_c().await?;
        server.shutdown().await;
        Ok(())
    })
}

/// Run the headless mobile gateway continuously until interrupted.
pub fn run_gateway_cli(lan_only: bool, relay_url: Option<String>) -> anyhow::Result<()> {
    if let Err(err) = anterm_core::daemon::spawn::ensure_running() {
        eprintln!("anterm-server: warning: could not start local daemon ({err})");
    }

    let store = GatewayStore::open_default()?;
    store.update_config(|cfg| {
        cfg.enabled = true;
        if lan_only {
            cfg.lan_only = true;
        }
        if let Some(ref relay) = relay_url {
            cfg.relay_url = Some(relay.clone());
        }
    })?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let server = GatewayServer::start(store, Arc::new(LocalDaemonBackend)).await?;
        eprintln!("anterm-server: mobile gateway listening as {}", server.node_id());
        tokio::signal::ctrl_c().await?;
        server.shutdown().await;
        Ok(())
    })
}
