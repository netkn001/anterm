use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anterm_core::machine::TabId;
use gpui::{Context, Entity};
use gpui_kit::component::input::InputState;

use super::{
    FileTreeView, SEARCH_DEBOUNCE, SEARCH_LIMIT, SEARCH_MAX_DIRS, TreeWrite, land_listing,
};
use anterm_ui_kit::host_ops::{ByHost, HostId, HostOps, SharedHost, WatchSub};

#[derive(Clone, PartialEq)]
pub struct TreeEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub ignored: bool,
}

pub(super) struct Landed {
    pub(super) superseded: bool,
    pub(super) changed: bool,
}

pub struct TreeRow {
    pub entry: TreeEntry,
    pub depth: usize,
    pub is_root: bool,
    pub expanded: bool,
    /// Stands in for the children of `entry` when there are none to draw.
    /// Without it, a directory still being listed, one that is genuinely empty,
    /// one whose contents are all hidden, and one the OS refused to read all
    /// render as the same nothing.
    pub note: Option<TreeNote>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeNote {
    Loading,
    Empty,
    HiddenOnly,
    Unreadable,
    /// The search stopped at `SEARCH_LIMIT`; the list is a prefix, not the
    /// whole answer, and has to say so.
    SearchCapped,
    /// The search never ran to an answer — the host refused it or the link to
    /// it went away. An empty list here means nothing at all.
    SearchFailed,
}

/// `landed` is how many entries the listing returned, or `None` when nothing
/// has come back yet — which is the whole difference between "empty" and
/// "still working". A non-zero count with no visible rows means the hidden
/// filter took them all, and calling that "empty" is a lie the user can
/// disprove with one keystroke.
pub(super) fn dir_note(unreadable: bool, landed: Option<usize>) -> TreeNote {
    match (unreadable, landed) {
        (true, _) => TreeNote::Unreadable,
        (false, None) => TreeNote::Loading,
        (false, Some(0)) => TreeNote::Empty,
        (false, Some(_)) => TreeNote::HiddenOnly,
    }
}

pub enum TreeEdit {
    NewFile {
        dir: PathBuf,
        input: Entity<InputState>,
    },
    NewFolder {
        dir: PathBuf,
        input: Entity<InputState>,
    },
    Rename {
        path: PathBuf,
        input: Entity<InputState>,
    },
}

impl TreeEdit {
    pub(super) fn input(&self) -> &Entity<InputState> {
        match self {
            TreeEdit::NewFile { input, .. }
            | TreeEdit::NewFolder { input, .. }
            | TreeEdit::Rename { input, .. } => input,
        }
    }

    pub(super) fn host_dir(&self) -> &Path {
        match self {
            TreeEdit::NewFile { dir, .. } | TreeEdit::NewFolder { dir, .. } => dir,
            TreeEdit::Rename { path, .. } => path.parent().unwrap_or(path),
        }
    }
}

pub(super) type DirKey = (HostId, PathBuf);

/// Per-tab directory expansion, selection and root paths owned by [`FileTreeView`].
#[derive(Default)]
pub struct TabTreeState {
    pub roots: Vec<PathBuf>,
    pub expanded: HashSet<PathBuf>,
    pub selected: Option<PathBuf>,
}

/// Workspace context pushed into [`FileTreeView`] before rendering.
#[derive(Clone)]
pub struct FileTreeContext {
    pub tab_id: Option<TabId>,
    pub host_id: HostId,
    pub host: Option<SharedHost>,
    pub pane_cwds: Vec<PathBuf>,
    pub active_is_ssh: bool,
    pub display_home: Option<PathBuf>,
    pub on_screen: bool,
    pub dirty_paths: HashSet<PathBuf>,
}

impl Default for FileTreeContext {
    fn default() -> Self {
        Self {
            tab_id: None,
            host_id: HostId::LOCAL,
            host: None,
            pane_cwds: Vec::new(),
            active_is_ssh: false,
            display_home: None,
            on_screen: false,
            dirty_paths: HashSet::new(),
        }
    }
}

/// Events emitted by [`FileTreeView`] to its parent workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTreeEvent {
    OpenFile(PathBuf),
    CdToDir(PathBuf),
    InsertPath(PathBuf),
    AttachToAgent(PathBuf),
    EditorPathMoved {
        host: HostId,
        from: PathBuf,
        to: PathBuf,
    },
    EditorPathRemoved {
        host: HostId,
        path: PathBuf,
    },
    ShowPanel,
}

#[derive(Default)]
pub(super) struct SearchState {
    pub(super) generation: u64,
    pub(super) pending: String,
    pub(super) hidden: bool,
    pub(super) hits: Vec<TreeEntry>,
    /// Whether the last search came back as a failure rather than as no hits.
    /// The two used to print the same "Nothing matches …", which is the same
    /// lie `unreadable` was added to stop a directory listing from telling.
    pub(super) failed: bool,
}

impl SearchState {
    pub(super) fn retarget(&mut self, query: &str, show_hidden: bool) -> Option<u64> {
        if self.pending == query && self.hidden == show_hidden {
            return None;
        }
        self.generation += 1;
        self.pending = query.to_string();
        self.hidden = show_hidden;
        if query.is_empty() {
            self.hits.clear();
            self.failed = false;
            return None;
        }
        Some(self.generation)
    }

    pub(super) fn accept(&mut self, generation: u64, ok: bool, hits: Vec<TreeEntry>) -> bool {
        if self.generation != generation {
            return false;
        }
        self.hits = hits;
        self.failed = !ok;
        true
    }

    pub(super) fn restart(&mut self) {
        self.generation += 1;
        self.pending.clear();
        self.failed = false;
    }
}

/// The rows a search puts in the column, and the note that stands for whatever
/// they do not say by themselves.
pub(super) fn search_rows(search: &SearchState) -> Vec<TreeRow> {
    let mut rows: Vec<TreeRow> = search
        .hits
        .iter()
        .map(|e| TreeRow {
            entry: e.clone(),
            depth: 0,
            is_root: false,
            expanded: false,
            note: None,
        })
        .collect();
    let note = if search.failed {
        // Ahead of the cap: a failed search has no hits to have capped, and
        // this is the one thing worth saying about it.
        Some(TreeNote::SearchFailed)
    } else if rows.len() >= SEARCH_LIMIT {
        Some(TreeNote::SearchCapped)
    } else {
        None
    };
    if let Some(note) = note {
        rows.push(TreeRow {
            entry: TreeEntry {
                name: String::new(),
                path: PathBuf::new(),
                is_dir: false,
                ignored: false,
            },
            depth: 0,
            is_root: false,
            expanded: false,
            note: Some(note),
        });
    }
    rows
}

impl FileTreeView {
    pub(super) fn active_tab_state(&self) -> Option<&TabTreeState> {
        self.tabs.get(&self.ctx.tab_id)
    }

    pub(super) fn active_tab_state_mut(&mut self) -> &mut TabTreeState {
        self.tabs.entry(self.ctx.tab_id).or_default()
    }

    pub(super) fn all_watched_dirs(tabs: &HashMap<Option<TabId>, TabTreeState>) -> HashSet<PathBuf> {
        tabs.values()
            .flat_map(|c| c.roots.iter().chain(c.expanded.iter()).cloned())
            .collect()
    }

    pub(super) fn sync_watch(
        &mut self,
        host: SharedHost,
        dirs: HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.watched = dirs;
        let want: Vec<PathBuf> = self.watched.iter().cloned().collect();
        if !self
            .watch_host
            .as_ref()
            .is_some_and(|opened_with| Arc::ptr_eq(opened_with, &host))
        {
            self.watch = None;
            self.watch_host = None;
            self.watch_busy = false;
            self.watch_dirty = false;
        }
        if let Some(sub) = self.watch.clone() {
            if self.watch_busy {
                self.watch_dirty = true;
                return;
            }
            self.watch_busy = true;
            HostOps::run(
                host,
                cx,
                move |_| sub.set_dirs(&want),
                |this: &mut FileTreeView, result: std::io::Result<()>, cx| {
                    this.watch_busy = false;
                    if let Err(e) = result {
                        log::warn!("file tree: could not update the watched set: {e}");
                    }
                    if std::mem::take(&mut this.watch_dirty) {
                        let want = this.watched.clone();
                        let Some(host) = this.ctx.host.clone() else {
                            return;
                        };
                        this.sync_watch(host, want, cx);
                    }
                },
            );
            return;
        }
        if self.watch_opening {
            return;
        }
        self.watch_opening = true;
        let host_id = host.id();
        let opened_host = Arc::clone(&host);
        let opened_with = self.watched.clone();
        HostOps::run(
            host,
            cx,
            {
                let want = want.clone();
                move |h| h.watch(&want).map(Arc::new)
            },
            move |this, result: std::io::Result<Arc<WatchSub>>, cx| {
                this.watch_opening = false;
                let sub = match result {
                    Ok(sub) => sub,
                    Err(e) => {
                        log::warn!("file tree: watcher unavailable: {e}");
                        return;
                    }
                };
                let events = sub.events().clone();
                this.watch = Some(sub);
                this.watch_host = Some(opened_host);
                cx.spawn(async move |this, cx| {
                    while let Ok(batch) = events.recv().await {
                        let ok = this.update(cx, |this, _cx| {
                            let _ = this.events_tx.try_send((host_id, batch));
                        });
                        if ok.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                if this.watched != opened_with {
                    let want = this.watched.clone();
                    let Some(host) = this.ctx.host.clone() else {
                        return;
                    };
                    this.sync_watch(host, want, cx);
                }
            },
        );
    }

    pub(super) fn request_loads(
        &mut self,
        host: &SharedHost,
        roots: &[PathBuf],
        expanded: &HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        for root in roots {
            self.request_load(host, root.clone(), root.clone(), cx);
            for dir in expanded {
                if dir.starts_with(root) {
                    self.request_load(host, dir.clone(), root.clone(), cx);
                }
            }
        }
    }

    fn request_load(
        &mut self,
        host: &SharedHost,
        dir: PathBuf,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let id = host.id();
        let key: DirKey = (id, dir.clone());
        let current = self.children.get(id, &dir).is_some() && !self.stale.contains(&key);
        if current || !self.loads.begin(key.clone()) {
            return;
        }
        self.spawn_load(host, dir, root, cx);
    }

    fn spawn_load(
        &mut self,
        host: &SharedHost,
        dir: PathBuf,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let id = host.id();
        let key: DirKey = (id, dir.clone());
        HostOps::run(
            host.clone(),
            cx,
            {
                let dir = dir.clone();
                let root = root.clone();
                move |h| {
                    let listing = h.read_dir(&dir, Some(&root));
                    let readable = listing.is_ok();
                    let entries = listing
                        .unwrap_or_default()
                        .into_iter()
                        .map(|e| TreeEntry {
                            path: h.join(&dir, &e.name),
                            name: e.name,
                            is_dir: e.is_dir,
                            ignored: e.ignored,
                        })
                        .collect::<Vec<_>>();
                    (readable, entries)
                }
            },
            move |this, (readable, entries), cx| {
                if readable {
                    this.unreadable.remove(&key);
                } else {
                    this.unreadable.insert(key.clone());
                }
                let landed = this.land_load(&key, id, dir.clone(), entries);
                if landed.changed {
                    cx.notify();
                }
                if !landed.superseded {
                    return;
                }
                let Some(host) = this.ctx.host.clone() else {
                    return;
                };
                if host.id() != id {
                    return;
                }
                this.loads.begin(key);
                this.spawn_load(&host, dir, root, cx);
            },
        );
    }

    fn land_load(
        &mut self,
        key: &DirKey,
        id: HostId,
        dir: PathBuf,
        entries: Vec<TreeEntry>,
    ) -> Landed {
        let changed = self.children.get(id, &dir) != Some(&entries);
        let superseded = land_listing(
            &mut self.loads,
            &mut self.children,
            &mut self.stale,
            key,
            id,
            dir,
            entries,
        );
        Landed {
            superseded,
            changed,
        }
    }

    pub(super) fn sync_search(
        &mut self,
        query: &str,
        roots: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let Some(generation) = self.search.retarget(query, self.show_hidden) else {
            return;
        };
        let show_hidden = self.show_hidden;
        let (query, roots) = (query.to_string(), roots.to_vec());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                if this.search.generation != generation {
                    return;
                }
                let Some(host) = this.ctx.host.clone() else {
                    return;
                };
                HostOps::run(
                    host,
                    cx,
                    move |h| {
                        // `(ok, hits)` the way `spawn_load` reports a listing:
                        // a search the host refused is not a search with no
                        // hits, and the column has to be able to tell them
                        // apart before it says "Nothing matches".
                        let found =
                            h.search(&roots, &query, SEARCH_LIMIT, SEARCH_MAX_DIRS, show_hidden);
                        let ok = found.is_ok();
                        let hits = found
                            .unwrap_or_default()
                            .into_iter()
                            .map(|hit| TreeEntry {
                                name: hit.name,
                                path: hit.path,
                                is_dir: hit.is_dir,
                                ignored: hit.ignored,
                            })
                            .collect::<Vec<_>>();
                        (ok, hits)
                    },
                    move |this, (ok, hits), cx| {
                        if this.search.accept(generation, ok, hits) {
                            cx.notify();
                        }
                    },
                );
            });
        })
        .detach();
    }

    pub(super) fn search_rows(&self) -> Vec<TreeRow> {
        search_rows(&self.search)
    }

    pub fn visible_rows(
        &self,
        host: HostId,
        roots: &[PathBuf],
        expanded: &HashSet<PathBuf>,
    ) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        for root in roots {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string());
            rows.push(TreeRow {
                entry: TreeEntry {
                    name,
                    path: root.clone(),
                    is_dir: true,
                    ignored: false,
                },
                depth: 0,
                is_root: true,
                expanded: true,
                note: None,
            });
            self.flatten_dir(host, root, 1, expanded, &mut rows);
        }
        rows
    }

    fn flatten_dir(
        &self,
        host: HostId,
        dir: &Path,
        depth: usize,
        expanded: &HashSet<PathBuf>,
        out: &mut Vec<TreeRow>,
    ) {
        let Some(entries) = self.children.get(host, &dir.to_path_buf()) else {
            out.push(self.note_row(host, dir, depth, None));
            return;
        };
        let mut shown = 0usize;
        for e in entries {
            if !self.show_hidden && e.name.starts_with('.') {
                continue;
            }
            shown += 1;
            let is_expanded = e.is_dir && expanded.contains(&e.path);
            out.push(TreeRow {
                entry: e.clone(),
                depth,
                is_root: false,
                expanded: is_expanded,
                note: None,
            });
            if is_expanded {
                self.flatten_dir(host, &e.path, depth + 1, expanded, out);
            }
        }
        if shown == 0 {
            out.push(self.note_row(host, dir, depth, Some(entries.len())));
        }
    }

    /// Why an expanded directory drew no children. `landed` is the number of
    /// entries the listing actually returned, or `None` when nothing has landed
    /// yet — that is the difference between "empty" and "still working".
    fn note_row(&self, host: HostId, dir: &Path, depth: usize, landed: Option<usize>) -> TreeRow {
        let key: DirKey = (host, dir.to_path_buf());
        let note = dir_note(self.unreadable.contains(&key), landed);
        TreeRow {
            entry: TreeEntry {
                name: String::new(),
                path: dir.to_path_buf(),
                is_dir: true,
                ignored: false,
            },
            depth,
            is_root: false,
            expanded: false,
            note: Some(note),
        }
    }

    pub(super) fn invalidate_dir(&mut self, host: HostId, dir: &Path) -> bool {
        let key: DirKey = (host, dir.to_path_buf());
        let cached = self.children.get(host, dir).is_some();
        if cached {
            self.stale.insert(key.clone());
        }
        let pending = self.loads.is_pending(&key);
        self.loads.invalidate(&key);
        cached || pending
    }

    pub(super) fn gitignore_reaches_tree(&self, host: HostId, paths: &HashSet<PathBuf>) -> bool {
        paths
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == ".gitignore"))
            .filter_map(|p| p.parent())
            .any(|dir| {
                self.children
                    .keys()
                    .any(|(id, cached)| id == host && cached.starts_with(dir))
                    || self
                        .loads
                        .pending_keys()
                        .any(|(id, pending)| *id == host && pending.starts_with(dir))
            })
    }

    pub(super) fn invalidate_all(&mut self) {
        self.stale
            .extend(self.children.keys().map(|(host, dir)| (host, dir.clone())));
        self.loads.invalidate_all();
        self.search.restart();
    }

    pub(super) fn invalidate_repo_roots(&mut self) -> bool {
        let had = !self.repo_roots.is_empty() || !self.repo_root_loads.is_empty();
        self.repo_roots.clear();
        self.root_aliases.clear();
        self.repo_root_loads.invalidate_all();
        had
    }

    pub(super) fn optimistic(
        &mut self,
        host: HostId,
        dir: &Path,
        op: &TreeWrite,
        target: &TreeEntry,
    ) -> Option<Vec<TreeEntry>> {
        self.loads.invalidate(&(host, dir.to_path_buf()));
        optimistic_write(&mut self.children, host, dir, op, target)
    }

    pub(super) fn rollback(&mut self, host: HostId, dir: &Path, before: Option<Vec<TreeEntry>>) {
        rollback_write(&mut self.children, host, dir, before)
    }
}

pub fn sort_entries(entries: &mut [TreeEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

pub(super) fn optimistic_write(
    children: &mut ByHost<PathBuf, Vec<TreeEntry>>,
    host: HostId,
    dir: &Path,
    op: &TreeWrite,
    target: &TreeEntry,
) -> Option<Vec<TreeEntry>> {
    let key = dir.to_path_buf();
    let before = children.get(host, &key).cloned();
    if let Some(mut entries) = children.remove(host, &key) {
        match op {
            TreeWrite::Rename { from } => entries.retain(|e| e.path != *from),
            TreeWrite::Delete => entries.retain(|e| e.path != target.path),
            TreeWrite::NewFile | TreeWrite::NewFolder => {}
        }
        if !matches!(op, TreeWrite::Delete) {
            entries.push(target.clone());
            sort_entries(&mut entries);
        }
        children.insert(host, key, entries);
    }
    before
}

pub(super) fn rollback_write(
    children: &mut ByHost<PathBuf, Vec<TreeEntry>>,
    host: HostId,
    dir: &Path,
    before: Option<Vec<TreeEntry>>,
) {
    drop(before);
    children.remove(host, &dir.to_path_buf());
}

/// Quote a path for the shell the pane is actually running.
///
/// The rules live in [`anterm_core::shell_quote`], shared with the terminal's
/// own path insertion so the two cannot drift apart again (#593).
pub fn shell_quote_for(path: &Path, shell_program: Option<&str>) -> String {
    anterm_core::shell_quote::quote_for_shell(&path.to_string_lossy(), shell_program)
}
