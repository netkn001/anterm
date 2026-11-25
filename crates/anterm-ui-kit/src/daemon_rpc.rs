//! Stateless daemon RPC client helpers and shared route value types
//! (`ForwardRoute`, `PlaceOutcome`, `PortRoute`, `SftpRoute`).
//!
//! Lives in `anterm-ui-kit` so `anterm-remote-ui`, `anterm-files-ui`,
//! `anterm-terminal`, and `anterm-app` share daemon IPC without coupling to
//! `RemoteTerminal` or `AntermApp`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anterm_core::daemon::protocol::{
    ClientMsg, DaemonMsg, KnownHostEntry, KnownHostId, LoopbackForward, LoopbackForwardRequest,
    ManagedForward, NativeSshSpec, PaneProcs, SftpEntry, SftpJobProgress, SftpOp, SftpOpResult,
    SftpTransferSpec, SshForwardRule, SshTestReport,
};
use anterm_core::daemon::transport::{self, Stream};

pub fn connect() -> anyhow::Result<Stream> {
    transport::connect().map_err(|e| {
        anyhow::Error::new(e).context(format!(
            "connect to daemon at {}",
            transport::endpoint_display()
        ))
    })
}

pub fn ensure_loopback_forward(
    pane_id: u64,
    remote_host: &str,
    remote_port: u16,
) -> anyhow::Result<LoopbackForward> {
    let mut stream = connect()?;
    ClientMsg::EnsureLoopbackForward(LoopbackForwardRequest {
        pane_id,
        remote_host: remote_host.to_string(),
        remote_port,
    })
    .encode(&mut stream)?;
    match DaemonMsg::read(&mut stream)? {
        DaemonMsg::LoopbackForward(forward) => Ok(forward),
        DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
        other => Err(anyhow::anyhow!(
            "unexpected reply to EnsureLoopbackForward: {other:?}"
        )),
    }
}

pub fn list_known_hosts() -> Vec<KnownHostEntry> {
    fn query() -> anyhow::Result<Vec<KnownHostEntry>> {
        let mut stream = connect()?;
        ClientMsg::ListKnownHosts.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::KnownHostsList(list) => Ok(list),
            other => Err(anyhow::anyhow!(
                "unexpected reply to ListKnownHosts: {other:?}"
            )),
        }
    }
    query().unwrap_or_default()
}

pub fn test_ssh(spec: Box<NativeSshSpec>) -> SshTestReport {
    fn query(spec: Box<NativeSshSpec>) -> anyhow::Result<SshTestReport> {
        let mut stream = connect()?;
        ClientMsg::TestSsh { spec }.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::SshTestResult(report) => Ok(report),
            other => Err(anyhow::anyhow!("unexpected reply to TestSsh: {other:?}")),
        }
    }
    query(spec).unwrap_or_else(|e| SshTestReport::Failed {
        reason: e.to_string(),
    })
}

pub fn delete_known_host(id: KnownHostId) -> Vec<KnownHostEntry> {
    fn query(id: KnownHostId) -> anyhow::Result<Vec<KnownHostEntry>> {
        let mut stream = connect()?;
        ClientMsg::DeleteKnownHost(id).encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::KnownHostsList(list) => Ok(list),
            other => Err(anyhow::anyhow!(
                "unexpected reply to DeleteKnownHost: {other:?}"
            )),
        }
    }
    query(id).unwrap_or_default()
}

pub fn sftp_list(pane_id: u64, path: &str) -> Result<Vec<SftpEntry>, String> {
    fn query(pane_id: u64, path: String) -> anyhow::Result<Result<Vec<SftpEntry>, String>> {
        let mut stream = connect()?;
        ClientMsg::SftpList { pane_id, path }.encode(&mut stream)?;
        Ok(match DaemonMsg::read(&mut stream)? {
            DaemonMsg::SftpEntries(entries) => Ok(entries),
            DaemonMsg::Error(msg) => Err(msg),
            other => Err(format!("unexpected reply to SftpList: {other:?}")),
        })
    }
    query(pane_id, path.to_string()).unwrap_or_else(|e| Err(e.to_string()))
}

pub fn sftp_op(pane_id: u64, op: SftpOp) -> SftpOpResult {
    fn query(pane_id: u64, op: SftpOp) -> anyhow::Result<SftpOpResult> {
        let mut stream = connect()?;
        ClientMsg::SftpOp { pane_id, op }.encode(&mut stream)?;
        Ok(match DaemonMsg::read(&mut stream)? {
            DaemonMsg::SftpOpResult(result) => result,
            DaemonMsg::Error(msg) => SftpOpResult::Error(msg),
            other => SftpOpResult::Error(format!("unexpected reply to SftpOp: {other:?}")),
        })
    }
    query(pane_id, op).unwrap_or_else(|e| SftpOpResult::Error(e.to_string()))
}

pub fn sftp_transfer_start(spec: SftpTransferSpec) -> Result<u64, String> {
    fn query(spec: SftpTransferSpec) -> anyhow::Result<Result<u64, String>> {
        let mut stream = connect()?;
        ClientMsg::SftpTransferStart(spec).encode(&mut stream)?;
        Ok(match DaemonMsg::read(&mut stream)? {
            DaemonMsg::SftpTransferStarted { job_id } => Ok(job_id),
            DaemonMsg::Error(msg) => Err(msg),
            other => Err(format!("unexpected reply to SftpTransferStart: {other:?}")),
        })
    }
    query(spec).unwrap_or_else(|e| Err(e.to_string()))
}

pub fn sftp_transfer_cancel(job_id: u64) -> Vec<SftpJobProgress> {
    fn query(job_id: u64) -> anyhow::Result<Vec<SftpJobProgress>> {
        let mut stream = connect()?;
        ClientMsg::SftpTransferCancel { job_id }.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::SftpTransferProgress(jobs) => Ok(jobs),
            other => Err(anyhow::anyhow!(
                "unexpected reply to SftpTransferCancel: {other:?}"
            )),
        }
    }
    query(job_id).unwrap_or_default()
}

pub fn sftp_transfer_list(pane_id: u64) -> Result<Vec<SftpJobProgress>, String> {
    fn query(pane_id: u64) -> anyhow::Result<Result<Vec<SftpJobProgress>, String>> {
        let mut stream = connect()?;
        ClientMsg::SftpTransferList { pane_id }.encode(&mut stream)?;
        Ok(match DaemonMsg::read(&mut stream)? {
            DaemonMsg::SftpTransferProgress(jobs) => Ok(jobs),
            DaemonMsg::Error(msg) => Err(msg),
            other => Err(format!("unexpected reply to SftpTransferList: {other:?}")),
        })
    }
    query(pane_id).unwrap_or_else(|e| Err(e.to_string()))
}

pub fn add_forward(pane_id: u64, rule: SshForwardRule) -> Option<Vec<ManagedForward>> {
    fn query(pane_id: u64, rule: SshForwardRule) -> anyhow::Result<Vec<ManagedForward>> {
        let mut stream = connect()?;
        ClientMsg::AddForward { pane_id, rule }.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::ForwardList(list) => Ok(list),
            DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
            other => Err(anyhow::anyhow!("unexpected reply to AddForward: {other:?}")),
        }
    }
    query(pane_id, rule)
        .inspect_err(|e| log::warn!("AddForward failed: {e}"))
        .ok()
}

pub fn remove_forward(pane_id: u64, forward_id: u64) -> Option<Vec<ManagedForward>> {
    fn query(pane_id: u64, forward_id: u64) -> anyhow::Result<Vec<ManagedForward>> {
        let mut stream = connect()?;
        ClientMsg::RemoveForward {
            pane_id,
            forward_id,
        }
        .encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::ForwardList(list) => Ok(list),
            other => Err(anyhow::anyhow!(
                "unexpected reply to RemoveForward: {other:?}"
            )),
        }
    }
    query(pane_id, forward_id)
        .inspect_err(|e| log::warn!("RemoveForward failed: {e}"))
        .ok()
}

pub fn set_forward_enabled(
    pane_id: u64,
    forward_id: u64,
    enabled: bool,
) -> anyhow::Result<Vec<ManagedForward>> {
    let mut stream = connect()?;
    ClientMsg::SetForwardEnabled {
        pane_id,
        forward_id,
        enabled,
    }
    .encode(&mut stream)?;
    match DaemonMsg::read(&mut stream)? {
        DaemonMsg::ForwardList(list) => Ok(list),
        DaemonMsg::Error(msg) => Err(anyhow::anyhow!(msg)),
        other => Err(anyhow::anyhow!(
            "unexpected reply to SetForwardEnabled: {other:?}"
        )),
    }
}

pub fn list_forwards(pane_id: u64) -> Vec<ManagedForward> {
    fn query(pane_id: u64) -> anyhow::Result<Vec<ManagedForward>> {
        let mut stream = connect()?;
        ClientMsg::ListForwards { pane_id }.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::ForwardList(list) => Ok(list),
            other => Err(anyhow::anyhow!(
                "unexpected reply to ListForwards: {other:?}"
            )),
        }
    }
    query(pane_id).unwrap_or_default()
}

pub fn query_procs(pane_id: u64) -> PaneProcs {
    fn query(pane_id: u64) -> anyhow::Result<PaneProcs> {
        let mut stream = connect()?;
        ClientMsg::QueryProcs { pane_id }.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::Procs(procs) => Ok(procs),
            other => Err(anyhow::anyhow!("unexpected reply to QueryProcs: {other:?}")),
        }
    }
    query(pane_id).unwrap_or_default()
}

/// What it takes to open one of a pane's loopback ports from this machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortRoute {
    /// The port is already reachable here under its own number.
    Direct,
    /// A local forward has to exist first; the pane's `ForwardRoute` builds it.
    Forward,
    /// Another machine's port, with no way to reach it from here.
    #[default]
    Blocked,
}

#[derive(Clone, Debug, Default)]
pub struct ForwardRoute {
    pub pane_id: u64,
}

/// What came of asking the far side to put one forward up.
pub enum PlaceOutcome {
    Placed,
    Rejected(String),
    Unreachable(String),
}

/// Who a set of forwards belongs to on the far side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ForwardOwnerKey {
    Pane(u64),
}

impl ForwardRoute {
    pub fn new(pane_id: u64) -> Self {
        Self { pane_id }
    }

    pub fn owner_key(&self) -> ForwardOwnerKey {
        ForwardOwnerKey::Pane(self.pane_id)
    }

    pub fn list(&self) -> Vec<ManagedForward> {
        list_forwards(self.pane_id)
    }

    pub fn add(&self, rule: SshForwardRule) -> Option<Vec<ManagedForward>> {
        add_forward(self.pane_id, rule)
    }

    pub fn ensure_loopback(
        &self,
        remote_host: &str,
        remote_port: u16,
    ) -> anyhow::Result<LoopbackForward> {
        ensure_loopback_forward(self.pane_id, remote_host, remote_port)
    }

    pub fn set_enabled(
        &self,
        forward_id: u64,
        enabled: bool,
    ) -> anyhow::Result<Vec<ManagedForward>> {
        set_forward_enabled(self.pane_id, forward_id, enabled)
    }

    pub fn remove(&self, forward_id: u64) -> Option<Vec<ManagedForward>> {
        remove_forward(self.pane_id, forward_id)
    }
}

#[derive(Clone, Debug)]
pub struct SftpRoute {
    pub pane_id: u64,
}

impl SftpRoute {
    pub fn new(pane_id: u64) -> Self {
        Self { pane_id }
    }

    pub fn connection_key(&self) -> String {
        format!("sftp:{}", self.pane_id)
    }

    pub fn list(&self, path: &str) -> Result<Vec<SftpEntry>, String> {
        sftp_list(self.pane_id, path)
    }

    pub fn op(&self, op: SftpOp) -> SftpOpResult {
        sftp_op(self.pane_id, op)
    }

    pub fn transfer_start(&self, spec: SftpTransferSpec) -> Result<u64, String> {
        sftp_transfer_start(spec)
    }

    pub fn transfer_list(&self) -> Result<Vec<SftpJobProgress>, String> {
        sftp_transfer_list(self.pane_id)
    }
}

fn local_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn local_download_dir() -> PathBuf {
    local_home().join("Downloads")
}

/// `dir/name`, or the first `dir/name (n)` that is not taken.
pub fn free_local_path(
    dir: &Path,
    name: &str,
    claimed: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    let taken = |p: &PathBuf| p.exists() || claimed.contains(p);
    let first = dir.join(name);
    if !taken(&first) {
        return Some(first);
    }
    let path = Path::new(name);
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    let stem = path
        .file_stem()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    (2..1000u32)
        .map(|n| match &ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        })
        .find(|candidate| !taken(candidate))
}
