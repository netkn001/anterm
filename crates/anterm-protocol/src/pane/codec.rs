use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util::{from_json, to_json};

use super::{ClientMsg, DaemonMsg, MAX_FRAME, RestoreFrom, ShellSpec, WinSize};

/// A frame is a little-endian `u32` length, a one-byte kind, then the payload.
const HEADER: usize = 5;

pub(super) mod kind {
    pub const SPAWN: u8 = 1;
    pub const ATTACH: u8 = 2;
    pub const INPUT: u8 = 3;
    pub const RESIZE: u8 = 4;
    pub const DETACH: u8 = 5;
    pub const KILL: u8 = 6;
    pub const LIST: u8 = 7;
    pub const SHUTDOWN: u8 = 8;
    pub const SPAWN_SHELL: u8 = 9;
    pub const ENSURE_LOOPBACK_FORWARD: u8 = 10;
    pub const SPAWN_NATIVE_SSH: u8 = 14;
    pub const AUTH_RESPONSE: u8 = 15;
    pub const LIST_KNOWN_HOSTS: u8 = 16;
    pub const DELETE_KNOWN_HOST: u8 = 17;
    pub const TEST_SSH: u8 = 18;
    pub const SFTP_LIST: u8 = 30;
    pub const SFTP_OP: u8 = 31;
    pub const SFTP_TRANSFER_START: u8 = 32;
    pub const SFTP_TRANSFER_CANCEL: u8 = 33;
    pub const SFTP_TRANSFER_LIST: u8 = 34;
    pub const ADD_FORWARD: u8 = 20;
    pub const REMOVE_FORWARD: u8 = 21;
    pub const LIST_FORWARDS: u8 = 22;
    pub const SET_FORWARD_ENABLED: u8 = 23;
    pub const VERSION: u8 = 40;
    pub const QUERY_PROCS: u8 = 50;
    pub const SPAWN_OWNED: u8 = 53;
    pub const OBSERVE: u8 = 54;
    pub const SEND_INPUT: u8 = 55;
    pub const HANDOFF: u8 = 56;
    pub const LEASE: u8 = 57;
    pub const AGENT_EVENT: u8 = 58;
    pub const CLEAR_SCROLLBACK: u8 = 59;

    pub const SPAWNED: u8 = 1;
    pub const SNAPSHOT: u8 = 2;
    pub const OUTPUT: u8 = 3;
    pub const CWD: u8 = 4;
    pub const PROMPT: u8 = 5;
    pub const EXITED: u8 = 6;
    pub const PANE_LIST: u8 = 7;
    pub const ERROR: u8 = 8;
    pub const SIZE: u8 = 9;
    pub const REMOTE_CONTEXT: u8 = 10;
    pub const LOOPBACK_FORWARD: u8 = 11;
    pub const AUTH_PROMPT: u8 = 13;
    pub const SSH_STATUS: u8 = 14;
    pub const KNOWN_HOSTS_LIST: u8 = 15;
    pub const SSH_TEST_RESULT: u8 = 16;
    pub const SFTP_ENTRIES: u8 = 30;
    pub const SFTP_OP_RESULT: u8 = 31;
    pub const SFTP_TRANSFER_STARTED: u8 = 32;
    pub const SFTP_TRANSFER_PROGRESS: u8 = 33;
    pub const FORWARD_LIST: u8 = 20;
    pub const AGENT: u8 = 21;
    pub const AGENT_STATUS: u8 = 22;
    pub const VERSION_REPLY: u8 = 40;
    pub const PROCS: u8 = 50;
    pub const INPUT_ACK: u8 = 51;
    /// `Image` — a kitty graphics frame lifted out of the PTY stream (issue
    /// #213). 60 sits clear of every range above; the payload is the compact
    /// binary encoding, not JSON, so it stays outside the `to_json` arms.
    pub const IMAGE: u8 = 60;
    /// `DeleteImage` — a kitty graphics `a=d` delete lifted out of the stream
    /// (issue #213). Compact binary selector, like `IMAGE`.
    pub const DELETE_IMAGE: u8 = 61;
    pub const CLIPBOARD_WRITE: u8 = 62;
    pub const LEASE_HOLDER: u8 = 63;
}

pub fn write_frame<W: Write>(w: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
    let len = payload.len();
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload exceeds MAX_FRAME",
        ));
    }
    // One write, not three. The pane socket is a loopback `TcpStream` with
    // `TCP_NODELAY` set, so three `write_all`s put the length, the kind and the
    // payload on the wire as three separate segments, and the reader on the far
    // side wakes from `read()` three times for one frame. Under a PTY flood the
    // daemon frames every ConPTY read, so that is two extra syscalls on each
    // side per frame, tens of thousands a second (issue #713).
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(&(len as u32).to_le_bytes());
    header[4] = kind;
    let mut bufs = [io::IoSlice::new(&header), io::IoSlice::new(payload)];
    let mut rest: &mut [io::IoSlice<'_>] = &mut bufs;
    while !rest.is_empty() {
        match w.write_vectored(rest) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the frame could not be written in full",
                ));
            }
            // A writer that does not implement `write_vectored` natively falls
            // back to writing the first non-empty slice, so this loop still
            // terminates — it just costs the two writes it used to cost.
            Ok(n) => io::IoSlice::advance_slices(&mut rest, n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<(u8, Vec<u8>)> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload exceeds MAX_FRAME",
        ));
    }
    let mut kind = [0u8; 1];
    r.read_exact(&mut kind)?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((kind[0], payload))
}

pub fn peek_frame_kind(buf: &[u8]) -> Option<u8> {
    (buf.len() >= 5).then(|| buf[4])
}

pub fn is_error_kind(kind: u8) -> bool {
    kind == kind::ERROR
}

pub fn take_frame(buf: &mut Vec<u8>) -> io::Result<Option<(u8, Vec<u8>)>> {
    if buf.len() < HEADER {
        return Ok(None);
    }
    let len = u32::from_le_bytes(
        buf[..4]
            .try_into()
            .expect("frame header length verified above"),
    ) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame payload exceeds MAX_FRAME",
        ));
    }
    if buf.len() < HEADER + len {
        return Ok(None);
    }
    let kind = buf[4];
    let payload = buf[HEADER..HEADER + len].to_vec();
    buf.drain(..HEADER + len);
    Ok(Some((kind, payload)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnedSpawn {
    #[serde(default)]
    cwd: Option<PathBuf>,
    size: WinSize,
    #[serde(default)]
    shell: Option<ShellSpec>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    restore: Option<RestoreFrom>,
    #[serde(default)]
    allow_remote_clipboard_write: bool,
}

impl ClientMsg {
    pub fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self {
            ClientMsg::Spawn {
                cwd,
                size,
                shell: None,
                owner: None,
                restore: None,
                allow_remote_clipboard_write: false,
            } => write_frame(w, kind::SPAWN, &to_json(&(cwd, size))?),
            ClientMsg::Spawn {
                cwd,
                size,
                shell: shell @ Some(_),
                owner: None,
                restore: None,
                allow_remote_clipboard_write: false,
            } => write_frame(w, kind::SPAWN_SHELL, &to_json(&(cwd, size, shell))?),
            ClientMsg::Spawn {
                cwd,
                size,
                shell,
                owner,
                restore,
                allow_remote_clipboard_write,
            } => write_frame(
                w,
                kind::SPAWN_OWNED,
                &to_json(&OwnedSpawn {
                    cwd: cwd.clone(),
                    size: *size,
                    shell: shell.clone(),
                    owner: owner.clone(),
                    restore: restore.clone(),
                    allow_remote_clipboard_write: *allow_remote_clipboard_write,
                })?,
            ),
            ClientMsg::Attach {
                pane_id,
                size,
                allow_remote_clipboard_write,
            } => write_frame(
                w,
                kind::ATTACH,
                &to_json(&(pane_id, size, allow_remote_clipboard_write))?,
            ),
            ClientMsg::Observe { pane_id, size } => {
                write_frame(w, kind::OBSERVE, &to_json(&(pane_id, size))?)
            }
            ClientMsg::Input(bytes) => write_frame(w, kind::INPUT, bytes),
            ClientMsg::SendInput { pane_id, bytes } => {
                write_frame(w, kind::SEND_INPUT, &to_json(&(pane_id, bytes))?)
            }
            ClientMsg::AgentEvent {
                pane_id,
                pid,
                event,
            } => write_frame(w, kind::AGENT_EVENT, &to_json(&(pane_id, pid, event))?),
            ClientMsg::Resize(size) => write_frame(w, kind::RESIZE, &to_json(size)?),
            ClientMsg::ClearScrollback => write_frame(w, kind::CLEAR_SCROLLBACK, &[]),
            ClientMsg::Detach => write_frame(w, kind::DETACH, &[]),
            ClientMsg::Kill { pane_id } => write_frame(w, kind::KILL, &to_json(pane_id)?),
            ClientMsg::List => write_frame(w, kind::LIST, &[]),
            ClientMsg::Shutdown => write_frame(w, kind::SHUTDOWN, &[]),
            ClientMsg::Handoff { exe } => write_frame(w, kind::HANDOFF, &to_json(exe)?),
            ClientMsg::Lease(request) => write_frame(w, kind::LEASE, &to_json(request)?),
            ClientMsg::EnsureLoopbackForward(req) => {
                write_frame(w, kind::ENSURE_LOOPBACK_FORWARD, &to_json(req)?)
            }
            ClientMsg::SpawnNativeSsh { cwd, size, spec } => {
                write_frame(w, kind::SPAWN_NATIVE_SSH, &to_json(&(cwd, size, spec))?)
            }
            ClientMsg::AuthResponse {
                request_id,
                response,
            } => write_frame(w, kind::AUTH_RESPONSE, &to_json(&(request_id, response))?),
            ClientMsg::ListKnownHosts => write_frame(w, kind::LIST_KNOWN_HOSTS, &[]),
            ClientMsg::DeleteKnownHost(id) => {
                write_frame(w, kind::DELETE_KNOWN_HOST, &to_json(id)?)
            }
            ClientMsg::TestSsh { spec } => write_frame(w, kind::TEST_SSH, &to_json(spec)?),
            ClientMsg::SftpList { pane_id, path } => {
                write_frame(w, kind::SFTP_LIST, &to_json(&(pane_id, path))?)
            }
            ClientMsg::SftpOp { pane_id, op } => {
                write_frame(w, kind::SFTP_OP, &to_json(&(pane_id, op))?)
            }
            ClientMsg::SftpTransferStart(spec) => {
                write_frame(w, kind::SFTP_TRANSFER_START, &to_json(spec)?)
            }
            ClientMsg::SftpTransferCancel { job_id } => {
                write_frame(w, kind::SFTP_TRANSFER_CANCEL, &to_json(job_id)?)
            }
            ClientMsg::SftpTransferList { pane_id } => {
                write_frame(w, kind::SFTP_TRANSFER_LIST, &to_json(pane_id)?)
            }
            ClientMsg::AddForward { pane_id, rule } => {
                write_frame(w, kind::ADD_FORWARD, &to_json(&(pane_id, rule))?)
            }
            ClientMsg::RemoveForward {
                pane_id,
                forward_id,
            } => write_frame(w, kind::REMOVE_FORWARD, &to_json(&(pane_id, forward_id))?),
            ClientMsg::SetForwardEnabled {
                pane_id,
                forward_id,
                enabled,
            } => write_frame(
                w,
                kind::SET_FORWARD_ENABLED,
                &to_json(&(pane_id, forward_id, enabled))?,
            ),
            ClientMsg::QueryProcs { pane_id } => {
                write_frame(w, kind::QUERY_PROCS, &to_json(pane_id)?)
            }
            ClientMsg::ListForwards { pane_id } => {
                write_frame(w, kind::LIST_FORWARDS, &to_json(pane_id)?)
            }
            ClientMsg::Version => write_frame(w, kind::VERSION, &[]),
        }
    }

    pub fn from_frame(k: u8, payload: Vec<u8>) -> io::Result<Self> {
        Ok(match k {
            kind::SPAWN => {
                let (cwd, size) = from_json(&payload)?;
                ClientMsg::Spawn {
                    cwd,
                    size,
                    shell: None,
                    owner: None,
                    restore: None,
                    allow_remote_clipboard_write: false,
                }
            }
            kind::SPAWN_SHELL => {
                let (cwd, size, shell) = from_json(&payload)?;
                ClientMsg::Spawn {
                    cwd,
                    size,
                    shell,
                    owner: None,
                    restore: None,
                    allow_remote_clipboard_write: false,
                }
            }
            kind::SPAWN_OWNED => {
                let OwnedSpawn {
                    cwd,
                    size,
                    shell,
                    owner,
                    restore,
                    allow_remote_clipboard_write,
                } = from_json(&payload)?;
                ClientMsg::Spawn {
                    cwd,
                    size,
                    shell,
                    owner,
                    restore,
                    allow_remote_clipboard_write,
                }
            }
            kind::ATTACH => {
                let (pane_id, size, allow_remote_clipboard_write) = from_json(&payload)?;
                ClientMsg::Attach {
                    pane_id,
                    size,
                    allow_remote_clipboard_write,
                }
            }
            kind::OBSERVE => {
                let (pane_id, size) = from_json(&payload)?;
                ClientMsg::Observe { pane_id, size }
            }
            kind::INPUT => ClientMsg::Input(payload),
            kind::SEND_INPUT => {
                let (pane_id, bytes) = from_json(&payload)?;
                ClientMsg::SendInput { pane_id, bytes }
            }
            kind::AGENT_EVENT => {
                let (pane_id, pid, event) = from_json(&payload)?;
                ClientMsg::AgentEvent {
                    pane_id,
                    pid,
                    event,
                }
            }
            kind::RESIZE => ClientMsg::Resize(from_json(&payload)?),
            kind::CLEAR_SCROLLBACK => ClientMsg::ClearScrollback,
            kind::DETACH => ClientMsg::Detach,
            kind::KILL => ClientMsg::Kill {
                pane_id: from_json(&payload)?,
            },
            kind::LIST => ClientMsg::List,
            kind::SHUTDOWN => ClientMsg::Shutdown,
            kind::LEASE => ClientMsg::Lease(from_json(&payload)?),
            kind::HANDOFF => ClientMsg::Handoff {
                exe: from_json(&payload)?,
            },
            kind::ENSURE_LOOPBACK_FORWARD => ClientMsg::EnsureLoopbackForward(from_json(&payload)?),
            kind::SPAWN_NATIVE_SSH => {
                let (cwd, size, spec) = from_json(&payload)?;
                ClientMsg::SpawnNativeSsh { cwd, size, spec }
            }
            kind::AUTH_RESPONSE => {
                let (request_id, response) = from_json(&payload)?;
                ClientMsg::AuthResponse {
                    request_id,
                    response,
                }
            }
            kind::LIST_KNOWN_HOSTS => ClientMsg::ListKnownHosts,
            kind::DELETE_KNOWN_HOST => ClientMsg::DeleteKnownHost(from_json(&payload)?),
            kind::TEST_SSH => ClientMsg::TestSsh {
                spec: from_json(&payload)?,
            },
            kind::SFTP_LIST => {
                let (pane_id, path) = from_json(&payload)?;
                ClientMsg::SftpList { pane_id, path }
            }
            kind::SFTP_OP => {
                let (pane_id, op) = from_json(&payload)?;
                ClientMsg::SftpOp { pane_id, op }
            }
            kind::SFTP_TRANSFER_START => ClientMsg::SftpTransferStart(from_json(&payload)?),
            kind::SFTP_TRANSFER_CANCEL => ClientMsg::SftpTransferCancel {
                job_id: from_json(&payload)?,
            },
            kind::SFTP_TRANSFER_LIST => ClientMsg::SftpTransferList {
                pane_id: from_json(&payload)?,
            },
            kind::QUERY_PROCS => ClientMsg::QueryProcs {
                pane_id: from_json(&payload)?,
            },
            kind::ADD_FORWARD => {
                let (pane_id, rule) = from_json(&payload)?;
                ClientMsg::AddForward { pane_id, rule }
            }
            kind::REMOVE_FORWARD => {
                let (pane_id, forward_id) = from_json(&payload)?;
                ClientMsg::RemoveForward {
                    pane_id,
                    forward_id,
                }
            }
            kind::SET_FORWARD_ENABLED => {
                let (pane_id, forward_id, enabled) = from_json(&payload)?;
                ClientMsg::SetForwardEnabled {
                    pane_id,
                    forward_id,
                    enabled,
                }
            }
            kind::LIST_FORWARDS => ClientMsg::ListForwards {
                pane_id: from_json(&payload)?,
            },
            kind::VERSION => ClientMsg::Version,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown ClientMsg kind {other}"),
                ));
            }
        })
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let (k, payload) = read_frame(r)?;
        Self::from_frame(k, payload)
    }
}

impl DaemonMsg {
    pub fn encode<W: Write>(&self, w: &mut W) -> io::Result<()> {
        match self {
            DaemonMsg::Spawned { pane_id } => write_frame(w, kind::SPAWNED, &to_json(pane_id)?),
            DaemonMsg::Size(size) => write_frame(w, kind::SIZE, &to_json(size)?),
            DaemonMsg::Snapshot(bytes) => write_frame(w, kind::SNAPSHOT, bytes),
            DaemonMsg::Output(bytes) => write_frame(w, kind::OUTPUT, bytes),
            DaemonMsg::Image(frame) => write_frame(w, kind::IMAGE, frame),
            DaemonMsg::DeleteImage(sel) => write_frame(w, kind::DELETE_IMAGE, sel),
            DaemonMsg::ClipboardWrite(frame) => write_frame(w, kind::CLIPBOARD_WRITE, frame),
            DaemonMsg::Lease(by) => write_frame(w, kind::LEASE_HOLDER, &to_json(by)?),
            DaemonMsg::Cwd(path) => write_frame(w, kind::CWD, &to_json(path)?),
            DaemonMsg::Prompt {
                active,
                at_prompt,
                last_exit,
            } => write_frame(w, kind::PROMPT, &to_json(&(active, at_prompt, last_exit))?),
            DaemonMsg::Exited { code } => write_frame(w, kind::EXITED, &to_json(code)?),
            DaemonMsg::PaneList(list) => write_frame(w, kind::PANE_LIST, &to_json(list)?),
            DaemonMsg::InputAck { pane_id } => write_frame(w, kind::INPUT_ACK, &to_json(pane_id)?),
            DaemonMsg::RemoteContext(remote) => {
                write_frame(w, kind::REMOTE_CONTEXT, &to_json(remote)?)
            }
            DaemonMsg::Agent(agent) => write_frame(w, kind::AGENT, &to_json(agent)?),
            DaemonMsg::AgentStatus(state) => write_frame(w, kind::AGENT_STATUS, &to_json(state)?),
            DaemonMsg::LoopbackForward(forward) => {
                write_frame(w, kind::LOOPBACK_FORWARD, &to_json(forward)?)
            }
            DaemonMsg::AuthPrompt { request_id, prompt } => {
                write_frame(w, kind::AUTH_PROMPT, &to_json(&(request_id, prompt))?)
            }
            DaemonMsg::SshStatus { phase } => write_frame(w, kind::SSH_STATUS, &to_json(phase)?),
            DaemonMsg::KnownHostsList(list) => {
                write_frame(w, kind::KNOWN_HOSTS_LIST, &to_json(list)?)
            }
            DaemonMsg::SshTestResult(report) => {
                write_frame(w, kind::SSH_TEST_RESULT, &to_json(report)?)
            }
            DaemonMsg::SftpEntries(entries) => {
                write_frame(w, kind::SFTP_ENTRIES, &to_json(entries)?)
            }
            DaemonMsg::SftpOpResult(result) => {
                write_frame(w, kind::SFTP_OP_RESULT, &to_json(result)?)
            }
            DaemonMsg::SftpTransferStarted { job_id } => {
                write_frame(w, kind::SFTP_TRANSFER_STARTED, &to_json(job_id)?)
            }
            DaemonMsg::SftpTransferProgress(jobs) => {
                write_frame(w, kind::SFTP_TRANSFER_PROGRESS, &to_json(jobs)?)
            }
            DaemonMsg::ForwardList(list) => write_frame(w, kind::FORWARD_LIST, &to_json(list)?),
            DaemonMsg::Procs(procs) => write_frame(w, kind::PROCS, &to_json(procs)?),
            DaemonMsg::Version(version) => write_frame(w, kind::VERSION_REPLY, &to_json(version)?),
            DaemonMsg::Error(msg) => write_frame(w, kind::ERROR, &to_json(msg)?),
        }
    }

    pub fn from_frame(k: u8, payload: Vec<u8>) -> io::Result<Self> {
        Ok(match k {
            kind::SPAWNED => DaemonMsg::Spawned {
                pane_id: from_json(&payload)?,
            },
            kind::SIZE => DaemonMsg::Size(from_json(&payload)?),
            kind::SNAPSHOT => DaemonMsg::Snapshot(payload),
            kind::OUTPUT => DaemonMsg::Output(payload),
            kind::IMAGE => DaemonMsg::Image(payload),
            kind::DELETE_IMAGE => DaemonMsg::DeleteImage(payload),
            kind::CLIPBOARD_WRITE => DaemonMsg::ClipboardWrite(payload),
            kind::LEASE_HOLDER => DaemonMsg::Lease(from_json(&payload)?),
            kind::CWD => DaemonMsg::Cwd(from_json(&payload)?),
            kind::PROMPT => {
                let (active, at_prompt, last_exit) = from_json(&payload)?;
                DaemonMsg::Prompt {
                    active,
                    at_prompt,
                    last_exit,
                }
            }
            kind::EXITED => DaemonMsg::Exited {
                code: from_json(&payload)?,
            },
            kind::PANE_LIST => DaemonMsg::PaneList(from_json(&payload)?),
            kind::INPUT_ACK => DaemonMsg::InputAck {
                pane_id: from_json(&payload)?,
            },
            kind::REMOTE_CONTEXT => DaemonMsg::RemoteContext(from_json(&payload)?),
            kind::AGENT => DaemonMsg::Agent(from_json(&payload)?),
            kind::AGENT_STATUS => DaemonMsg::AgentStatus(from_json(&payload)?),
            kind::LOOPBACK_FORWARD => DaemonMsg::LoopbackForward(from_json(&payload)?),
            kind::AUTH_PROMPT => {
                let (request_id, prompt) = from_json(&payload)?;
                DaemonMsg::AuthPrompt { request_id, prompt }
            }
            kind::SSH_STATUS => DaemonMsg::SshStatus {
                phase: from_json(&payload)?,
            },
            kind::KNOWN_HOSTS_LIST => DaemonMsg::KnownHostsList(from_json(&payload)?),
            kind::SSH_TEST_RESULT => DaemonMsg::SshTestResult(from_json(&payload)?),
            kind::SFTP_ENTRIES => DaemonMsg::SftpEntries(from_json(&payload)?),
            kind::SFTP_OP_RESULT => DaemonMsg::SftpOpResult(from_json(&payload)?),
            kind::SFTP_TRANSFER_STARTED => DaemonMsg::SftpTransferStarted {
                job_id: from_json(&payload)?,
            },
            kind::SFTP_TRANSFER_PROGRESS => DaemonMsg::SftpTransferProgress(from_json(&payload)?),
            kind::FORWARD_LIST => DaemonMsg::ForwardList(from_json(&payload)?),
            kind::PROCS => DaemonMsg::Procs(from_json(&payload)?),
            kind::VERSION_REPLY => DaemonMsg::Version(from_json(&payload)?),
            kind::ERROR => DaemonMsg::Error(from_json(&payload)?),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown DaemonMsg kind {other}"),
                ));
            }
        })
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let (k, payload) = read_frame(r)?;
        Self::from_frame(k, payload)
    }
}
