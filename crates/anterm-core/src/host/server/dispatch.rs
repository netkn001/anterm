use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::machine;
use crate::daemon::control::{
    CONTROL_VERSION, ControlReply, ControlRequest, ReplyOk, ServerStatus, WireError,
    WireErrorKind, server_started,
};
use crate::host::{Host, SearchHit};

use super::Conn;
#[cfg(windows)]
use super::control_endpoint_path;
#[cfg(unix)]
use super::control_socket_path;

pub(super) fn submit(conn: &Arc<Conn>, req_id: u64, req: ControlRequest, blob: Vec<u8>) {
    conn.inflight
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(req_id, false);

    let job_conn = Arc::clone(conn);
    let queued = conn
        .pool
        .submit(move || run_job(&job_conn, req_id, req, blob));
    if !queued {
        conn.finish(
            req_id,
            ControlReply::Err(WireError::new(
                WireErrorKind::Other,
                "the control server has more requests queued than it will hold",
            )),
            Vec::new(),
            false,
        );
    }
}

fn run_job(conn: &Arc<Conn>, req_id: u64, req: ControlRequest, blob: Vec<u8>) {
    if conn.is_cancelled(req_id) {
        conn.forget(req_id);
        return;
    }

    let wants_blob = req.returns_blob();
    let (reply, out_blob) = match run_request(conn, req_id, req, blob) {
        Ok((ok, bytes)) => (ControlReply::Ok(ok), bytes),
        Err(e) => (ControlReply::Err(WireError::from_io(&e)), Vec::new()),
    };
    conn.finish(req_id, reply, out_blob, wants_blob);
}

pub(super) fn drop_unsendable_hits(hits: &mut Vec<SearchHit>) {
    let before = hits.len();
    hits.retain(|hit| hit.path.to_str().is_some());
    if hits.len() != before {
        log::debug!(
            "search dropped {} hit(s) whose paths are not UTF-8",
            before - hits.len()
        );
    }
}

/// [`drop_unsendable_hits`] for content hits: a path that is not UTF-8 cannot
/// cross the JSON wire intact, and a hit the client cannot open is worse than
/// one it never sees.
fn drop_unsendable_content_hits(hits: &mut Vec<crate::host::ContentHit>) {
    let before = hits.len();
    hits.retain(|hit| hit.path.to_str().is_some());
    if hits.len() != before {
        log::debug!(
            "content search dropped {} hit(s) whose paths are not UTF-8",
            before - hits.len()
        );
    }
}

fn machine_with_live_panes(conn: &Conn) -> io::Result<machine::Machine> {
    let mut machine = conn.machine()?.machine();
    let Some(panes) = conn.panes.as_ref().map(|p| p.panes()) else {
        return Ok(machine);
    };
    for info in panes {
        let record = match machine.panes.iter_mut().find(|p| p.id == info.pane_id) {
            Some(record) => record,
            None => {
                machine.panes.push(machine::PaneRecord::new(info.pane_id));
                machine.panes.last_mut().expect("record was just inserted")
            }
        };
        record.cwd = info.cwd.map(|p| p.to_string_lossy().into_owned());
        record.title = info.title;
        record.osc_title = info.osc_title;
        record.live = info.alive;
    }
    Ok(machine)
}

fn run_request(
    conn: &Arc<Conn>,
    req_id: u64,
    req: ControlRequest,
    blob: Vec<u8>,
) -> io::Result<(ReplyOk, Vec<u8>)> {
    let h: &dyn Host = &*conn.host;
    let p = |s: &str| PathBuf::from(s);

    Ok(match req {
        ControlRequest::Ping => (ReplyOk::Pong, Vec::new()),

        ControlRequest::ReadDir { dir, root } => {
            let root = root.map(|r| p(&r));
            let entries = h.read_dir(&p(&dir), root.as_deref())?;
            (ReplyOk::Entries(entries), Vec::new())
        }
        ControlRequest::Stat { path } => (ReplyOk::Meta(h.stat(&p(&path))?), Vec::new()),
        ControlRequest::Exists { path } => (ReplyOk::Bool(h.exists(&p(&path))), Vec::new()),
        ControlRequest::Canonicalize { path } => {
            let canon = h.canonicalize(&p(&path))?;
            (
                ReplyOk::Path(canon.to_string_lossy().into_owned()),
                Vec::new(),
            )
        }
        ControlRequest::ReadFile { path, max_bytes } => {
            let path = p(&path);
            let bytes = h.read_file(&path, max_bytes)?;
            let meta = h.stat(&path)?;
            (ReplyOk::FileMeta { meta }, bytes)
        }
        ControlRequest::Search {
            roots,
            query,
            limit,
            max_dirs,
            show_hidden,
        } => {
            let roots: Vec<PathBuf> = roots.iter().map(|r| p(r)).collect();
            let mut hits = h.search(
                &roots,
                &query,
                clamp_usize(limit),
                clamp_usize(max_dirs),
                show_hidden,
            )?;
            drop_unsendable_hits(&mut hits);
            (ReplyOk::Hits(hits), Vec::new())
        }
        ControlRequest::SearchContent {
            roots,
            query,
            limits,
        } => {
            let roots: Vec<PathBuf> = roots.iter().map(|r| p(r)).collect();
            let mut found = h.search_content(&roots, &query, &limits)?;
            drop_unsendable_content_hits(&mut found.hits);
            (ReplyOk::ContentHits(found), Vec::new())
        }

        ControlRequest::WriteFile { path } => {
            (ReplyOk::Meta(h.write_file(&p(&path), &blob)?), Vec::new())
        }
        ControlRequest::CreateFileNew { path } => {
            h.create_file_new(&p(&path))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::CreateDir { path, recursive } => {
            h.create_dir(&p(&path), recursive)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::Rename { from, to } => {
            h.rename(&p(&from), &p(&to))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::Remove { path, recursive } => {
            h.remove(&p(&path), recursive)?;
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::RepoRoot { path } => {
            let root = h.repo_root(&p(&path))?;
            (
                ReplyOk::OptPath(root.map(|r| r.to_string_lossy().into_owned())),
                Vec::new(),
            )
        }
        ControlRequest::Git { cwd, args } => {
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            (ReplyOk::Output(h.git(&p(&cwd), &borrowed)?), Vec::new())
        }
        ControlRequest::GitStream { id, cwd, args } => {
            conn.start_git_stream(id, p(&cwd), args);
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::Shells => (ReplyOk::Shells(h.shells()?), Vec::new()),

        ControlRequest::WatchOpen { dirs } => {
            let id = conn.open_watch(req_id, &paths(&dirs))?;
            (ReplyOk::WatchId(id), Vec::new())
        }
        ControlRequest::WatchSet { id, dirs } => {
            conn.set_watch_dirs(id, &paths(&dirs))?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::WatchClose { id } => {
            conn.close_watch(id);
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::GuiOpen { path } => {
            (ReplyOk::Bool(conn.attachments.open_gui(path)), Vec::new())
        }

        ControlRequest::MachineGet => (
            ReplyOk::MachineTree(Box::new(machine_with_live_panes(conn)?)),
            Vec::new(),
        ),
        ControlRequest::SetActiveTab { tab } => {
            conn.machine()?.set_active_tab(tab, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabCreate { at, pane, tab } => (
            ReplyOk::TabTree(Box::new(conn.machine()?.tab_create(
                at.map(clamp_usize),
                pane,
                tab,
                conn.machine_origin,
            )?)),
            Vec::new(),
        ),
        ControlRequest::TabClose { tab } => (
            ReplyOk::Panes(conn.machine()?.tab_close(tab, conn.machine_origin)?),
            Vec::new(),
        ),
        ControlRequest::TabCloseRemembered { tab, panes } => {
            let Some(directory) = conn.panes.as_ref() else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this peer serves no panes, so it cannot keep a closed tab to reopen",
                ));
            };
            let closed = conn.machine()?.tab_close_remembered(
                tab,
                &panes,
                crate::machine::unix_now(),
                conn.machine_origin,
            )?;
            // After the entry is in the tree, for the reason a sleeping tab's
            // panes are stopped after its mark: a pane stopped first reads, to
            // anyone looking in between, as one that died with nothing to
            // restore it for — and the sweeps take its screen.
            for pane in &closed.stopped {
                directory.hibernate_pane(*pane);
            }
            for pane in closed.strays.iter().chain(&closed.dropped) {
                directory.close_pane(*pane);
            }
            (ReplyOk::Panes(closed.stopped), Vec::new())
        }
        ControlRequest::TabReopen { tab } => {
            let (reopened, dropped) = conn.machine()?.tab_reopen(
                tab,
                crate::machine::unix_now(),
                conn.machine_origin,
            )?;
            if let Some(directory) = conn.panes.as_ref() {
                for pane in &dropped {
                    directory.close_pane(*pane);
                }
            }
            (ReplyOk::ReopenedTab(reopened.map(Box::new)), Vec::new())
        }
        ControlRequest::TabRename { tab, name } => {
            conn.machine()?.tab_rename(tab, name, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabMove { tab, to } => {
            conn.machine()?
                .tab_move(tab, clamp_usize(to), conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabSetTags { tab, tags } => {
            conn.machine()?
                .tab_set_tags(tab, tags, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::SetTags { tags } => {
            conn.machine()?.set_tags(tags, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::TabSetHibernated { tab, hibernated } => {
            let Some(directory) = conn.panes.as_ref() else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "this peer serves no panes, so it cannot put a tab to sleep",
                ));
            };
            let panes = conn
                .machine()?
                .tab_set_hibernated(tab, hibernated, conn.machine_origin)?;
            // After the mark, not before: a pane stopped under a tab the tree
            // does not yet call asleep is, to anyone reading the tree in
            // between, a pane that died — and a restore reading it that way
            // would spawn it straight back.
            if hibernated {
                for pane in &panes {
                    directory.hibernate_pane(*pane);
                }
            }
            (ReplyOk::Panes(panes), Vec::new())
        }
        ControlRequest::PaneSplit {
            pane,
            axis,
            ratio,
            new,
            first,
        } => {
            conn.machine()?
                .pane_split(pane, axis, ratio, new, first, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneClose { pane } => (
            ReplyOk::Panes(conn.machine()?.pane_close(pane, conn.machine_origin)?),
            Vec::new(),
        ),
        ControlRequest::PaneSetRatio { tab, path, ratio } => {
            conn.machine()?
                .pane_set_ratio(tab, path, ratio, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneMove {
            pane,
            to,
            axis,
            first,
        } => {
            conn.machine()?
                .pane_move(pane, to, axis, first, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }
        ControlRequest::PaneReplace { old, new } => {
            conn.machine()?
                .pane_replace(old, new, conn.machine_origin)?;
            (ReplyOk::Unit, Vec::new())
        }

        ControlRequest::AgentStates => (
            ReplyOk::AgentStates(
                conn.panes
                    .as_ref()
                    .map(|p| p.agent_states())
                    .unwrap_or_default(),
            ),
            Vec::new(),
        ),
        ControlRequest::PaneProcs { pane_id } => (
            ReplyOk::PaneProcs(
                conn.panes
                    .as_ref()
                    .map(|p| p.pane_procs(pane_id))
                    .unwrap_or_default(),
            ),
            Vec::new(),
        ),
        ControlRequest::Routes => (
            ReplyOk::Routes(crate::daemon::ssh::SshManager::global().routes()),
            Vec::new(),
        ),
        ControlRequest::Status => (
            ReplyOk::Status(ServerStatus {
                pid: std::process::id(),
                uptime_secs: server_started().elapsed().as_secs(),
                panes: conn.panes.as_ref().map(|p| p.pane_count()).unwrap_or(0),
                control_version: CONTROL_VERSION,
                protocol_version: crate::daemon::protocol::PROTOCOL_VERSION,
                build: env!("CARGO_PKG_VERSION").to_string(),
                socket: control_endpoint_display(),
            }),
            Vec::new(),
        ),
    })
}

#[cfg(unix)]
pub(super) fn control_endpoint_display() -> String {
    control_socket_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(windows)]
pub(super) fn control_endpoint_display() -> String {
    control_endpoint_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[cfg(not(any(unix, windows)))]
pub(super) fn control_endpoint_display() -> String {
    String::new()
}

fn paths(v: &[String]) -> Vec<PathBuf> {
    v.iter().map(PathBuf::from).collect()
}

fn clamp_usize(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}
