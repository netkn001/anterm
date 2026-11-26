use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{
    Axis, CONTROL_VERSION, ContentLimits, ContentQuery, ContentResults, Entry, LayoutDelta,
    Machine, Meta, Output, PaneSeed, ReopenedTab, SearchHit, ShellInventory, Side, Tab, TabId,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlRequest {
    Ping,

    ReadDir {
        dir: String,
        root: Option<String>,
    },
    Stat {
        path: String,
    },
    Exists {
        path: String,
    },
    Canonicalize {
        path: String,
    },
    ReadFile {
        path: String,
        max_bytes: u64,
    },
    Search {
        roots: Vec<String>,
        query: String,
        limit: u64,
        max_dirs: u64,
        show_hidden: bool,
    },
    /// Find in files. Gated on [`feature::CONTENT_SEARCH`].
    SearchContent {
        roots: Vec<String>,
        query: ContentQuery,
        limits: ContentLimits,
    },

    WriteFile {
        path: String,
    },
    /// Upload a mobile attachment (photo, screenshot, snippet) into a private
    /// host temp directory and return its host-side path for agent composers.
    /// Gated on [`crate::daemon::protocol::FEATURE_GATEWAY_UPLOAD`].
    UploadTempFile {
        filename: String,
        #[serde(default)]
        mime: String,
    },
    CreateFileNew {
        path: String,
    },
    CreateDir {
        path: String,
        recursive: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    Remove {
        path: String,
        recursive: bool,
    },

    RepoRoot {
        path: String,
    },
    Git {
        cwd: String,
        args: Vec<String>,
    },
    GitStream {
        id: u64,
        cwd: String,
        args: Vec<String>,
    },

    Shells,

    WatchOpen {
        dirs: Vec<String>,
    },
    WatchSet {
        id: u64,
        dirs: Vec<String>,
    },
    WatchClose {
        id: u64,
    },

    GuiOpen {
        path: Option<String>,
    },

    MachineGet,
    SetActiveTab {
        tab: TabId,
    },
    TabCreate {
        at: Option<u64>,
        pane: PaneSeed,
        #[serde(default)]
        tab: Option<TabId>,
    },
    TabClose {
        tab: TabId,
    },
    /// Close a tab into the machine's recently-closed list rather than for
    /// good: the peer stops its panes keeping their screens, so a later
    /// [`Self::TabReopen`] — from any client, after any restart — can put it
    /// back. `panes` is every pane the client held in the tab; any the tree
    /// never recorded there are ended outright. Answers the panes stopped.
    /// Gated on [`feature::CLOSED_TABS`].
    TabCloseRemembered {
        tab: TabId,
        #[serde(default)]
        panes: Vec<u64>,
    },
    /// Take the named closed tab, or the most recently closed one, off the
    /// machine's list and answer it with its panes' records, for the client
    /// to rebuild. Answers [`ReplyOk::ReopenedTab`]. Gated on
    /// [`feature::CLOSED_TABS`].
    TabReopen {
        #[serde(default)]
        tab: Option<TabId>,
    },
    TabRename {
        tab: TabId,
        name: Option<String>,
    },
    TabMove {
        tab: TabId,
        to: u64,
    },
    TabSetTags {
        tab: TabId,
        tags: Vec<crate::tag::TagId>,
    },
    /// The machine's sidebar tag catalog, replaced whole.
    SetTags {
        tags: crate::tag::SidebarTags,
    },
    /// Put a tab to sleep or mark it awake. Going to sleep, the peer also
    /// stops every pane the tab holds, keeping each one's screen on disk for
    /// the wake to restore; waking only clears the mark, because the panes a
    /// wake brings up are spawned by the client like any restored pane.
    /// Answers the tab's panes. Gated on [`feature::TAB_HIBERNATE`].
    TabSetHibernated {
        tab: TabId,
        hibernated: bool,
    },
    PaneSplit {
        pane: u64,
        axis: Axis,
        ratio: f32,
        new: PaneSeed,
        first: bool,
    },
    PaneClose {
        pane: u64,
    },
    PaneSetRatio {
        tab: TabId,
        path: Vec<Side>,
        ratio: f32,
    },
    PaneMove {
        pane: u64,
        to: u64,
        axis: Axis,
        first: bool,
    },
    PaneReplace {
        old: u64,
        new: PaneSeed,
    },

    AgentStates,
    /// What is running inside one of the peer's panes, and what it is
    /// listening on. `pane_id` is the peer's own id for it — the same one the
    /// client spawned the pane with.
    PaneProcs {
        pane_id: u64,
    },
    Routes,
    Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneAgentState {
    pub pane_id: u64,
    #[serde(default)]
    pub agent: Option<crate::cli_agent::CLIAgent>,
    pub state: crate::cli_agent::AgentSessionState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteInfo {
    pub key: String,
    pub kind: String,
    pub connected: bool,
}

impl RouteInfo {
    /// The host a link reaches: `build-box` in `me@build-box:22`. `None` for a
    /// key of another shape.
    pub fn host(&self) -> Option<&str> {
        let first = self.key.split('|').next()?;
        let after_user = first.split('@').nth(1)?;
        after_user.split(':').next()
    }

    /// Rebuilds the target to route to this link over the local server, from
    /// its key.
    ///
    /// Routing over a link the server already holds only needs the key's
    /// `user@host:port` — the link is found by it and its credentials are
    /// never used again — so an `auto` auth mode stands in for the profile's.
    /// A jump/proxy chain would need the whole chain rebuilt, which the key
    /// alone cannot do yet.
    pub fn target(&self) -> Result<crate::daemon::router::RouteTarget, String> {
        let unrecognized = || format!("unrecognized machine key '{}'", self.key);
        if self.kind != "ssh" {
            return Err(format!(
                "machine '{}' is a {} link — anterm can only route over ssh links yet",
                self.key, self.kind
            ));
        }
        if self.key.contains('|') {
            return Err(format!(
                "machine '{}' is reached through a jump/proxy chain, which cannot be rebuilt \
                 from the link key yet — use the GUI for this machine",
                self.key
            ));
        }
        let (user, rest) = self.key.split_once('@').ok_or_else(unrecognized)?;
        let (host, port) = rest.rsplit_once(':').ok_or_else(unrecognized)?;
        let port: u16 = port.parse().map_err(|_| unrecognized())?;
        let spec = serde_json::from_value(serde_json::json!({
            "user": user,
            "host": host,
            "port": port,
            "auth_mode": "auto",
        }))
        .map_err(|e| e.to_string())?;
        Ok(crate::daemon::router::RouteTarget::Ssh(Box::new(spec)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub pid: u32,
    pub uptime_secs: u64,
    pub panes: u64,
    pub control_version: u32,
    pub protocol_version: u32,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub socket: String,
}

impl ControlRequest {
    pub fn deadline(&self) -> Duration {
        use ControlRequest::*;
        match self {
            Ping => Duration::from_secs(5),
            ReadDir { .. }
            | Stat { .. }
            | Exists { .. }
            | Canonicalize { .. }
            | RepoRoot { .. }
            | WatchOpen { .. }
            | WatchSet { .. }
            | WatchClose { .. }
            // A poll, on a two-second timer: waiting longer than the gap
            // between asks would only stack up requests behind a peer that has
            // stopped answering.
            | PaneProcs { .. } => Duration::from_secs(5),
            ReadFile { .. } | WriteFile { .. } | UploadTempFile { .. } => Duration::from_secs(30),
            CreateFileNew { .. } | CreateDir { .. } | Rename { .. } | Remove { .. } => {
                Duration::from_secs(10)
            }
            Git { .. } | GitStream { .. } | Search { .. } => Duration::from_secs(20),
            // Past the host's own time budget (`ContentLimits::max_millis`),
            // so a search that ran long still gets to say what it found.
            SearchContent { .. } => Duration::from_secs(30),
            Shells => Duration::from_secs(20),
            GuiOpen { .. } => Duration::from_secs(5),
            MachineGet
            | SetActiveTab { .. }
            | TabCreate { .. }
            | TabClose { .. }
            | TabCloseRemembered { .. }
            | TabReopen { .. }
            | TabRename { .. }
            | TabMove { .. }
            | TabSetTags { .. }
            | SetTags { .. }
            | TabSetHibernated { .. }
            | PaneSplit { .. }
            | PaneClose { .. }
            | PaneSetRatio { .. }
            | PaneMove { .. }
            | PaneReplace { .. } => Duration::from_secs(10),
            AgentStates | Routes | Status => Duration::from_secs(5),
        }
    }

    pub fn takes_blob(&self) -> bool {
        matches!(
            self,
            ControlRequest::WriteFile { .. } | ControlRequest::UploadTempFile { .. }
        )
    }

    pub fn returns_blob(&self) -> bool {
        matches!(self, ControlRequest::ReadFile { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ControlReply {
    #[serde(rename = "ok")]
    Ok(ReplyOk),
    #[serde(rename = "err")]
    Err(WireError),
}

impl ControlReply {
    pub fn into_result(self) -> io::Result<ReplyOk> {
        match self {
            ControlReply::Ok(v) => Ok(v),
            ControlReply::Err(e) => Err(e.into_io()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyOk {
    Unit,
    Pong,
    Entries(Vec<Entry>),
    Meta(Meta),
    Bool(bool),
    Path(String),
    OptPath(Option<String>),
    FileMeta {
        meta: Meta,
    },
    Hits(Vec<SearchHit>),
    ContentHits(ContentResults),
    Output(Output),
    WatchId(u64),
    Shells(ShellInventory),
    MachineTree(Box<Machine>),
    TabTree(Box<Tab>),
    /// The answer to [`ControlRequest::TabReopen`]; `None` when the workspace
    /// has nothing left to reopen.
    ReopenedTab(Option<Box<ReopenedTab>>),
    Panes(Vec<u64>),
    AgentStates(Vec<PaneAgentState>),
    PaneProcs(crate::daemon::protocol::PaneProcs),
    Routes(Vec<RouteInfo>),
    Status(ServerStatus),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    pub kind: WireErrorKind,
    pub msg: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidInput,
    NotADirectory,
    IsADirectory,
    DirectoryNotEmpty,
    FileTooLarge,
    GitUnavailable,
    TimedOut,
    ConnectionReset,
    Other,
}

impl WireError {
    pub fn new(kind: WireErrorKind, msg: impl Into<String>) -> Self {
        WireError {
            kind,
            msg: msg.into(),
        }
    }

    pub fn from_io(e: &io::Error) -> Self {
        use io::ErrorKind as K;
        let kind = match e.kind() {
            K::NotFound => WireErrorKind::NotFound,
            K::PermissionDenied => WireErrorKind::PermissionDenied,
            K::AlreadyExists => WireErrorKind::AlreadyExists,
            K::InvalidInput | K::InvalidFilename => WireErrorKind::InvalidInput,
            K::NotADirectory => WireErrorKind::NotADirectory,
            K::IsADirectory => WireErrorKind::IsADirectory,
            K::DirectoryNotEmpty => WireErrorKind::DirectoryNotEmpty,
            K::FileTooLarge => WireErrorKind::FileTooLarge,
            K::TimedOut => WireErrorKind::TimedOut,
            K::ConnectionReset | K::BrokenPipe | K::UnexpectedEof => WireErrorKind::ConnectionReset,
            _ => WireErrorKind::Other,
        };
        WireError {
            kind,
            msg: e.to_string(),
        }
    }

    pub fn into_io(self) -> io::Error {
        io::Error::new(self.kind.to_io_kind(), self.msg)
    }
}

impl WireErrorKind {
    pub fn to_io_kind(self) -> io::ErrorKind {
        use io::ErrorKind as K;
        match self {
            WireErrorKind::NotFound | WireErrorKind::GitUnavailable => K::NotFound,
            WireErrorKind::PermissionDenied => K::PermissionDenied,
            WireErrorKind::AlreadyExists => K::AlreadyExists,
            WireErrorKind::InvalidInput => K::InvalidInput,
            WireErrorKind::NotADirectory => K::NotADirectory,
            WireErrorKind::IsADirectory => K::IsADirectory,
            WireErrorKind::DirectoryNotEmpty => K::DirectoryNotEmpty,
            WireErrorKind::FileTooLarge => K::FileTooLarge,
            WireErrorKind::TimedOut => K::TimedOut,
            WireErrorKind::ConnectionReset => K::ConnectionReset,
            WireErrorKind::Other => K::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlEvent {
    Watch {
        id: u64,
        paths: Vec<String>,
    },
    WatchOverflow {
        id: u64,
    },

    GitChunk {
        id: u64,
        #[serde(with = "crate::util::b64")]
        bytes: Vec<u8>,
    },
    GitEnd {
        id: u64,
        code: Option<i32>,
        failed: bool,
    },

    PaneExited {
        pane_id: u64,
        code: Option<i32>,
    },
    AgentStatus {
        pane_id: u64,
        json: serde_json::Value,
    },
    GuiOpen {
        path: Option<String>,
    },
    Layout {
        delta: LayoutDelta,
    },
    LayoutResync,
}

pub type EventObserver = Arc<dyn Fn(crate::host::HostId, ControlEvent) + Send + Sync>;

static EVENT_OBSERVER: Mutex<Option<EventObserver>> = Mutex::new(None);

pub fn set_event_observer(f: EventObserver) {
    if let Ok(mut slot) = EVENT_OBSERVER.lock() {
        *slot = Some(f);
    }
}

pub fn observe_event(host: crate::host::HostId, event: ControlEvent) {
    let observer = match EVENT_OBSERVER.lock() {
        Ok(slot) => slot.clone(),
        Err(_) => None,
    };
    match observer {
        Some(f) => f(host, event),
        None => log::trace!("control event with nobody to hear it: {event:?}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlHello {
    pub control_version: u32,
    pub client_token: String,
    pub client_hostname: String,
    #[serde(default)]
    pub gui: bool,
}

impl ControlHello {
    pub fn host_rpc(client_token: impl Into<String>, client_hostname: impl Into<String>) -> Self {
        ControlHello {
            control_version: CONTROL_VERSION,
            client_token: client_token.into(),
            client_hostname: client_hostname.into(),
            gui: false,
        }
    }

    pub fn gui(client_token: impl Into<String>, client_hostname: impl Into<String>) -> Self {
        ControlHello {
            gui: true,
            ..Self::host_rpc(client_token, client_hostname)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlHelloOk {
    pub control_version: u32,
    pub protocol_version: u32,
    pub build: String,
    pub separator: char,
    pub home: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub instance: String,
}

impl ControlHelloOk {
    pub fn has_feature(&self, name: &str) -> bool {
        self.features.iter().any(|f| f == name)
    }
}
