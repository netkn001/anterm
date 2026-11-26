//! Lightweight wire protocol, framing, updater contracts, and leaf value types
//! shared across all `anterm` workspace crates.
//!
//! This crate sits at the bottom of the workspace dependency DAG with zero GUI,
//! SSH, PTY, SQLite, or async-runtime dependencies.

pub mod cli_agent;
pub mod diff;
pub mod gateway;
pub mod ids;
pub mod keychain;
pub mod pane;
pub mod shell_quote;
pub mod ssh_profile;
pub mod tag;
pub mod transcript;
pub mod updater;
pub mod util;
