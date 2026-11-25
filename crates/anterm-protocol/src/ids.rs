//! Stable cross-process identifiers shared between the wire protocol, daemon,
//! CLI, and GUI.

use crate::util::fnv1a64;

/// Deterministic identifier for a local or remote host.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct HostId(pub u64);

impl HostId {
    pub const LOCAL: HostId = HostId(0);

    pub fn from_connection_key(key: &str) -> HostId {
        let h = fnv1a64(key.as_bytes());
        HostId(if h == 0 { 1 } else { h })
    }

    pub fn is_local(self) -> bool {
        self == HostId::LOCAL
    }
}
