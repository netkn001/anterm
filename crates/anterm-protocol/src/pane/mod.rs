mod codec;
mod ssh;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use codec::{is_error_kind, peek_frame_kind, read_frame, take_frame, write_frame};
pub use ssh::{
    AuthPromptKind, AuthResponse, ForwardStatus, KiPrompt, KnownHostEntry, KnownHostId,
    LoopbackForward, LoopbackForwardRequest, ManagedForward, NativeSshSpec, PaneContext,
    PaneProcs, PortEntry, PortProbe, ProcEntry, SftpEntry, SftpEntryKind, SftpJobProgress,
    SftpJobState, SftpOp, SftpOpResult, SftpTransferKind, SftpTransferSpec, SshAlgorithms,
    SshAuthMode, SshForwardKind, SshForwardRule, SshPhase, SshProxy, SshTestNeed, SshTestReport,
    ssh_option_takes_value,
};

pub const MAX_FRAME: usize = 64 * 1024 * 1024;

pub const PROTOCOL_VERSION: u32 = 7;

/// Named capability flags advertised in [`DaemonVersion::features`].
pub mod feature {
    pub const CONTROL: &str = "control";
    pub const HOST_RPC: &str = "host-rpc";
    pub const MACHINE_TREE: &str = "machine-tree";
    pub const STDIO_BRIDGE: &str = "stdio-bridge";
    pub const PANE_PROCS: &str = "pane-procs";
    pub const TAB_HIBERNATE: &str = "tab-hibernate";
    pub const CONTENT_SEARCH: &str = "content-search";
    pub const CLOSED_TABS: &str = "closed-tabs";
}

pub const FEATURE_PANE_OWNER: &str = "pane-owner";

/// The daemon echoes a `DaemonMsg::Size` to the controlling subscriber, in
/// stream order, when it applies a `ClientMsg::Resize`. A client that sees
/// this feature defers its local grid reflow to that echo so the reflow lands
/// at the stream position where the PTY actually changed geometry; against an
/// older daemon it must keep reflowing locally at request time.
pub const FEATURE_RESIZE_ECHO: &str = "resize-echo";

/// The daemon can seed a new pane with the screen a dead one left behind, named
/// by `ClientMsg::Spawn`'s `restore` field. A client that does not see this
/// feature leaves the field out: an older daemon would ignore it and spawn a
/// blank pane, which is the same outcome, but sending it would make the wire
/// claim a restore that never happened.
pub const FEATURE_RESTORE_SCROLLBACK: &str = "restore-scrollback";

/// The daemon can replace its own binary without stopping, keeping every pty
/// and everything running on one — `ClientMsg::Handoff`. Advertised only where
/// it can actually be done, which is where `execve` exists, so a client can use
/// it to choose between offering an upgrade that costs the user nothing and one
/// that costs them every running command.
pub const FEATURE_HANDOFF: &str = "handoff";

/// The daemon's router understands `RouteAction::UpdateServer`: put this
/// build's server on a remote host even when one speaking our dialect is
/// already there. A daemon without it cannot decode a header asking for that
/// and drops the route unanswered, so the client checks first and says why.
pub const FEATURE_UPDATE_SERVER: &str = "update-server";

/// The daemon understands [`ClientMsg::Lease`]: an observer can show a pane at
/// its own size until it lets go or the controller takes it back. A client
/// must see this before sending one; an older daemon cannot decode the frame
/// and drops the connection.
pub const FEATURE_SIZE_LEASE: &str = "size-lease";

/// The daemon understands [`ClientMsg::ClearScrollback`]: clears the pane's
/// in-memory replay ring and removes its persisted scrollback snapshot on disk.
pub const FEATURE_CLEAR_SCROLLBACK: &str = "clear-scrollback";

/// The daemon understands [`ClientMsg::ObserveSince`] and emits
/// [`DaemonMsg::ReplayCursor`] ahead of delta or full replay so mobile clients
/// resume across lock/network handoffs without re-downloading the whole ring.
pub const FEATURE_DELTA_REPLAY: &str = "delta-replay";

/// The daemon records `transcript_path` on agent sessions and can stream
/// structured [`crate::transcript::TranscriptBatch`] updates without touching
/// the pane's PTY size.
pub const FEATURE_AGENT_TRANSCRIPT: &str = "agent-transcript";

/// The control server supports `UploadTempFile` for mobile photo/file composer
/// attachments.
pub const FEATURE_GATEWAY_UPLOAD: &str = "gateway-upload";

/// Whether a [`ClientMsg::ObserveSince`] replay carries only new delta bytes or
/// fell back to a complete ring snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    Delta,
    Full,
}

/// What a [`ClientMsg::Lease`] asks. The first two are an observer's, the last
/// two the controller's; each side's are ignored from the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseRequest {
    /// Put the pty at `size` while this observer is connected. `by` names
    /// it to the controller ("Thomas's iPhone"). Sent again to change size.
    Take { size: WinSize, by: String },
    /// Put the pane back at the size the controller asked for.
    Release,
    /// The controller can hear [`DaemonMsg::Lease`]; tell it now and on
    /// every change.
    Watch,
    /// The controller ends the lease, whoever holds it.
    TakeBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonVersion {
    pub protocol: u32,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub instance: String,
}

impl DaemonVersion {
    pub fn current() -> DaemonVersion {
        let mut features = vec![
            FEATURE_PANE_OWNER.to_string(),
            FEATURE_RESIZE_ECHO.to_string(),
            FEATURE_RESTORE_SCROLLBACK.to_string(),
            FEATURE_UPDATE_SERVER.to_string(),
            FEATURE_SIZE_LEASE.to_string(),
            FEATURE_CLEAR_SCROLLBACK.to_string(),
            FEATURE_DELTA_REPLAY.to_string(),
            FEATURE_AGENT_TRANSCRIPT.to_string(),
        ];
        if cfg!(unix) {
            features.push(FEATURE_HANDOFF.to_string());
        }
        DaemonVersion {
            protocol: PROTOCOL_VERSION,
            build: env!("CARGO_PKG_VERSION").to_string(),
            features,
            instance: process_instance().to_string(),
        }
    }

    pub fn has_feature(&self, name: &str) -> bool {
        self.features.iter().any(|f| f == name)
    }
}

pub fn process_instance() -> &'static str {
    crate::util::process_instance()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WinSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub args_are_anterm_defaults: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane_id: u64,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub title: String,
    /// See [`crate::core::machine::PaneRecord::osc_title`] — the terminal's own
    /// title, not the foreground process name `title` carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub osc_title: Option<String>,
    pub alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteContext {
    pub kind: RemoteKind,
    pub argv: Vec<String>,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteKind {
    Ssh,
    NativeSsh,
    Wsl,
}

/// "This pane replaces one that died with the daemon."
///
/// Carried on a spawn rather than an attach because there is nothing to attach
/// to: the process is gone. The daemon looks up what pane `pane_id` last had on
/// its screen and seeds the new pane's ring with it, so the window shows the
/// output it lost under a shell that is plainly new.
///
/// `banner` is the line drawn between the two, and it comes from the client
/// because the daemon has no locale — it serves a GUI that might be running in
/// any language, and a CLI whose output is always English. A client that has
/// nothing to say can leave it out; the reset sequence is emitted either way.
///
/// Old daemons decode this frame without the field and simply spawn a blank
/// pane, which is what they did before it existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreFrom {
    pub pane_id: u64,
    #[serde(default)]
    pub banner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    Spawn {
        cwd: Option<PathBuf>,
        size: WinSize,
        shell: Option<ShellSpec>,
        owner: Option<String>,
        restore: Option<RestoreFrom>,
        allow_remote_clipboard_write: bool,
    },
    Attach {
        pane_id: u64,
        size: WinSize,
        allow_remote_clipboard_write: bool,
    },
    Observe {
        pane_id: u64,
        size: WinSize,
    },
    /// Like [`ClientMsg::Observe`], but resumes from `since_offset` in the
    /// daemon's replay ring when still available, avoiding full 8 MiB re-sync
    /// on mobile wake or network switch. Only sent after the daemon advertises
    /// [`FEATURE_DELTA_REPLAY`].
    ObserveSince {
        pane_id: u64,
        size: WinSize,
        since_offset: Option<u64>,
    },
    Input(Vec<u8>),
    SendInput {
        pane_id: u64,
        bytes: Vec<u8>,
    },
    /// An agent hook's report about the pane it runs in — the JSON body of
    /// the `anterm://cli-agent` OSC 777 sequence — handed straight to the
    /// daemon rather than written to the pane's tty, where the agent's own
    /// output can cut into it. `pid` is the hook's process, which has to be
    /// running under the pane for the report to count. Answered with
    /// `InputAck` when applied and `Error` when not, so the hook knows to
    /// fall back to the tty.
    AgentEvent {
        pane_id: u64,
        pid: u32,
        event: String,
    },
    Resize(WinSize),
    ClearScrollback,
    Detach,
    Kill {
        pane_id: u64,
    },
    List,
    Shutdown,
    /// Become `exe` without stopping: the daemon rewrites itself in place and
    /// keeps every pty, shell and pane id it is holding. The connection dies in
    /// the process — the new image has never heard of it — so this is the last
    /// thing a client can say on it, and the reply is the socket closing.
    ///
    /// Unix only. Elsewhere the daemon answers with an error and the caller
    /// falls back to stopping and starting it.
    Handoff {
        exe: PathBuf,
    },
    EnsureLoopbackForward(LoopbackForwardRequest),
    SpawnNativeSsh {
        /// The directory *on the remote host* for the shell to start in —
        /// where a pane being dialled again (restore, Reconnect, a split, ⌘T)
        /// last reported it was. Never a path on this machine: it is not
        /// checked or canonicalized here, only handed to the far shell, and a
        /// fresh connection from a saved host sends `None`.
        ///
        /// Honoured only when the session gets a shell-integration bootstrap
        /// to carry it; a relative path is ignored, and one that no longer
        /// exists leaves the shell in the login directory.
        cwd: Option<PathBuf>,
        size: WinSize,
        spec: Box<NativeSshSpec>,
    },
    AuthResponse {
        request_id: u64,
        response: AuthResponse,
    },
    ListKnownHosts,
    DeleteKnownHost(KnownHostId),
    /// Open this connection, report what happened, and drop it. Answered with
    /// exactly one [`DaemonMsg::SshTestResult`].
    TestSsh {
        spec: Box<NativeSshSpec>,
    },
    SftpList {
        pane_id: u64,
        path: String,
    },
    SftpOp {
        pane_id: u64,
        op: SftpOp,
    },
    SftpTransferStart(SftpTransferSpec),
    SftpTransferCancel {
        job_id: u64,
    },
    SftpTransferList {
        pane_id: u64,
    },
    AddForward {
        pane_id: u64,
        rule: SshForwardRule,
    },
    RemoveForward {
        pane_id: u64,
        forward_id: u64,
    },
    SetForwardEnabled {
        pane_id: u64,
        forward_id: u64,
        enabled: bool,
    },
    ListForwards {
        pane_id: u64,
    },
    QueryProcs {
        pane_id: u64,
    },
    Version,
    /// Only after the daemon advertised [`FEATURE_SIZE_LEASE`].
    Lease(LeaseRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonMsg {
    Spawned {
        pane_id: u64,
    },
    Size(WinSize),
    Snapshot(Vec<u8>),
    Output(Vec<u8>),
    /// A kitty graphics image lifted out of the PTY stream daemon-side (issue
    /// #213). Carried out-of-band as a compact binary frame
    /// ([`crate::core::kitty_graphics::Image::encode_frame`]) so the base64 text
    /// never rides the socket and the client's VT parser never sees it. The
    /// pixel payload stays *compressed* on the wire — the client inflates — so a
    /// remote pane's frames don't balloon across the SSH tunnel.
    Image(Vec<u8>),
    /// A kitty graphics delete (`a=d`) lifted out of the PTY stream daemon-side.
    /// Payload is a compact selector frame
    /// ([`crate::core::kitty_graphics::ImageDelete::encode`]) telling the client
    /// which stored image(s)/placement(s) to drop.
    DeleteImage(Vec<u8>),
    /// A completed OSC 5522 clipboard write. The daemon strips the control
    /// sequence from replay; the GUI decides whether it may touch the clipboard.
    ClipboardWrite(Vec<u8>),
    Cwd(PathBuf),
    Prompt {
        active: bool,
        at_prompt: bool,
        last_exit: Option<i32>,
    },
    Exited {
        code: Option<i32>,
    },
    PaneList(Vec<PaneInfo>),
    InputAck {
        pane_id: u64,
    },
    RemoteContext(Option<RemoteContext>),
    Agent(Option<crate::cli_agent::CLIAgent>),
    AgentStatus(Option<crate::cli_agent::AgentSessionState>),
    LoopbackForward(LoopbackForward),
    AuthPrompt {
        request_id: u64,
        prompt: AuthPromptKind,
    },
    SshStatus {
        phase: SshPhase,
    },
    KnownHostsList(Vec<KnownHostEntry>),
    SshTestResult(SshTestReport),
    SftpEntries(Vec<SftpEntry>),
    SftpOpResult(SftpOpResult),
    SftpTransferStarted {
        job_id: u64,
    },
    SftpTransferProgress(Vec<SftpJobProgress>),
    ForwardList(Vec<ManagedForward>),
    Procs(PaneProcs),
    Version(DaemonVersion),
    /// Who holds the pane at their own size, or `None` once nobody does. Sent
    /// to the holding observer and to a controller that asked with
    /// [`LeaseRequest::Watch`], never to anyone else: older clients cannot
    /// decode it.
    Lease(Option<String>),
    /// Sent ahead of a [`ClientMsg::ObserveSince`] replay, telling the client
    /// the monotonic ring byte offset (`offset`) covered by the replay and
    /// whether it is an incremental [`ReplayMode::Delta`] or a [`ReplayMode::Full`]
    /// reset.
    ReplayCursor {
        offset: u64,
        mode: ReplayMode,
    },
    Error(String),
}
