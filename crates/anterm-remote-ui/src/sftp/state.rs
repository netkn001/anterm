use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AppContext as _, Context, Entity, Pixels, Size, Subscription, Window};
use gpui_kit::component::VirtualListScrollHandle;
use gpui_kit::component::input::InputState;

use anterm_core::daemon::protocol::{
    SftpEntry, SftpEntryKind, SftpJobProgress, SftpJobState,
};
use anterm_ui_kit::host_ops::SharedHost;
use anterm_ui_kit::intent::WorkspaceIntent;

/// Outbound events emitted by [`SftpPanelView`] so the panel owns its state
/// and rendering without reaching into `AntermApp`.
#[derive(Clone)]
pub enum SftpEvent {
    /// User clicked a remote file (or chose Edit from its menu) to open it in
    /// the workspace code editor.
    OpenRemoteFile { host: SharedHost, path: PathBuf },
    /// User asked to jump the SFTP browser to the active SSH pane's shell cwd.
    GotoShellCwd { pane_id: u64 },
    /// Workspace-level navigation or focus intent (e.g. restoring terminal
    /// focus after closing an inline edit form).
    Intent(WorkspaceIntent),
}

#[derive(Clone, Copy)]
pub(super) enum SftpMenuAction {
    NewFolder,
    NewFile,
    Upload,
    GotoShellCwd,
    ToggleHistory,
}

pub enum SftpEdit {
    NewFolder(Entity<InputState>),
    NewFile(Entity<InputState>),
    Rename {
        original: String,
        input: Entity<InputState>,
    },
    Chmod {
        path: String,
        readable: String,
        input: Entity<InputState>,
    },
}

pub use anterm_ui_kit::daemon_rpc::{SftpRoute, free_local_path, local_download_dir};

pub struct SftpPanelView {
    pub(super) open_pane_id: Option<u64>,
    pub(super) host_label: String,
    pub(super) cwd: String,
    pub(super) cwds: HashMap<u64, String>,
    /// The side panel has been closed since the browser last opened, so the
    /// next open is the user asking to look at files again rather than the
    /// browser following a pane switch. See [`sftp_start_dir`].
    pub(super) panel_was_closed: bool,
    pub(super) entries: Vec<SftpEntry>,
    pub(super) entries_rev: u64,
    pub(super) filtered_indices: Vec<usize>,
    pub(super) filtered_key: (u64, String),
    pub(super) item_sizes: Rc<Vec<Size<Pixels>>>,
    pub(super) filter_input: Entity<InputState>,
    pub(super) error: Option<String>,
    pub(super) jobs: Vec<SftpJobProgress>,
    /// Why the last transfer poll came back empty-handed, if it did.
    ///
    /// Kept apart from `error`, which blanks the directory listing: a poll
    /// that could not reach the daemon says nothing about the listing already
    /// on screen, and the transfer tray is the only place it belongs.
    pub(super) jobs_error: Option<String>,
    /// Uploads this panel started whose landing it has not listed yet.
    ///
    /// An upload is written to `<name>.anterm-upload-<hex>` and renamed into
    /// place at the very end, so any listing taken while one is in flight
    /// shows the temporary name. These are the jobs a listing is owed to.
    pub(super) uploads_awaiting_listing: HashSet<u64>,
    /// Local names handed to a download that has not created its file yet.
    /// Two quick downloads of the same remote file would otherwise both find
    /// the name free and the second would write over the first.
    pub(super) claimed_downloads: HashSet<PathBuf>,
    pub(super) dismissed_jobs: HashSet<u64>,
    pub(super) show_history: bool,
    pub(super) tray_expanded: bool,
    pub(super) loading: bool,
    pub(super) nav_gen: u64,
    pub(super) editing: Option<SftpEdit>,
    pub(super) editing_sub: Vec<Subscription>,
    pub(super) editing_path: Option<Entity<InputState>>,
    pub(super) editing_path_sub: Vec<Subscription>,
    pub(super) poll_gen: u64,
    pub(super) scroll: VirtualListScrollHandle,
    pub(super) transfers_scroll: gpui::ScrollHandle,
    _subs: Vec<Subscription>,
}

impl SftpPanelView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(anterm_i18n::t(anterm_i18n::L10nKey::SearchFiles))
        });
        let sub = cx.subscribe_in(&filter_input, window, |_this, _input, ev, _w, cx| {
            if matches!(ev, gpui_kit::component::input::InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            open_pane_id: None,
            host_label: String::new(),
            cwd: "/".to_string(),
            cwds: HashMap::new(),
            panel_was_closed: true,
            entries: Vec::new(),
            entries_rev: 0,
            filtered_indices: Vec::new(),
            filtered_key: (u64::MAX, String::new()),
            item_sizes: Rc::new(Vec::new()),
            filter_input,
            error: None,
            jobs: Vec::new(),
            jobs_error: None,
            uploads_awaiting_listing: HashSet::new(),
            claimed_downloads: HashSet::new(),
            dismissed_jobs: HashSet::new(),
            show_history: false,
            tray_expanded: false,
            loading: false,
            nav_gen: 0,
            editing: None,
            editing_sub: Vec::new(),
            editing_path: None,
            editing_path_sub: Vec::new(),
            poll_gen: 0,
            scroll: VirtualListScrollHandle::new(),
            transfers_scroll: gpui::ScrollHandle::new(),
            _subs: vec![sub],
        }
    }
}

/// Of the uploads a listing is owed to, the ones that are still writing.
///
/// A job that has dropped off the list entirely counts as done: whether it
/// finished, failed or was trimmed from the history, it is not going to rename
/// anything into place later.
pub(super) fn uploads_still_running(
    owed: &HashSet<u64>,
    jobs: &[SftpJobProgress],
) -> HashSet<u64> {
    owed.iter()
        .copied()
        .filter(|id| {
            jobs.iter()
                .find(|job| job.job_id == *id)
                .is_some_and(|job| job.state == SftpJobState::Running)
        })
        .collect()
}

/// What the tray shows after a poll: the jobs to draw, and the failure to say
/// out loud beside them.
///
/// A poll that failed used to come back as an empty `Vec`, which reads as "the
/// transfers are all gone" — the tray disappeared and every upload the panel
/// was waiting on counted as landed. Over a link that is down that is not a
/// blink but the permanent answer, so a failure keeps the previous list and is
/// reported instead of replacing it.
pub(super) fn apply_poll(
    previous: Vec<SftpJobProgress>,
    reply: Result<Vec<SftpJobProgress>, String>,
) -> (Vec<SftpJobProgress>, Option<String>) {
    match reply {
        Ok(jobs) => (jobs, None),
        Err(e) => (previous, Some(e)),
    }
}

pub(super) fn is_dir_like(e: &SftpEntry) -> bool {
    matches!(e.kind, SftpEntryKind::Dir)
        || (matches!(e.kind, SftpEntryKind::Symlink) && e.target_is_dir)
}

#[cfg(test)]
pub fn sorted_filtered_entries<'a>(
    entries: &'a [SftpEntry],
    filter: &str,
) -> Vec<&'a SftpEntry> {
    let needle = filter.to_lowercase();
    let mut out: Vec<&SftpEntry> = entries
        .iter()
        .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
        .collect();
    out.sort_by(|a, b| {
        let (ad, bd) = (is_dir_like(a), is_dir_like(b));
        bd.cmp(&ad)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

pub(super) fn sorted_filtered_indices(entries: &[SftpEntry], filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    let mut out: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| needle.is_empty() || e.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    out.sort_by(|&ia, &ib| {
        let (a, b) = (&entries[ia], &entries[ib]);
        let (ad, bd) = (is_dir_like(a), is_dir_like(b));
        bd.cmp(&ad)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

pub fn breadcrumb_segments(path: &str) -> Vec<(String, String)> {
    let mut out = vec![("/".to_string(), "/".to_string())];
    let mut acc = String::new();
    for comp in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(comp);
        out.push((comp.to_string(), acc.clone()));
    }
    out
}

pub(super) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

pub(super) fn mode_string(mode: u32) -> String {
    let rwx = |bits: u32| {
        format!(
            "{}{}{}",
            if bits & 0o4 != 0 { 'r' } else { '-' },
            if bits & 0o2 != 0 { 'w' } else { '-' },
            if bits & 0o1 != 0 { 'x' } else { '-' },
        )
    };
    format!(
        "{}{}{}",
        rwx((mode >> 6) & 0o7),
        rwx((mode >> 3) & 0o7),
        rwx(mode & 0o7)
    )
}



/// Where the browser starts when it opens on a pane.
///
/// Opening the panel afresh is a request to see where the shell is now, so the
/// shell's directory wins over wherever the browser was left last time. When
/// the browser instead follows a pane switch with the panel still open, the
/// user never left it, so the directory they were browsing there comes back.
/// Either falls back to the other; `None` means neither is known and the
/// caller goes to the login directory.
pub(super) fn sftp_start_dir(
    fresh_open: bool,
    shell_cwd: Option<String>,
    remembered: Option<String>,
) -> Option<String> {
    if fresh_open {
        shell_cwd.or(remembered)
    } else {
        remembered.or(shell_cwd)
    }
}
