mod client;
mod codec;
mod wire;
#[cfg(test)]
mod tests;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub use client::{
    CLOSE_GRACE, ControlClient, ControlResponse, EventSink, KEEPALIVE_DEAD_AFTER,
    KEEPALIVE_IDLE_BEFORE_PING, KEEPALIVE_PING_INTERVAL, LinkShutdown,
};
pub use codec::{ControlClientMsg, ControlServerMsg};
pub use wire::{
    ControlEvent, ControlHello, ControlHelloOk, ControlReply, ControlRequest, EventObserver,
    PaneAgentState, ReplyOk, RouteInfo, ServerStatus, WireError, WireErrorKind, observe_event,
    set_event_observer,
};

/// The control dialect this build speaks. A peer that answers the hello with a
/// different number is refused outright, and the number is half of the remote
/// server's filename (`anterm-server-c{control}p{protocol}`), so moving it also
/// makes a client install the server that matches instead of trusting the one
/// already sitting at that path.
///
/// Move it whenever a variant is added to or removed from [`ControlRequest`],
/// [`ReplyOk`] or [`ControlEvent`]. The [`feature`] strings cover only what a
/// peer can safely ignore — a field added to a message it already decodes. A
/// request it has never heard of is not ignorable: the frame fails to decode
/// and the read loop that failed on it takes the whole link down, which is how
/// a remote workspace opens with no tabs and a git detail pane never fills.
///
/// v6 pays off the drift since v5, which was most of the dialect: the machine
/// tree replaced `WorkspaceList`/`Get`/`Put`/`Delete` with `WorkspaceTree`,
/// `MachineGet` and the tab/pane verbs, `GitStream` arrived with its chunk and
/// end events, and `ReplyOk::Attached` and `FileMeta` went away. All of it
/// shipped against a number that never moved, so every one of those servers
/// still answers the hello and then drops the link on the first call.
///
/// v7 changes no message at all. It is here so that the handling of a dialect
/// refusal — the parked strip, its Update Server button, and the installer
/// refusing to kill a server it has nothing to replace with — can be exercised
/// against the v6 servers already deployed. A number is the only way to reach
/// that path, and a mismatch nobody can reproduce is a mismatch nobody can fix.
///
/// v8 added the project verbs; v9 takes them back out. The dialect this build
/// speaks is v7's again, message for message, but the number does not go back
/// to 7 with it: v8 is deployed, and a number that moves backwards stops being
/// an identity and becomes a coincidence. A 7 on the wire would then mean
/// either "before projects" or "after them" depending on which build put it
/// there, and the handshake — which has nothing but the number — cannot tell
/// the two apart. A v8 peer meeting this one must be turned away, and only a
/// number it has never seen does that.
///
/// v10, like v7, changes no message. It moves because the number is also the
/// only thing that makes a client replace the server on a remote host: a
/// same-dialect install trusts whatever binary already sits at
/// `anterm-server-c9p6`, and "Update Server" only restarts that same file. So
/// daemon-side fixes shipped under v9 — modes restored on re-attach (#828),
/// the replay ring and clipboard sniffer that never cut an escape sequence in
/// half (#857) — would have stayed on this machine and never reached a remote
/// one. A new filename is what gets them uploaded, at the price of one Update
/// Server per host, which ends the sessions on it.
///
/// v11 replaces the sidebar's hand-made groups. A tab now names a pinned group
/// by id (`TabSetGroup` carries a `GroupId`, not a string), the groups
/// themselves live on the workspace and move as `WorkspaceSetGroups` and
/// `LayoutDelta::GroupsChanged`, and a v10 peer can decode none of that.
///
/// v12 adds `AgentSessions`: the search's Sessions tab in a remote workspace
/// lists the coding-agent sessions of the machine the workspace is on.
///
/// v13 flattens the machine tree onto a single session per machine and removes
/// the workspace verbs and attachment/preemption messages.
///
/// v14 replaces sidebar groups (`TabSetGroup`, `SetGroups`,
/// `LayoutDelta::TabRegrouped`, `LayoutDelta::GroupsChanged`) with flat tab
/// tags (`TabSetTags`, `SetTags`, `LayoutDelta::TabTagsChanged`,
/// `LayoutDelta::TagsChanged`).
///
/// v15 removes `AgentSessions` (scanning coding agents' past sessions from
/// disk/SQLite); users resume sessions via each agent's own `/resume`.
pub const CONTROL_VERSION: u32 = 15;

const DIALECT_MARKER: &str = "speaks control v";

fn dialect_refusal(peer_build: &str, peer: u32, ours: u32) -> String {
    format!("control peer (build {peer_build}) {DIALECT_MARKER}{peer}, this build speaks v{ours}")
}

/// Whether this is a refusal over the control dialect — the whole shape of one,
/// not just the marker.
///
/// Every reader of a `true` here goes on to restate the refusal with
/// [`parse_dialect_refusal`], and one of them parks a link in a state only a
/// person can leave. Two predicates for one question is how those two come
/// apart: a message carrying the marker in some other shape would be parked on
/// and then shown as the raw protocol wording it was supposed to replace.
pub fn is_dialect_refusal(message: &str) -> bool {
    parse_dialect_refusal(message).is_some()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialectRefusal {
    pub peer_build: String,
    pub peer: u32,
    pub ours: u32,
}

pub fn parse_dialect_refusal(message: &str) -> Option<DialectRefusal> {
    const OPEN: &str = "control peer (build ";
    const TAIL: &str = ", this build speaks v";

    let rest = &message[message.find(OPEN)? + OPEN.len()..];
    let (peer_build, rest) = rest.split_once(") ")?;
    let rest = rest.strip_prefix(DIALECT_MARKER)?;
    let (peer, rest) = rest.split_once(TAIL)?;
    let ours: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some(DialectRefusal {
        peer_build: peer_build.to_string(),
        peer: peer.parse().ok()?,
        ours: ours.parse().ok()?,
    })
}

pub fn server_instance() -> &'static str {
    crate::util::process_instance()
}

pub fn server_started() -> Instant {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    *STARTED.get_or_init(Instant::now)
}

pub const WATCH_BURST_CAP: usize = 1024;

pub const WATCH_COALESCE_WINDOW: Duration = Duration::from_millis(100);

pub mod kind {

    pub const HELLO: u8 = 60;
    pub const REQUEST: u8 = 61;
    pub const REQUEST_BLOB: u8 = 62;
    pub const CANCEL: u8 = 63;

    pub const HELLO_OK: u8 = 60;
    pub const RESPONSE: u8 = 61;
    pub const RESPONSE_BLOB: u8 = 62;
    pub const EVENT: u8 = 63;
}

pub const GIT_STREAM_CHUNK: usize = 64 * 1024;

pub const GIT_STREAM_CHUNK_MAX: usize = crate::daemon::protocol::MAX_FRAME / 2;

pub const GIT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub const MAX_CONCURRENT_GIT_STREAMS: usize = 8;

pub use anterm_protocol::pane::feature;

pub use crate::host::{
    ContentLimits, ContentQuery, ContentResults, Entry, MTime, Meta, Output, SearchHit,
};

pub use crate::shells::{DetectedShell, ShellInventory};

pub use crate::machine::{
    Axis, LayoutDelta, Machine, PaneSeed, ReopenedTab, Side, Tab, TabId,
};
