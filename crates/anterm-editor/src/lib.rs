//! Framework-free editor engines for `anterm`:
//!
//! - [`helix`] — Helix modal editing state machine, motions, text objects, and
//!   `:command` parser (takes framework-agnostic [`helix::KeyInput`])
//! - [`text`] — File decoding/encoding, BOM/line-ending handling, indentation
//!   detection, and `.editorconfig` resolution
//! - [`diff`] — Side-by-side and unified diff row pairing and selection logic
//! - [`lsp`] — LSP JSON-RPC framing ([`lsp::rpc`]) and language server
//!   registry + project root discovery ([`lsp::servers`])

pub mod diff;
pub mod helix;
pub mod lsp;
pub mod text;
