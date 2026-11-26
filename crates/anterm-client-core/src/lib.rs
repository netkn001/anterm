//! Framework-free terminal client core shared across desktop (`anterm-terminal`)
//! and mobile (`anterm-mobile-ffi`).
//!
//! Zero GPUI, zero PTY, zero OS-keychain dependencies — compiles cleanly on
//! macOS, Linux, Windows, iOS (`aarch64-apple-ios`), and Android.

pub mod composer;
pub mod frame;
pub mod headless;
pub mod mirror;
pub mod predictive_echo;
pub mod reflow;

pub use headless::{HeadlessTerminal, TerminalEventSummary};
pub use mirror::{EventReceiver, TermStateMirror};
pub use predictive_echo::PredictiveEcho;
pub use reflow::ReflowedLine;
