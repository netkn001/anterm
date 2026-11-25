use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::RemoteContext;

pub fn ssh_option_takes_value(flag: char) -> bool {
    matches!(
        flag,
        'B' | 'b'
            | 'c'
            | 'D'
            | 'E'
            | 'e'
            | 'F'
            | 'I'
            | 'i'
            | 'J'
            | 'L'
            | 'l'
            | 'm'
            | 'O'
            | 'o'
            | 'p'
            | 'Q'
            | 'R'
            | 'S'
            | 'W'
            | 'w'
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackForwardRequest {
    pub pane_id: u64,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackForward {
    pub local_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SshAuthMode {
    #[default]
    Auto,
    Gssapi,
    Password,
    PublicKey,
    Agent,
    KeyboardInteractive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SshProxy {
    #[default]
    None,
    Command(String),
    Socks {
        host: String,
        port: u16,
    },
    Http {
        host: String,
        port: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SshAlgorithms {
    #[serde(default)]
    pub kex: Vec<String>,
    #[serde(default)]
    pub cipher: Vec<String>,
    #[serde(default)]
    pub mac: Vec<String>,
    #[serde(default)]
    pub host_key: Vec<String>,
    #[serde(default)]
    pub compression: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SshForwardKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshForwardRule {
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    #[serde(default)]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
    #[serde(default)]
    pub description: Option<String>,
    /// A switched-off rule is registered but binds nothing. Defaults to on,
    /// and is not a `PROTOCOL_VERSION` bump: a peer that predates it never
    /// sends it and every rule it knows is live, while one that receives it
    /// ignores it — the worst case is an older far end opening a rule the
    /// user switched off, which is what it always did.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SftpEntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub kind: SftpEntryKind,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mtime: u64,
    #[serde(default)]
    pub permissions: u32,
    #[serde(default)]
    pub target_is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SftpOp {
    Stat {
        path: String,
    },
    Mkdir {
        path: String,
    },
    CreateFile {
        path: String,
    },
    RemoveFile {
        path: String,
    },
    RemoveDir {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    Readlink {
        path: String,
    },
    Realpath {
        path: String,
    },
    /// Whole-file read, for the built-in editor. `max_bytes` is the reader's
    /// own ceiling; a file larger than it answers with an error instead of a
    /// truncated body that would later be saved back short.
    ReadFile {
        path: String,
        max_bytes: u64,
    },
    /// Whole-file write, in place (truncate + write, no temp-and-rename): the
    /// editor saves over a file the user already has open, and replacing the
    /// inode would silently drop its mode and ownership.
    WriteFile {
        path: String,
        #[serde(with = "crate::util::b64")]
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SftpOpResult {
    Done,
    Stat(SftpEntry),
    Link(String),
    Error(String),
    /// Reply to [`SftpOp::ReadFile`]: the bytes, plus the stat they were
    /// actually read under. The stat costs nothing — the size check before
    /// the read has it in hand either way — and it is the only description of
    /// the body that cannot disagree with it, which a separate `Stat` round
    /// trip either side of the read can.
    File {
        entry: SftpEntry,
        #[serde(with = "crate::util::b64")]
        bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SftpTransferKind {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpTransferSpec {
    pub pane_id: u64,
    pub kind: SftpTransferKind,
    pub local: PathBuf,
    pub remote: String,
    #[serde(default)]
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SftpJobState {
    Running,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpJobProgress {
    pub job_id: u64,
    pub pane_id: u64,
    pub kind: SftpTransferKind,
    pub state: SftpJobState,
    #[serde(default)]
    pub current: String,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub bytes_total: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub local: String,
    #[serde(default)]
    pub remote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForwardStatus {
    Listening,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedForward {
    pub id: u64,
    pub pane_id: u64,
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    #[serde(default)]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
    #[serde(default)]
    pub description: Option<String>,
    pub status: ForwardStatus,
    /// Whether the forward is switched on. A field rather than a
    /// `ForwardStatus` variant for the reason `loop_exit_status` gives: an
    /// older peer fails the whole frame on a variant it has never heard of,
    /// but skips a field it does not know. A switched-off forward's status is
    /// an `Error` saying so, which is what such a peer then draws.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcEntry {
    pub pid: u32,
    pub name: String,
    pub depth: u8,
    #[serde(default)]
    pub foreground: bool,
    /// Resident memory in bytes, total CPU time (user + system) in ns, and an
    /// opaque start stamp that tells a reused pid from the process it replaced.
    /// `None` from a daemon that predates them or could not read them; not a
    /// `PROTOCOL_VERSION` bump, since either side skips what it does not know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortEntry {
    pub port: u16,
    pub pid: u32,
    pub name: String,
    /// The address the socket is bound to, as `lsof` spells it — `*`,
    /// `0.0.0.0`, `127.0.0.1`, `[::1]`, or a specific interface.
    ///
    /// `serde(default)` because a daemon from before this field existed
    /// answers `QueryProcs` without it, and an empty address is read the same
    /// way that daemon's callers read every address: as localhost.
    #[serde(default)]
    pub addr: String,
}

impl PortEntry {
    /// Whether a bound address can be reached on this machine's loopback —
    /// true for the wildcards and the loopback addresses themselves, false for
    /// a socket pinned to one specific non-loopback interface.
    pub fn reaches_loopback(addr: &str) -> bool {
        matches!(
            addr,
            "" | "*" | "0.0.0.0" | "::" | "[::]" | "127.0.0.1" | "::1" | "[::1]" | "localhost"
        )
    }

    /// What to copy, and what to open in a browser: `host:port`.
    ///
    /// Loopback and wildcard binds are spelled `localhost`, which is what
    /// anyone typing the address by hand would write. A socket bound to one
    /// specific interface keeps that interface — the panel presents this as
    /// the address of the pane's server, and `localhost` would not be it.
    pub fn authority(&self) -> String {
        match Self::reaches_loopback(&self.addr) {
            true => format!("localhost:{}", self.port),
            false => format!("{}:{}", self.addr, self.port),
        }
    }
}

/// Whether the answer in `PaneProcs::ports` can be believed.
///
/// An empty port list used to mean two very different things at once: nothing
/// in this pane is listening, or the thing that looks for listeners never got
/// to say. On unix that look is an `lsof` subprocess, and every way it can go
/// wrong — absent from the daemon's `PATH`, killed, hung on a wedged mount,
/// pointed at sockets it has no permission to read — arrived as the same empty
/// vector as a genuinely quiet pane. Whoever reads the list gets to know which
/// one it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum PortProbe {
    /// The probe ran and the list is its whole answer.
    #[default]
    Ok,
    /// The probe ran, but at least one process in this pane belongs to another
    /// user — `sudo go run`, a root-owned server on a low port — and a probe
    /// running as this user cannot see that process's sockets. The list holds
    /// what could be seen, which may be nothing.
    Restricted,
    /// The probe could not be run at all. The string is for a log line or for
    /// `anterm procs`, not for the panel: it names the tool and what went wrong.
    Unavailable(String),
}

impl PortProbe {
    pub fn is_ok(&self) -> bool {
        matches!(self, PortProbe::Ok)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneProcs {
    pub procs: Vec<ProcEntry>,
    pub ports: Vec<PortEntry>,
    /// `serde(default)` because a daemon from before this field existed
    /// answers `QueryProcs` without it, and its silence is read the way that
    /// daemon's callers read every answer it gives: as a complete one.
    #[serde(default)]
    pub probe: PortProbe,
    /// What the pane can say about itself that the process list cannot — see
    /// [`PaneContext`]. `None` from a daemon built before the field existed,
    /// which reads as "this daemon cannot say", never as a set of falses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<PaneContext>,
}

/// Where a pane's session actually lives, and what its shell says about itself.
///
/// The process list beside it is always *this* machine's: it starts at the
/// pty's own child and walks down. For a pane that is only the near end of a
/// connection — an `ssh` the shell is running, a native-SSH pane whose pty is
/// on another host — that list describes the tunnel, not the work. This is the
/// part of the answer that can still speak for the far side.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneContext {
    /// The host the pane is pointed at, when it is not this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteContext>,
    /// Whether this machine holds the pane's pty at all. `false` for a
    /// native-SSH pane, whose `procs` is empty because there is nothing here
    /// to walk — not because the walk failed.
    pub local_pty: bool,
    /// What the pane's shell integration last said, `None` until it emits its
    /// first OSC 133 mark.
    ///
    /// This is the mark's own reading, taken before the suppression that keeps
    /// a foreground program's prompt marks from engaging the local line editor.
    /// That suppression is right for the editor and wrong here: on a pane
    /// running `ssh` the marks are the *far* shell's, and they are the only
    /// thing on this side that knows whether the far shell is busy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_prompt: Option<bool>,
    /// Whether a prompt mark has arrived while the pane was pointed at
    /// `remote`. Only the far shell can be at a prompt while the near one is
    /// occupied by the connection, so this is the proof that the far side's
    /// shell integration is loaded and reporting. Without it, "not at a
    /// prompt" on a remote pane means nothing: the newest mark is then the
    /// near shell's own "I started `ssh`", and it will never be replaced.
    pub remote_prompt_seen: bool,
    /// Whether the pane's output has switched bracketed paste (mode 2004) on,
    /// from the same fold the replay restores modes from. `None` from a daemon
    /// built before the field existed — "cannot say", not "off".
    ///
    /// It is here so a client with no terminal of its own — `anterm send
    /// --paste` — can frame a paste the way the GUI does, by asking the pane
    /// rather than assuming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bracketed_paste: Option<bool>,
}

fn default_term() -> String {
    "xterm-256color".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSshSpec {
    pub host: String,
    pub port: u16,
    pub user: String,

    pub auth_mode: SshAuthMode,
    #[serde(default)]
    pub identity_files: Vec<String>,
    #[serde(default)]
    pub agent_forward: bool,

    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub key_passphrases: Option<std::collections::HashMap<String, String>>,

    #[serde(default)]
    pub proxy: SshProxy,
    #[serde(default)]
    pub jump: Option<Box<NativeSshSpec>>,

    #[serde(default)]
    pub forwards: Vec<SshForwardRule>,

    #[serde(default)]
    pub keepalive_interval_s: Option<u32>,
    #[serde(default)]
    pub keepalive_count_max: Option<u32>,
    #[serde(default)]
    pub connect_timeout_s: Option<u32>,

    #[serde(default)]
    pub algorithms: SshAlgorithms,
    #[serde(default)]
    pub x11: bool,

    #[serde(default = "default_term")]
    pub term: String,
    #[serde(default = "default_true")]
    pub verify_host_keys: bool,
    #[serde(default)]
    pub skip_banner: bool,
    #[serde(default = "default_true")]
    pub shell_integration: bool,
    #[serde(default)]
    pub remote_clipboard_write: bool,
    #[serde(default)]
    pub login_script: Vec<String>,

    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub profile_id: Option<String>,
}

/// What a connection test found. The daemon reports the outcome, never the
/// wording: the sentence a user reads is localized, and the daemon has no
/// locale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshTestReport {
    /// Transport, host key and authentication all went through. The connection
    /// was dropped immediately afterwards.
    Authenticated { elapsed_ms: u32 },
    /// The server answered and the handshake got as far as needing something
    /// only a person can supply. Reaching this point already proves the address,
    /// the port, the proxy chain and the jump host.
    NeedsInput { need: SshTestNeed, elapsed_ms: u32 },
    /// Never got that far. The reason is the connect path's own message, which
    /// is what the pane would have printed.
    Failed { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshTestNeed {
    Password,
    KeyPassphrase,
    KeyboardInteractive,
    /// The server's host key is one nobody here has accepted yet.
    HostKeyDecision,
    /// The server presented a *different* key than the one on file. Kept apart
    /// from `HostKeyDecision` because "not accepted yet" and "not the key it
    /// gave last time" are not the same news.
    HostKeyChanged,
}

impl NativeSshSpec {
    pub fn without_secrets(&self) -> NativeSshSpec {
        NativeSshSpec {
            password: None,
            key_passphrases: None,
            jump: self.jump.as_ref().map(|j| Box::new(j.without_secrets())),
            ..self.clone()
        }
    }
}

impl std::fmt::Debug for NativeSshSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeSshSpec")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("auth_mode", &self.auth_mode)
            .field("identity_files", &self.identity_files)
            .field("agent_forward", &self.agent_forward)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field(
                "key_passphrases",
                &self.key_passphrases.as_ref().map(|m| m.len()),
            )
            .field("proxy", &self.proxy)
            .field("jump", &self.jump)
            .field("forwards", &self.forwards)
            .field("keepalive_interval_s", &self.keepalive_interval_s)
            .field("keepalive_count_max", &self.keepalive_count_max)
            .field("connect_timeout_s", &self.connect_timeout_s)
            .field("algorithms", &self.algorithms)
            .field("x11", &self.x11)
            .field("term", &self.term)
            .field("verify_host_keys", &self.verify_host_keys)
            .field("skip_banner", &self.skip_banner)
            .field("shell_integration", &self.shell_integration)
            .field("remote_clipboard_write", &self.remote_clipboard_write)
            .field("login_script", &self.login_script)
            .field("display_name", &self.display_name)
            .field("profile_id", &self.profile_id)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHostEntry {
    pub host: String,
    #[serde(default)]
    pub marker: Option<String>,
    pub key_type: String,
    pub fingerprint_sha256: String,
    pub id: KnownHostId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHostId {
    pub host: String,
    pub key_type: String,
    pub keyblob: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiPrompt {
    pub text: String,
    pub echo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthPromptKind {
    Password {
        user: String,
        host: String,
    },
    KeyPassphrase {
        key_path: String,
        comment: String,
        /// The connection already had a passphrase for this key and the key
        /// would not decrypt with it, so the one the user is about to type
        /// replaces a stored secret rather than adding a first one. Only the
        /// daemon can know this — the client sees a passphrase prompt and
        /// cannot tell a first ask from a second — and without it a wrong
        /// "remember" bricks the key for every later connection.
        ///
        /// `#[serde(default)]`, and deliberately not a `PROTOCOL_VERSION`
        /// bump: an older peer on either side of this field simply never sets
        /// it, and serde ignores fields it does not know, so the flag is
        /// compatible in both directions across daemon↔GUI and GUI↔remote
        /// `anterm-server`.
        #[serde(default)]
        rejected: bool,
    },
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<KiPrompt>,
        /// The round that just failed was answered with the stored password
        /// rather than by the user, so this prompt exists to replace it. Same
        /// compatibility reasoning as `KeyPassphrase::rejected` above.
        #[serde(default)]
        stored_rejected: bool,
    },
    HostKeyUnknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint_sha256: String,
        /// The algorithm this host is already on file under, when the key is
        /// new only because its algorithm is.
        ///
        /// A field rather than a variant of its own on purpose. This enum is
        /// externally tagged and travels both to the GUI and to the remote
        /// `anterm-server`, either of which may be an older build; an unknown
        /// variant fails the whole decode, while an unknown field is ignored
        /// and a missing one defaults. So an old peer shows the plain
        /// unknown-host confirmation, which is the right prompt either way.
        #[serde(default)]
        previously_known_as: Option<String>,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint_sha256: String,
        old_fingerprint_sha256: String,
    },
    Banner {
        text: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthResponse {
    Secret(String),
    Secrets(Vec<String>),
    HostKeyDecision { accept: bool, remember: bool },
    Cancelled,
}

impl std::fmt::Debug for AuthResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthResponse::Secret(_) => f.write_str("Secret(<redacted>)"),
            AuthResponse::Secrets(v) => write!(f, "Secrets(<{} redacted>)", v.len()),
            AuthResponse::HostKeyDecision { accept, remember } => f
                .debug_struct("HostKeyDecision")
                .field("accept", accept)
                .field("remember", remember)
                .finish(),
            AuthResponse::Cancelled => f.write_str("Cancelled"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshPhase {
    Connecting,
    Authenticating,
    Connected,
    Failed { reason: String },
}
