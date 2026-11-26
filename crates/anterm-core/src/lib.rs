//! Framework-free core for `anterm`: session daemon, PTY, native SSH engine,
//! remote host abstraction, git probes, config, and workspace state machine.

#[cfg(feature = "local-daemon")]
pub mod agent_hooks;
pub mod agent_prompt;
pub mod agent_transcript;
pub mod client;
pub mod clipboard;
pub mod codename;
pub mod config;
pub mod crash;
pub mod daemon;
pub mod git;
pub mod gitignore;
#[cfg(feature = "history-db")]
pub mod history;
pub mod host;
pub mod kitty_graphics;
pub mod logfile;
pub mod machine;
pub mod osc;
pub mod paste;
pub mod path_spelling;
pub mod proc;
pub mod rate_meter;
pub mod session;
pub mod shells;
pub mod ssh_config;
pub mod tab_view;
pub mod term_modes;
pub mod threads;
pub mod window_state;
pub mod worktree;

// Leaf protocol and shared domain modules re-exported from `anterm-protocol`.
pub use anterm_protocol::{cli_agent, gateway, keychain, shell_quote, ssh_profile, tag, transcript, util};
