use std::path::PathBuf;

use anterm_core::daemon::protocol::{
    ClientMsg, DaemonMsg, KnownHostEntry, KnownHostId, LoopbackForward, LoopbackForwardRequest,
    ManagedForward, NativeSshSpec, PaneInfo, PaneProcs, SftpEntry, SftpJobProgress, SftpOp,
    SftpOpResult, SftpTransferSpec, SshForwardRule, SshTestReport,
};
use crate::backend::size::TermSize;

use super::{
    PaneRoute, PtySource, RemoteTerminal, attach_reply_wait, connect,
    connect_routed, daemon_disconnected_before_spawn_reply, spawn_reply, win_size,
};

impl RemoteTerminal {
    pub fn list_panes() -> Vec<PaneInfo> {
        Self::list_panes_on(&PaneRoute::Local)
    }

    pub fn list_panes_on(route: &PaneRoute) -> Vec<PaneInfo> {
        Self::try_list_panes_on(route).unwrap_or_default()
    }

    pub fn try_list_panes_on(route: &PaneRoute) -> anyhow::Result<Vec<PaneInfo>> {
        let mut stream = connect_routed(route)?;
        ClientMsg::List.encode(&mut stream)?;
        match DaemonMsg::read(&mut stream)? {
            DaemonMsg::PaneList(list) => Ok(list),
            other => Err(anyhow::anyhow!("unexpected reply to List: {other:?}")),
        }
    }

    pub fn kill_pane(pane_id: u64) {
        Self::kill_pane_on(&PaneRoute::Local, pane_id)
    }

    pub fn kill_pane_on(route: &PaneRoute, pane_id: u64) {
        if let Ok(mut stream) = connect_routed(route) {
            let _ = ClientMsg::Kill { pane_id }.encode(&mut stream);
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
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

    /// `remote_start_dir` is a directory on the far host, carried as
    /// [`ClientMsg::SpawnNativeSsh`]'s `cwd`.
    pub fn spawn_native_ssh(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        remote_start_dir: Option<PathBuf>,
        spec: Box<NativeSshSpec>,
    ) -> anyhow::Result<(Self, u64)> {
        match Self::spawn_native_ssh_once(
            size,
            cell_w,
            cell_h,
            remote_start_dir.clone(),
            spec.clone(),
        ) {
            Err(first_err) if daemon_disconnected_before_spawn_reply(&first_err) => {
                log::warn!(
                    "spawn connection ended before reply; probing daemon without forcing a restart: {first_err:#}"
                );
                if let Err(restart_err) = anterm_core::daemon::spawn::ensure_running() {
                    return Err(anyhow::anyhow!(
                        "daemon disconnected before SpawnNativeSsh reply ({first_err}); recovery failed: {restart_err}"
                    ));
                }
                Self::spawn_native_ssh_once(size, cell_w, cell_h, remote_start_dir, spec).map_err(
                    |second_err| {
                        anyhow::anyhow!(
                            "daemon disconnected before SpawnNativeSsh reply ({first_err}); recovery attempted but it still failed: {second_err}"
                        )
                    },
                )
            }
            other => other,
        }
    }

    fn spawn_native_ssh_once(
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        remote_start_dir: Option<PathBuf>,
        spec: Box<NativeSshSpec>,
    ) -> anyhow::Result<(Self, u64)> {
        let mut stream = connect()?;
        let win = win_size(size, cell_w, cell_h);
        let endpoint = (spec.host.clone(), spec.port);
        let user = spec.user.clone();
        let auto_supplied_password = spec.password.is_some();

        ClientMsg::SpawnNativeSsh {
            cwd: remote_start_dir,
            size: win,
            spec,
        }
        .encode(&mut stream)?;
        // Native SSH is always dialled through the local daemon, whatever the
        // far end turns out to be, so this waits on the local budget.
        let pane_id = match spawn_reply(
            &mut stream,
            attach_reply_wait(&PaneRoute::Local),
            "SpawnNativeSsh",
        )? {
            DaemonMsg::Spawned { pane_id } => pane_id,
            DaemonMsg::Error(msg) => {
                return Err(anyhow::anyhow!("daemon refused SpawnNativeSsh: {msg}"));
            }
            other => {
                return Err(anyhow::anyhow!(
                    "unexpected daemon reply to SpawnNativeSsh: {other:?}"
                ));
            }
        };

        // The local daemon dialled this one, but it opened no pty for it: the
        // pane is an ssh channel bridged straight through, so the bytes are the
        // far host's raw pty and no conhost ever sees them.
        let mut term = Self::from_stream_with(stream, size, Vec::new(), PtySource::Raw)?;
        term.synced_size = true;
        term.synced_cell = (cell_w, cell_h);
        term.ssh_endpoint = Some(endpoint);
        term.ssh_user = Some(user);
        term.auto_supplied_password = auto_supplied_password;
        Ok((term, pane_id))
    }

    /// The client half of known-hosts management.
    ///
    /// Nothing calls this yet: there is no known-hosts surface in the window or
    /// the CLI. What sits behind it is not a stub, though — `ssh::known_hosts`
    /// parses the real file, fingerprints each key, and rewrites through a
    /// 0600 temp file — so this is an interface waiting for a screen, not
    /// scaffolding around nothing. Deleting it would throw away the finished
    /// half of the feature.
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

    /// Ask the daemon to dial this spec and say what happened. Blocking, and
    /// bounded by the spec's own connect timeout on the far side — call it off
    /// the UI thread.
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

    /// See [`Self::list_known_hosts`] — same story, and it returns the list
    /// after the removal so a caller can redraw from one round trip.
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

    /// A failed poll is not an empty transfer list: the caller has to be able
    /// to keep the jobs it already knows about, so this reports the failure
    /// the way `sftp_list` does rather than answering with an empty `Vec`.
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

    /// `None` when the request never got a list back — which is not the same
    /// as getting an empty one, because only the caller of a *failed* request
    /// still has to keep showing what it had.
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

    /// `None` when the request never got a list back — see `add_forward`.
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

    /// The list after switching one forward on or off, or why it was not
    /// switched — the daemon's own refusal, which names what is in the way.
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
}
