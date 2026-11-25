//! Shared GPUI foundation crate for `anterm` desktop UI features.
//!
//! Owns every cross-cutting GUI primitive that multiple feature crates share:
//! theme, presets, bundled assets, GPUI action definitions, keymap engine,
//! reactive `ConfigStore`, host/git client caches, file index, Helix key bridge,
//! stateless daemon RPC helpers, and shared chrome/panel layout tokens.

extern crate gpui_kit as gpui;

pub mod actions;
pub mod app_hooks;
pub mod assets;
#[cfg(target_os = "windows")]
pub mod aumid;
pub mod config;
pub mod config_store;
pub mod confirm_answers;
pub mod daemon_rpc;
pub mod default_terminal;
pub mod dialog;
pub mod explorer_context_menu;
pub mod git;
pub mod helix_bridge;
pub mod host_ops;
pub mod host_registry;
pub mod intent;
pub mod keychain;
pub mod keymap;
pub mod kit;
pub mod notice;
pub mod palette_colors;
pub mod panel_tokens;
pub mod path_display;
pub mod perf;
pub mod platform;
pub mod prefill;
pub mod presets;
pub mod prompt;
pub mod rounding;
pub mod scrollbar;
pub mod search;
pub mod ssh_helpers;
pub mod theme;
#[cfg(target_os = "windows")]
pub mod toast_activator;
pub mod update;
pub mod window_chrome;
pub mod window_state;

pub use confirm_answers::confirm_answers;
