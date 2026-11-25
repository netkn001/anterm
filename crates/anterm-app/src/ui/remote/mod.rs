//! Remote connectivity, daemon tree sync, and re-exports from `anterm-ui-kit`
//! and `anterm-remote-ui`.

pub(crate) use anterm_remote_ui::{forwards, ssh_prompt};
pub(crate) use anterm_ui_kit::{host_ops, host_registry};

pub(crate) mod local_link;
pub(crate) mod machine_mirror;
pub(crate) mod ssh_connect;
pub(crate) mod tree_sync;
