use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anterm_core::machine::TabId;
use anterm_i18n::{L10nKey, t, t_fmt};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, Pixels, PromptLevel, ScrollStrategy, SharedString, Size,
    Subscription, Window, div, px, size,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, VirtualListScrollHandle, WindowExt as _,
    h_flex, v_flex, v_virtual_list,
};

use anterm_core::git::status::{DecoStatus, DirRollup, StatusIndex};
use anterm_ui_kit::git::data::{index_of, scm_invalidate_cwd, scm_refresh};
use crate::file_copy;
use crate::panel_search::{PanelSearchTarget, PanelSearchView};
use crate::right_panel::{ROW_FILL_RADIUS, ROW_GLYPH, ROW_INSET, git_badge};
use crate::scm::status::{status_color, status_glyph};
use anterm_ui_kit::host_ops::{ByHost, HostId, HostOps, InFlight, SharedHost, WatchSub};
use anterm_ui_kit::window_chrome::CONTENT_INSET;

mod model;
#[cfg(test)]
mod tests;

pub use model::{
    FileTreeContext, FileTreeEvent, TabTreeState, TreeEdit, TreeEntry, TreeNote, TreeRow,
    shell_quote_for,
};
use model::{DirKey, SearchState};
#[cfg(test)]
use model::sort_entries;

// The tree is laid out the way every other list in this panel is: the column
// sits `CONTENT_INSET` in and each row pads itself a further `ROW_INSET`, so a
// depth-0 chevron lands on the panel's 20px text column — the search glyph's —
// while the row's hover and selection fill bleeds past it to 12. Depth is
// added on top of that inset, so `INDENT` is the step between levels and
// nothing else: 16, which puts a child's chevron under its parent's folder
// glyph.
const INDENT: f32 = 16.0;

/// Room under the last row, so the tree does not end flush on the window's
/// bottom edge.
const TREE_PAD_BOTTOM: f32 = 16.;

/// How tall a tree row is.
const TREE_ROW_H: f32 = 26.;

/// The disclosure chevron's column, drawn on folders and left empty on files
/// so every name at one depth starts at the same x.
const CHEVRON_W: f32 = 10.;

/// The gap between a row's chevron, icon and name.
const TREE_GAP: f32 = 6.;

/// Where a row's label starts, measured from the row's own inset.
const LABEL_LEAD: f32 = CHEVRON_W + TREE_GAP + ROW_GLYPH + TREE_GAP;

const REFRESH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

const SEARCH_LIMIT: usize = 200;

const SEARCH_MAX_DIRS: usize = 2000;

/// How many renders a reveal waits for its row to show up.
const TREE_REVEAL_RENDERS: u8 = 60;

#[cfg(test)]
pub mod row_probe {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static LAST_RENDERED: AtomicUsize = AtomicUsize::new(0);

    pub fn reset() {
        LAST_RENDERED.store(0, Ordering::SeqCst);
    }

    pub(super) fn record(count: usize) {
        LAST_RENDERED.store(count, Ordering::SeqCst);
    }

    pub fn last() -> usize {
        LAST_RENDERED.load(Ordering::SeqCst)
    }
}

enum FlatTreeItem {
    Row(TreeRow),
    CreateInput {
        depth: usize,
        input: Entity<InputState>,
    },
}

#[derive(Clone, Copy)]
enum TreeEditKind {
    NewFile,
    NewFolder,
    Rename,
}

enum TreeWrite {
    NewFile,
    NewFolder,
    Rename { from: PathBuf },
    Delete,
}

/// Standalone virtualized file tree entity.
pub struct FileTreeView {
    pub(super) ctx: FileTreeContext,
    pub(super) tabs: HashMap<Option<TabId>, TabTreeState>,
    search_input: Entity<InputState>,
    _search_sub: Subscription,
    panel_search: Entity<PanelSearchView>,
    scroll: VirtualListScrollHandle,
    search_scroll: gpui::ScrollHandle,
    tree_reveal: Option<(PathBuf, u8)>,
    rendered_items: Vec<FlatTreeItem>,
    rendered_decor: Decorations,
    pub(super) children: ByHost<PathBuf, Vec<TreeEntry>>,
    pub(super) loads: InFlight<DirKey>,
    pub(super) unreadable: HashSet<DirKey>,
    pub(super) stale: HashSet<DirKey>,
    pub(super) repo_roots: ByHost<PathBuf, PathBuf>,
    pub(super) root_aliases: ByHost<PathBuf, PathBuf>,
    repo_root_loads: InFlight<DirKey>,
    search: SearchState,
    /// Every file under the project, for Quick Open (`ui::chrome::search::files`).
    pub quick_open: anterm_ui_kit::search::FileIndexStore,
    pub show_hidden: bool,
    pub editing: Option<TreeEdit>,
    pub(super) editing_subs: Vec<Subscription>,
    pub(super) watch: Option<Arc<WatchSub>>,
    pub(super) watch_host: Option<SharedHost>,
    pub(super) watch_opening: bool,
    pub(super) watch_busy: bool,
    pub(super) watch_dirty: bool,
    pub(super) watched: HashSet<PathBuf>,
    pub(super) events_tx: smol::channel::Sender<(HostId, Vec<PathBuf>)>,
    pub focus_handle: FocusHandle,
}

impl Focusable for FileTreeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<FileTreeEvent> for FileTreeView {}

impl FileTreeView {
    pub fn new(
        search_input: Entity<InputState>,
        panel_search: Entity<PanelSearchView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = smol::channel::unbounded::<(HostId, Vec<PathBuf>)>();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok((host, first)) = rx.recv().await {
                cx.background_executor().timer(REFRESH_DEBOUNCE).await;
                let mut changed: HashSet<PathBuf> = first.into_iter().collect();
                while let Ok((h, more)) = rx.try_recv() {
                    if h == host {
                        changed.extend(more);
                    }
                }
                let ok = this.update(cx, |this, cx| {
                    this.apply_fs_events(host, &changed, cx);
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        let search_sub = cx.subscribe_in(&search_input, window, |_this, _, ev, _, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            ctx: FileTreeContext::default(),
            tabs: HashMap::new(),
            search_input,
            _search_sub: search_sub,
            panel_search,
            scroll: VirtualListScrollHandle::new(),
            search_scroll: gpui::ScrollHandle::new(),
            tree_reveal: None,
            rendered_items: Vec::new(),
            rendered_decor: Vec::new(),
            watch_host: None,
            children: ByHost::default(),
            loads: InFlight::default(),
            unreadable: HashSet::new(),
            stale: HashSet::new(),
            repo_roots: ByHost::default(),
            root_aliases: ByHost::default(),
            repo_root_loads: InFlight::default(),
            search: SearchState::default(),
            quick_open: Default::default(),
            show_hidden: false,
            editing: None,
            editing_subs: Vec::new(),
            watch: None,
            watch_opening: false,
            watch_busy: false,
            watch_dirty: false,
            watched: HashSet::new(),
            events_tx: tx,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Push the active workspace context into the tree before painting.
    pub fn sync_context(&mut self, ctx: FileTreeContext, _cx: &mut Context<Self>) {
        self.ctx = ctx;
    }

    pub fn active_roots(&self) -> &[PathBuf] {
        self.active_tab_state()
            .map(|c| c.roots.as_slice())
            .unwrap_or(&[])
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.editing = None;
            self.editing_subs.clear();
            cx.notify();
        }
    }

    pub fn query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().trim().to_lowercase()
    }

    pub fn searching(&self, cx: &App) -> bool {
        !self.query(cx).is_empty()
    }

    pub fn listings_on_screen(&self, cx: &App) -> bool {
        self.ctx.on_screen && !self.searching(cx)
    }

    pub fn refresh_roots(&mut self, cx: &mut Context<Self>) {
        let id = self.ctx.host_id;
        let Some((host, mut roots)) = self.resolve_project_roots(cx) else {
            return;
        };
        if roots.is_empty()
            && id.is_local()
            && let Some(home) = std::env::var_os("HOME")
        {
            roots.push(PathBuf::from(home));
        }
        let changed = {
            let state = self.active_tab_state_mut();
            if roots != state.roots {
                state.roots = roots;
                true
            } else {
                false
            }
        };
        if changed {
            self.invalidate_all();
            cx.notify();
        }
        self.sync_watch_all(host, cx);
    }

    /// The project directories behind the active tab, on the host they live
    /// on: each pane's repository root, or its directory where it is not in
    /// one. `None` while a root is still being asked for.
    pub fn resolve_project_roots(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(SharedHost, Vec<PathBuf>)> {
        let id = self.ctx.host_id;
        let host = self.ctx.host.clone()?;
        let cwds = self.ctx.pane_cwds.clone();
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut resolved = true;
        for cwd in &cwds {
            match self.repo_roots.get(id, cwd) {
                Some(root) => {
                    if !roots.contains(root) {
                        roots.push(root.clone());
                    }
                }
                None => {
                    resolved = false;
                    self.request_repo_root(&host, cwd.clone(), cx);
                }
            }
        }
        resolved.then_some((host, roots))
    }

    fn sync_watch_all(&mut self, host: SharedHost, cx: &mut Context<Self>) {
        let union = Self::all_watched_dirs(&self.tabs);
        if union != self.watched {
            self.sync_watch(host, union, cx);
        }
    }

    fn request_repo_root(&mut self, host: &SharedHost, cwd: PathBuf, cx: &mut Context<Self>) {
        let id = host.id();
        let key: DirKey = (id, cwd.clone());
        if !self.repo_root_loads.begin(key.clone()) {
            return;
        }
        HostOps::run(
            host.clone(),
            cx,
            {
                let cwd = cwd.clone();
                move |h| {
                    let root = h.repo_root(&cwd).ok().flatten().unwrap_or(cwd);
                    let real = h.canonicalize(&root).ok().filter(|real| *real != root);
                    (root, real)
                }
            },
            move |this, (root, real), cx| {
                if this.repo_root_loads.finish(&key) {
                    if let Some(real) = real {
                        this.root_aliases.insert(id, root.clone(), real);
                    }
                    this.repo_roots.insert(id, cwd, root);
                }
                cx.notify();
            },
        );
    }

    pub fn apply_fs_events(
        &mut self,
        host: HostId,
        paths: &HashSet<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        log::debug!(
            target: "anterm::file_tree",
            "fs events on host {host:?}: {:?}",
            paths.iter().take(8).collect::<Vec<_>>()
        );
        let on_screen = self.ctx.on_screen;
        let listings_on_screen = self.listings_on_screen(cx);
        let mut roots_moved = false;
        if paths.iter().any(|p| {
            p.file_name().is_some_and(|n| n == ".git")
                || p.parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == ".git")
        }) {
            roots_moved = self.invalidate_repo_roots();
        }
        // Working-tree edits the source control cache has no other way to hear
        // about. Anything under `.git` is skipped: the repository has its own
        // watch.
        let mut announced: HashSet<&Path> = HashSet::new();
        for path in paths {
            if path.components().any(|c| c.as_os_str() == ".git") {
                continue;
            }
            let Some(dir) = path.parent() else { continue };
            if announced.insert(dir) {
                scm_invalidate_cwd(host, dir, cx);
            }
        }

        let gitignore_touched = paths
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == ".gitignore"));
        if gitignore_touched && self.gitignore_reaches_tree(host, paths) {
            self.invalidate_all();
            if on_screen {
                cx.notify();
            }
        } else {
            let mut touched = false;
            for dir in dirs_to_relist(paths, self.show_hidden) {
                touched |= self.invalidate_dir(host, dir);
            }
            if roots_moved && on_screen {
                cx.notify();
            }
            if !touched || !listings_on_screen {
                return;
            }
            let Some(shared) = self.ctx.host.clone() else {
                return;
            };
            if shared.id() != host {
                return;
            }
            let (roots, expanded) = match self.active_tab_state() {
                Some(code) => (code.roots.clone(), code.expanded.clone()),
                None => return,
            };
            self.request_loads(&shared, &roots, &expanded, cx);
        }
    }

    fn toggle_expand(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let state = self.active_tab_state_mut();
        if !state.expanded.remove(dir) {
            state.expanded.insert(dir.to_path_buf());
        }
        cx.notify();
    }

    fn activate(
        &mut self,
        row_path: &Path,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_tab_state_mut().selected = Some(row_path.to_path_buf());
        let searching = self.searching(cx);
        if is_dir && searching {
            self.reveal_dir(row_path, cx);
            self.search_input
                .update(cx, |st, cx| st.set_value("", window, cx));
            cx.notify();
            return;
        }
        if is_dir {
            self.toggle_expand(row_path, cx);
        } else {
            cx.emit(FileTreeEvent::OpenFile(row_path.to_path_buf()));
        }
        cx.notify();
    }

    fn reveal_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.expand_ancestors(dir) {
            cx.notify();
        }
    }

    /// Opens `dir` and every directory between it and its root.
    fn expand_ancestors(&mut self, dir: &Path) -> bool {
        let roots = self
            .active_tab_state()
            .map(|c| c.roots.clone())
            .unwrap_or_default();
        let Some(root) = roots.iter().find(|r| dir.starts_with(r)).cloned() else {
            return false;
        };
        let state = self.active_tab_state_mut();
        let mut opened = false;
        for a in dir.ancestors().take_while(|a| a.starts_with(&root)) {
            opened |= state.expanded.insert(a.to_path_buf());
        }
        opened
    }

    /// `path` as the tree spells it: under the project root as the tab names
    /// it, when `path` is that root's real path spelled out.
    pub fn project_spelling(&self, path: &Path) -> PathBuf {
        let host = self.ctx.host_id;
        let roots = self.active_roots();
        if roots.iter().any(|root| path.starts_with(root)) {
            return path.to_path_buf();
        }
        roots
            .iter()
            .find_map(|root| {
                let real = self.root_aliases.get(host, root)?;
                Some(root.join(path.strip_prefix(real).ok()?))
            })
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// `path` relative to the first of this tab's tree roots it sits under.
    pub fn path_under_tree_root(&self, path: &Path) -> Option<PathBuf> {
        self.active_roots()
            .iter()
            .find_map(|r| path.strip_prefix(r).ok())
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
    }

    /// Points the tree at `path`: opens every directory above it, selects it,
    /// and asks the column to scroll it into view.
    pub fn reveal_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let path = self.project_spelling(path);
        let roots = self.active_roots().to_vec();
        if !roots.is_empty() && !roots.iter().any(|root| path.starts_with(root)) {
            return false;
        }
        self.reveal_dir(&path, cx);
        self.active_tab_state_mut().selected = Some(path.clone());
        self.tree_reveal = Some((path, TREE_REVEAL_RENDERS));
        cx.notify();
        true
    }

    /// [`Self::reveal_path`], with the panel brought up to show it.
    pub fn show_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.reveal_path(path, cx) {
            if let Err(e) = anterm_ui_kit::platform::open::open_file_path(path) {
                log::warn!("failed to open {}: {e}", path.display());
                window.push_notification(
                    anterm_ui_kit::host_ops::failure(
                        t_fmt(
                            L10nKey::LinkFileOpenFailed,
                            &[
                                ("path", &path.display().to_string()),
                                ("error", &e.to_string()),
                            ],
                        ),
                        &e,
                    ),
                    cx,
                );
            } else {
                window.push_notification(
                    t_fmt(
                        L10nKey::LinkDirOutsideTree,
                        &[("path", &path.display().to_string())],
                    ),
                    cx,
                );
            }
            return;
        }
        cx.emit(FileTreeEvent::ShowPanel);
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() && ev.keystroke.key == "escape" {
            self.cancel_edit(cx);
            self.focus_handle.focus(window, cx);
            cx.stop_propagation();
            return;
        }
        let host = self.ctx.host_id;
        let Some(code) = self.active_tab_state() else {
            return;
        };
        let rows: Vec<TreeRow> = self
            .visible_rows(host, &code.roots, &code.expanded)
            .into_iter()
            .filter(|r| r.note.is_none())
            .collect();
        if rows.is_empty() {
            return;
        }
        let sel_ix = code
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| r.entry.path == *s));
        let key = ev.keystroke.key.as_str();
        match key {
            "up" | "down" => {
                let next = match (sel_ix, key) {
                    (None, _) => 0,
                    (Some(i), "up") => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(rows.len() - 1),
                };
                let path = rows[next].entry.path.clone();
                self.active_tab_state_mut().selected = Some(path);
                cx.notify();
            }
            "left" => {
                let Some(i) = sel_ix else { return };
                let row = &rows[i];
                let (path, is_dir, expanded, is_root) = (
                    row.entry.path.clone(),
                    row.entry.is_dir,
                    row.expanded,
                    row.is_root,
                );
                let parent_in_rows = path
                    .parent()
                    .is_some_and(|p| rows.iter().any(|r| r.entry.path == p));
                let state = self.active_tab_state_mut();
                if is_dir && expanded && !is_root {
                    state.expanded.remove(&path);
                } else if parent_in_rows && let Some(parent) = path.parent() {
                    state.selected = Some(parent.to_path_buf());
                }
                cx.notify();
            }
            "right" => {
                let Some(i) = sel_ix else { return };
                let row = &rows[i];
                if row.entry.is_dir && !row.expanded && !row.is_root {
                    let path = row.entry.path.clone();
                    self.active_tab_state_mut().expanded.insert(path);
                    cx.notify();
                }
            }
            "enter" => {
                let Some(i) = sel_ix else { return };
                let (path, is_dir) = (rows[i].entry.path.clone(), rows[i].entry.is_dir);
                self.activate(&path, is_dir, window, cx);
            }
            _ => {}
        }
    }

    fn begin_edit(
        &mut self,
        edit_for: TreeEditKind,
        target: &Path,
        target_is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = match edit_for {
            TreeEditKind::Rename => target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let input = cx.new(|cx| {
            let mut st = InputState::new(window, cx).placeholder(match edit_for {
                TreeEditKind::NewFile => t(L10nKey::FileTreePlaceholderFileName),
                TreeEditKind::NewFolder => t(L10nKey::FileTreePlaceholderFolderName),
                TreeEditKind::Rename => t(L10nKey::FileTreePlaceholderNewName),
            });
            st.set_value(initial, window, cx);
            st
        });
        input.update(cx, |st, cx| st.focus(window, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut FileTreeView, _input, ev, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.commit_edit(window, cx),
                InputEvent::Blur => this.cancel_edit(cx),
                _ => {}
            },
        );
        self.editing_subs = vec![sub];
        let host_dir = if target_is_dir {
            target.to_path_buf()
        } else {
            target.parent().unwrap_or(target).to_path_buf()
        };
        if !matches!(edit_for, TreeEditKind::Rename) {
            self.active_tab_state_mut().expanded.insert(host_dir.clone());
        }
        self.editing = Some(match edit_for {
            TreeEditKind::NewFile => TreeEdit::NewFile {
                dir: host_dir,
                input,
            },
            TreeEditKind::NewFolder => TreeEdit::NewFolder {
                dir: host_dir,
                input,
            },
            TreeEditKind::Rename => TreeEdit::Rename {
                path: target.to_path_buf(),
                input,
            },
        });
        cx.notify();
    }

    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        self.editing_subs.clear();
        let name = edit.input().read(cx).value().trim().to_string();
        if name.is_empty() || name.contains('/') {
            cx.notify();
            return;
        }
        let Some(host) = self.ctx.host.clone() else {
            return;
        };
        let id = host.id();
        let dir = edit.host_dir().to_path_buf();
        let (new_path, is_dir, op): (PathBuf, bool, TreeWrite) = match &edit {
            TreeEdit::NewFile { dir, .. } => (host.join(dir, &name), false, TreeWrite::NewFile),
            TreeEdit::NewFolder { dir, .. } => (host.join(dir, &name), true, TreeWrite::NewFolder),
            TreeEdit::Rename { path, .. } => {
                let was_dir = self
                    .children
                    .get(id, &dir)
                    .and_then(|entries| entries.iter().find(|e| e.path == *path))
                    .is_some_and(|e| e.is_dir);
                let parent = path.parent().unwrap_or(path);
                (
                    host.join(parent, &name),
                    was_dir,
                    TreeWrite::Rename { from: path.clone() },
                )
            }
        };

        let row = TreeEntry {
            name: name.clone(),
            path: new_path.clone(),
            is_dir,
            ignored: false,
        };
        let rollback = self.optimistic(id, &dir, &op, &row);
        self.active_tab_state_mut().selected = Some(new_path.clone());

        let target = new_path.clone();
        let (context, failed_name) = match &edit {
            TreeEdit::Rename { path, .. } => (
                L10nKey::FileTreeRenameFailed,
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| name.clone()),
            ),
            _ => (L10nKey::FileTreeCreateFailed, name.clone()),
        };
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| match &op {
                TreeWrite::NewFile => h.create_file_new(&target).map(|()| None),
                TreeWrite::NewFolder => h.create_dir(&target, false).map(|()| None),
                TreeWrite::Rename { from } => {
                    let canon_from = h.canonicalize(from).unwrap_or_else(|_| from.clone());
                    h.rename(from, &target)?;
                    let canon_to = h.canonicalize(&target).unwrap_or_else(|_| target.clone());
                    Ok(Some((canon_from, canon_to)))
                }
                TreeWrite::Delete => h.remove(&target, is_dir).map(|()| None),
            },
            move |this, result: std::io::Result<Option<(PathBuf, PathBuf)>>, window, cx| {
                match result {
                    Ok(moved) => {
                        if let Some((from, to)) = moved {
                            cx.emit(FileTreeEvent::EditorPathMoved {
                                host: id,
                                from,
                                to,
                            });
                        }
                        this.invalidate_dir(id, &dir);
                        if matches!(edit, TreeEdit::NewFile { .. }) {
                            cx.emit(FileTreeEvent::OpenFile(new_path));
                        }
                    }
                    Err(e) => {
                        this.rollback(id, &dir, rollback);
                        if this
                            .active_tab_state()
                            .and_then(|c| c.selected.as_deref())
                            == Some(&*new_path)
                        {
                            this.active_tab_state_mut().selected = None;
                        }
                        HostOps::notify_err(
                            window,
                            cx,
                            &t_fmt(context, &[("name", &failed_name)]),
                            &e,
                        );
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn download(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.ctx.host.clone() else {
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let src = path.to_path_buf();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                file_copy::download_file(
                    h,
                    &src,
                    &anterm_ui_kit::daemon_rpc::local_download_dir(),
                    file_copy::REMOTE_FILE_MAX,
                )
            },
            move |_this, result: std::io::Result<PathBuf>, window, cx| match result {
                Ok(local) => window.push_notification(
                    t_fmt(
                        L10nKey::FileTreeDownloaded,
                        &[(
                            "path",
                            &anterm_ui_kit::path_display::native_separators(&local)
                                .display()
                                .to_string(),
                        )],
                    ),
                    cx,
                ),
                Err(e) => HostOps::notify_err(
                    window,
                    cx,
                    &t_fmt(L10nKey::FileTreeDownloadFailed, &[("name", &name)]),
                    &e,
                ),
            },
        );
    }

    fn delete(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let detail = if is_dir {
            t(L10nKey::FileTreeDeleteFolderBody)
        } else {
            t(L10nKey::FileTreeDeleteFileBody)
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &t_fmt(L10nKey::FileTreeDeleteTitle, &[("name", &name)]),
            Some(detail),
            &anterm_ui_kit::confirm_answers(t(L10nKey::Delete), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(0) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(host) = this.ctx.host.clone() else {
                    return;
                };
                let id = host.id();
                let Some(parent) = path.parent().map(Path::to_path_buf) else {
                    return;
                };
                let row = TreeEntry {
                    name: name.clone(),
                    path: path.clone(),
                    is_dir,
                    ignored: false,
                };
                let rollback = this.optimistic(id, &parent, &TreeWrite::Delete, &row);
                if this
                    .active_tab_state()
                    .and_then(|c| c.selected.as_deref())
                    == Some(&*path)
                {
                    this.active_tab_state_mut().selected = None;
                }
                let target = path.clone();
                let failed_name = name.clone();
                HostOps::run_in(
                    host,
                    window,
                    cx,
                    move |h| {
                        let canon = h.canonicalize(&target).unwrap_or_else(|_| target.clone());
                        h.remove(&target, is_dir).map(|()| canon)
                    },
                    move |this, result: std::io::Result<PathBuf>, window, cx| {
                        match result {
                            Ok(removed) => {
                                cx.emit(FileTreeEvent::EditorPathRemoved {
                                    host: id,
                                    path: removed,
                                });
                                this.invalidate_dir(id, &parent);
                            }
                            Err(e) => {
                                this.rollback(id, &parent, rollback);
                                HostOps::notify_err(
                                    window,
                                    cx,
                                    &t_fmt(
                                        L10nKey::FileTreeDeleteFailed,
                                        &[("name", &failed_name)],
                                    ),
                                    &e,
                                );
                            }
                        }
                        cx.notify();
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// Copy what was dropped on the tree into `dir`.
    pub fn drop_paths(
        &mut self,
        sources: Vec<PathBuf>,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_into(sources, dir, false, window, cx);
    }

    fn copy_into(
        &mut self,
        sources: Vec<PathBuf>,
        dir: PathBuf,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() {
            return;
        }
        let Some(host) = self.ctx.host.clone() else {
            return;
        };
        let id = host.id();
        let asked_for = sources.clone();
        let target = dir.clone();
        HostOps::run_in(
            host,
            window,
            cx,
            move |h| file_copy::copy_into_dir(h, &sources, &target, overwrite),
            move |this, report: file_copy::DropReport, window, cx| {
                if !report.copied.is_empty() {
                    this.invalidate_dir(id, &dir);
                }
                if !report.conflicts.is_empty() {
                    this.confirm_replace(asked_for, dir, report.conflicts, window, cx);
                } else if let Some((name, e)) = report.errors.first() {
                    let context = match report.errors.len() {
                        1 => t_fmt(L10nKey::FileDropFailed, &[("name", name)]),
                        n => t_fmt(
                            L10nKey::FileDropFailedMany,
                            &[("name", name), ("n", &(n - 1).to_string())],
                        ),
                    };
                    HostOps::notify_err(window, cx, &context, e);
                }
                cx.notify();
            },
        );
    }

    fn confirm_replace(
        &mut self,
        sources: Vec<PathBuf>,
        dir: PathBuf,
        conflicts: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = match conflicts.as_slice() {
            [one] => t_fmt(L10nKey::FileDropReplaceTitle, &[("name", one)]),
            many => t_fmt(
                L10nKey::FileDropReplaceManyTitle,
                &[("n", &many.len().to_string())],
            ),
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(t(L10nKey::FileDropReplaceBody)),
            &anterm_ui_kit::confirm_answers(t(L10nKey::FileDropReplace), t(L10nKey::Cancel)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(0) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.copy_into(sources, dir, true, window, cx);
            });
        })
        .detach();
    }

    fn decorations(
        &self,
        host: Option<&SharedHost>,
        host_id: HostId,
        roots: &[PathBuf],
        cx: &mut Context<Self>,
    ) -> Decorations {
        let mut decor: Decorations = Vec::new();
        for root in roots {
            if let Some(host) = host {
                scm_refresh(host.clone(), root.clone(), cx);
            }
            if let Some(index) = index_of(cx, host_id, root) {
                decor.push((root.clone(), index));
            }
        }
        order_innermost_first(&mut decor);
        decor
    }

    fn flatten_items(&self, rows: Vec<TreeRow>) -> Vec<FlatTreeItem> {
        let mut out = Vec::with_capacity(rows.len() + 1);
        for row in rows {
            let create_input = self.editing.as_ref().and_then(|edit| match edit {
                TreeEdit::NewFile { dir, input } | TreeEdit::NewFolder { dir, input }
                    if row.note.is_none() && *dir == row.entry.path =>
                {
                    Some((row.depth + 1, input.clone()))
                }
                _ => None,
            });
            out.push(FlatTreeItem::Row(row));
            if let Some((depth, input)) = create_input {
                out.push(FlatTreeItem::CreateInput { depth, input });
            }
        }
        out
    }

    fn render_flat_item(
        &self,
        item: &FlatTreeItem,
        decor: &Decorations,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            FlatTreeItem::Row(row) => {
                let deco = row_decoration(decor, &row.entry);
                self.render_tree_row(row, deco, window, cx)
            }
            FlatTreeItem::CreateInput { depth, input } => h_flex()
                .id("tree-create-input")
                .items_center()
                .gap_1()
                .h(px(TREE_ROW_H))
                .pl(px(ROW_INSET + *depth as f32 * INDENT))
                .pr(px(ROW_INSET))
                .py_0p5()
                .child(Input::new(input).xsmall())
                .into_any_element(),
        }
    }

    fn render_tree_row(
        &self,
        row: &TreeRow,
        deco: RowDeco,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = row.entry.path.clone();
        let is_dir = row.entry.is_dir;
        let selected = self
            .active_tab_state()
            .and_then(|c| c.selected.as_deref())
            == Some(&*path);
        let muted = cx.theme().muted_foreground;
        let paths_are_local = self.ctx.host_id.is_local();

        if let Some(note) = row.note {
            let (key, ink) = match note {
                TreeNote::Loading => (L10nKey::TreeDirLoading, muted),
                TreeNote::Empty => (L10nKey::TreeDirEmpty, muted),
                TreeNote::HiddenOnly => (L10nKey::TreeDirHiddenOnly, muted),
                TreeNote::Unreadable => (L10nKey::TreeDirUnreadable, cx.theme().danger),
                TreeNote::SearchCapped => (L10nKey::TreeSearchCapped, muted),
                TreeNote::SearchFailed => (L10nKey::TreeSearchFailed, cx.theme().danger),
            };
            return h_flex()
                .id(SharedString::from(format!(
                    "tree-note-{}-{note:?}",
                    path.display()
                )))
                .pl(px(ROW_INSET + row.depth as f32 * INDENT + LABEL_LEAD))
                .h(px(TREE_ROW_H))
                .items_center()
                .text_xs()
                .italic()
                .text_color(ink)
                .child(match note {
                    TreeNote::SearchCapped => t_fmt(key, &[("n", &SEARCH_LIMIT.to_string())]),
                    _ => t(key).to_string(),
                })
                .when(!path.as_os_str().is_empty(), |d| {
                    d.drag_over::<ExternalPaths>(|s, _, _, cx| {
                        s.bg(cx.theme().drag_border.opacity(0.14))
                    })
                    .on_drop(cx.listener(
                        move |this, paths: &ExternalPaths, window, cx| {
                            this.drop_paths(paths.paths().to_vec(), path.clone(), window, cx);
                        },
                    ))
                })
                .into_any_element();
        }

        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().popover;
        let dirty = self.ctx.dirty_paths.contains(&path);

        let renaming = matches!(
            &self.editing,
            Some(TreeEdit::Rename { path: p, .. }) if *p == path
        );

        let icon = if row.is_root {
            IconName::FolderOpen
        } else if is_dir {
            if row.expanded {
                IconName::FolderOpen
            } else {
                IconName::Folder
            }
        } else {
            IconName::File
        };

        let label: AnyElement = if renaming
            && let Some(editing) = self.editing.as_ref()
        {
            let input = editing.input().clone();
            Input::new(&input).xsmall().into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .text_sm()
                .text_color(if selected {
                    gpui::rgb(sf.text_selected).into()
                } else {
                    cx.theme().sidebar_foreground
                })
                .when(selected, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .when(row.entry.ignored, |d| d.text_color(muted))
                .when_some(deco.tint, |d, status| {
                    d.when(status == DecoStatus::Conflict, |d| {
                        d.text_color(status_color(status, cx))
                    })
                })
                .when(deco.strike, |d| d.line_through())
                .when(deco.bold, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .when(row.is_root, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .child(SharedString::from(row.entry.name.clone()))
                .into_any_element()
        };

        let chevron = is_dir.then(|| {
            Icon::new(match row.expanded || row.is_root {
                true => IconName::ChevronDown,
                false => IconName::ChevronRight,
            })
            .size(px(CHEVRON_W))
            .text_color(muted)
        });
        h_flex()
            .id(SharedString::from(format!("tree-{}", path.display())))
            .role(gpui::Role::TreeItem)
            .aria_label(SharedString::from(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
            ))
            .aria_selected(selected)
            .when(is_dir, |r| r.aria_expanded(row.expanded))
            .items_center()
            .gap(px(TREE_GAP))
            .h(px(TREE_ROW_H))
            .pl(px(ROW_INSET + row.depth as f32 * INDENT))
            .pr(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .when(selected, |d| d.bg(gpui::rgb(sf.selected)))
            .when(!selected, |d| d.hover(|s| s.bg(gpui::rgb(sf.hover))))
            .child(
                div()
                    .flex_none()
                    .w(px(CHEVRON_W))
                    .flex()
                    .justify_center()
                    .children(chevron),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .when(row.entry.ignored, |d| d.opacity(0.5))
                    .child(Icon::new(icon).size(px(ROW_GLYPH)).text_color(muted)),
            )
            .child(label)
            .when_some(is_dir.then_some(deco.tint).flatten(), |d, tint| {
                d.child(
                    div()
                        .flex_none()
                        .size(px(5.))
                        .rounded_full()
                        .bg(status_color(tint, cx)),
                )
            })
            .when(dirty, |d| {
                d.child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(cx.theme().warning),
                )
            })
            .when_some(deco.badge(), |d, (letter, status)| {
                d.child(git_badge(
                    letter,
                    status_color(status, cx),
                    &cx.theme().mono_font_family,
                ))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let path = path.clone();
                    move |this, _, window, cx| {
                        this.focus_handle.focus(window, cx);
                        this.activate(&path, is_dir, window, cx);
                    }
                }),
            )
            .on_a11y_action(gpui::AccessibleAction::Click, {
                let path = path.clone();
                let view = cx.entity().downgrade();
                move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| {
                        this.focus_handle.focus(window, cx);
                        this.activate(&path, is_dir, window, cx);
                    });
                }
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let path = path.clone();
                    move |this, _, _window, cx| {
                        this.active_tab_state_mut().selected = Some(path.clone());
                        cx.notify();
                    }
                }),
            )
            .map(|d| {
                let name = row.entry.name.clone();
                let ghost = move |cx: &mut App| {
                    let name = name.clone();
                    cx.new(|_| DragGhost { name })
                };
                match paths_are_local {
                    true => d.on_drag(
                        ExternalPaths(vec![path.clone()].into()),
                        move |_, _, _, cx| ghost(cx),
                    ),
                    false => d.on_drag(
                        RemotePathDrag { path: path.clone() },
                        move |_, _, _, cx| ghost(cx),
                    ),
                }
            })
            .drag_over::<ExternalPaths>(|s, _, _, cx| s.bg(cx.theme().drag_border.opacity(0.14)))
            .on_drop(cx.listener({
                let dir = match is_dir {
                    true => path.clone(),
                    false => path.parent().unwrap_or(&path).to_path_buf(),
                };
                move |this, paths: &ExternalPaths, window, cx| {
                    this.drop_paths(paths.paths().to_vec(), dir.clone(), window, cx);
                }
            }))
            .context_menu({
                let view = cx.entity().downgrade();
                let path = path.clone();
                let is_root = row.is_root;
                let show_hidden = self.show_hidden;
                move |menu, _window, cx| {
                    Self::row_context_menu(
                        menu,
                        &path,
                        is_dir,
                        is_root,
                        show_hidden,
                        paths_are_local,
                        &view,
                        cx,
                    )
                }
            })
            .into_any_element()
    }

    fn row_context_menu(
        menu: PopupMenu,
        path: &Path,
        is_dir: bool,
        is_root: bool,
        show_hidden: bool,
        paths_are_local: bool,
        view: &gpui::WeakEntity<Self>,
        cx: &App,
    ) -> PopupMenu {
        let danger = cx.theme().danger;
        let mut menu = menu.min_w(px(200.));
        let p = path.to_path_buf();

        if !is_dir {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextOpen)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, _window, cx| {
                        let _ = view.update(cx, |_, cx| {
                            cx.emit(FileTreeEvent::OpenFile(p.clone()));
                        });
                    }
                }),
            );
        }
        if is_dir {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextCdHere)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, _window, cx| {
                        let _ = view.update(cx, |_, cx| {
                            cx.emit(FileTreeEvent::CdToDir(p.clone()));
                        });
                    }
                }),
            );
        }
        menu = menu
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextInsertPath)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, _window, cx| {
                        let _ = view.update(cx, |_, cx| {
                            cx.emit(FileTreeEvent::InsertPath(p.clone()));
                        });
                    }
                }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextAttachAgent)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, _window, cx| {
                        let _ = view.update(cx, |_, cx| {
                            cx.emit(FileTreeEvent::AttachToAgent(p.clone()));
                        });
                    }
                }),
            )
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextNewFile)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.begin_edit(TreeEditKind::NewFile, &p, is_dir, window, cx)
                        });
                    }
                }),
            )
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextNewFolder)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.begin_edit(TreeEditKind::NewFolder, &p, is_dir, window, cx)
                        });
                    }
                }),
            );

        if !is_root {
            menu = menu.item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextRename)).on_click({
                    let view = view.clone();
                    let p = p.clone();
                    move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| {
                            this.begin_edit(TreeEditKind::Rename, &p, is_dir, window, cx)
                        });
                    }
                }),
            );
        }

        menu = menu.separator().item(
            PopupMenuItem::new(t(L10nKey::FileTreeContextCopyPath)).on_click({
                let text = match paths_are_local {
                    true => anterm_ui_kit::path_display::native_separators(&p)
                        .display()
                        .to_string(),
                    false => p.display().to_string(),
                };
                move |_, _window, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                }
            }),
        );
        if paths_are_local {
            menu = menu.item(
                PopupMenuItem::new(crate::right_panel::reveal_label()).on_click({
                    let p = p.clone();
                    move |_, _window, cx| {
                        cx.reveal_path(&anterm_ui_kit::path_display::native_separators(&p));
                    }
                }),
            );
        } else if !is_dir {
            menu = menu.item(PopupMenuItem::new(t(L10nKey::Download)).on_click({
                let view = view.clone();
                let p = p.clone();
                move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.download(&p, window, cx));
                }
            }));
        }

        menu = menu.separator().item(dotfiles_menu_item(show_hidden, view));

        if !is_root {
            menu = menu.separator().item(
                PopupMenuItem::element(move |_window, _cx| {
                    div().text_color(danger).child(t(L10nKey::Delete))
                })
                .on_click({
                    let view = view.clone();
                    move |_, window, cx| {
                        let p = p.clone();
                        let _ = view.update(cx, |this, cx| this.delete(p, is_dir, window, cx));
                    }
                }),
            );
        }
        menu
    }
}

impl Render for FileTreeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_roots(cx);
        if let Some((path, _)) = self.tree_reveal.clone() {
            self.expand_ancestors(&path);
        }
        let (roots, expanded) = match self.active_tab_state() {
            Some(code) => (code.roots.clone(), code.expanded.clone()),
            None => (Vec::new(), HashSet::new()),
        };
        let query = self.query(cx);
        let searching = !query.is_empty();
        let host = self.ctx.host.clone();
        let host_id = self.ctx.host_id;
        if let Some(host) = host.clone() {
            self.sync_watch_all(host, cx);
        }
        let decor = self.decorations(host.as_ref(), host_id, &roots, cx);
        self.sync_search(&query, &roots, cx);

        let search_target = if self.ctx.active_is_ssh {
            PanelSearchTarget::SshPane
        } else {
            match self.resolve_project_roots(cx) {
                None => PanelSearchTarget::Resolving,
                Some((_, resolved_roots)) if resolved_roots.is_empty() => {
                    PanelSearchTarget::NoFolder
                }
                Some((resolved_host, resolved_roots)) => PanelSearchTarget::Ready {
                    host: resolved_host.id(),
                    roots: resolved_roots,
                    home: self.ctx.display_home.clone(),
                },
            }
        };
        let has_contents = self
            .panel_search
            .update(cx, |s, cx| s.sync_target(search_target, searching, cx));

        let rows = if searching {
            self.search_rows()
        } else {
            if let Some(host) = &host {
                self.request_loads(host, &roots, &expanded, cx);
            }
            self.visible_rows(host_id, &roots, &expanded)
        };

        if searching {
            let names_heading = (!rows.is_empty()).then(|| {
                let found = rows.iter().filter(|r| r.note.is_none()).count();
                search_section_heading(
                    t(L10nKey::PanelFilesNameMatches),
                    Some(found.to_string()),
                    cx,
                )
            });
            let flat = self.flatten_items(rows);
            let rendered: Vec<AnyElement> = flat
                .iter()
                .map(|item| self.render_flat_item(item, &decor, window, cx))
                .collect();
            let column = v_flex()
                .id("right-panel-tree-search-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.search_scroll)
                .px(px(CONTENT_INSET))
                .pb(px(TREE_PAD_BOTTOM))
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    this.key_down(ev, window, cx);
                }))
                .children(names_heading)
                .children(rendered)
                .when(has_contents, |column| {
                    column
                        .child(div().flex_none().h(px(SEARCH_SECTION_GAP)))
                        .child(self.panel_search.clone())
                });
            return anterm_ui_kit::scrollbar::with_vertical_scrollbar(
                "right-panel-tree-search-scrollbar",
                column,
                &self.search_scroll,
            )
            .into_any_element();
        }

        let flat = self.flatten_items(rows);
        if let Some((path, left)) = self.tree_reveal.clone() {
            let reveal_ix = flat.iter().position(|item| match item {
                FlatTreeItem::Row(row) => row.note.is_none() && row.entry.path == path,
                FlatTreeItem::CreateInput { .. } => false,
            });
            match reveal_ix {
                Some(ix) => {
                    self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
                    self.tree_reveal = None;
                }
                None => {
                    self.tree_reveal = left.checked_sub(1).map(|left| (path, left));
                }
            }
        }

        if flat.is_empty() {
            let text = t(L10nKey::OpenFileFromTree).to_string();
            return v_flex()
                .id("right-panel-tree-empty")
                .flex_1()
                .min_h_0()
                .px(px(CONTENT_INSET))
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    this.key_down(ev, window, cx);
                }))
                .child(
                    div()
                        .px(px(ROW_INSET))
                        .py_4()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(text),
                )
                .into_any_element();
        }

        let item_sizes: Rc<Vec<Size<Pixels>>> =
            Rc::new(vec![size(px(0.), px(TREE_ROW_H)); flat.len()]);
        self.rendered_items = flat;
        self.rendered_decor = decor;

        let list = v_virtual_list(
            cx.entity(),
            "right-panel-tree-rows",
            item_sizes,
            |this, range, window, cx| {
                #[cfg(test)]
                row_probe::record(range.len());
                range
                    .filter_map(|ix| this.rendered_items.get(ix))
                    .map(|item| {
                        div()
                            .w_full()
                            .px(px(CONTENT_INSET))
                            .child(this.render_flat_item(item, &this.rendered_decor, window, cx))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .w_full()
        .pb(px(TREE_PAD_BOTTOM));

        let body = anterm_ui_kit::scrollbar::with_vertical_scrollbar(
            "right-panel-tree-scrollbar",
            list,
            &self.scroll,
        );

        v_flex()
            .id("right-panel-tree-host")
            .flex_1()
            .min_h_0()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.key_down(ev, window, cx);
            }))
            .when_some(roots.first().cloned(), |d, root| {
                d.drag_over::<ExternalPaths>(|s, _, _, cx| {
                    s.bg(cx.theme().drag_border.opacity(0.06))
                })
                .on_drop(cx.listener(
                    move |this, paths: &ExternalPaths, window, cx| {
                        this.drop_paths(paths.paths().to_vec(), root.clone(), window, cx);
                    },
                ))
            })
            .child(body)
            .into_any_element()
    }
}

fn dotfiles_menu_item(show_hidden: bool, view: &gpui::WeakEntity<FileTreeView>) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(if show_hidden {
        t(L10nKey::FileTreeContextHideDotfiles)
    } else {
        t(L10nKey::FileTreeContextShowDotfiles)
    })
    .on_click(move |_, _window, cx| {
        let _ = view.update(cx, |this, cx| {
            this.show_hidden = !this.show_hidden;
            cx.notify();
        });
    })
}

struct DragGhost {
    name: String,
}

pub use anterm_ui_kit::panel_tokens::RemotePathDrag;

impl gpui::Render for DragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .text_sm()
            .child(Icon::new(IconName::File).size(px(ROW_GLYPH)))
            .child(SharedString::from(self.name.clone()))
    }
}

fn land_listing(
    loads: &mut InFlight<DirKey>,
    children: &mut ByHost<PathBuf, Vec<TreeEntry>>,
    stale: &mut HashSet<DirKey>,
    key: &DirKey,
    id: HostId,
    dir: PathBuf,
    entries: Vec<TreeEntry>,
) -> bool {
    let superseded = !loads.finish(key);
    children.insert(id, dir, entries);
    stale.remove(key);
    superseded
}

fn dirs_to_relist(paths: &HashSet<PathBuf>, show_hidden: bool) -> HashSet<&Path> {
    paths
        .iter()
        .filter(|p| event_can_change_a_row(p, show_hidden))
        .filter_map(|p| p.parent())
        .collect()
}

/// Every repository behind the tree, paired with the root its index keys are
/// relative to. Built once per render and ordered innermost-first.
type Decorations = Vec<(PathBuf, Arc<StatusIndex>)>;

/// What git says about one row: a letter for the trailing badge and the shape
/// of the name beside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct RowDeco {
    letter: &'static str,
    tint: Option<DecoStatus>,
    strike: bool,
    bold: bool,
}

impl RowDeco {
    fn file(status: DecoStatus) -> RowDeco {
        RowDeco {
            letter: status_glyph(status),
            tint: Some(status),
            strike: status == DecoStatus::Deleted,
            bold: status == DecoStatus::Conflict,
        }
    }

    fn dir(rollup: DirRollup) -> RowDeco {
        let tint = if rollup.conflict {
            Some(DecoStatus::Conflict)
        } else if rollup.changed {
            Some(DecoStatus::Modified)
        } else {
            None
        };
        RowDeco {
            tint,
            ..RowDeco::default()
        }
    }

    fn badge(&self) -> Option<(&'static str, DecoStatus)> {
        let status = self.tint?;
        (!self.letter.is_empty()).then_some((self.letter, status))
    }
}

fn repo_relative<'a>(root: &Path, path: &'a Path) -> Option<Cow<'a, str>> {
    let rel = path.strip_prefix(root).ok()?.to_str()?;
    if rel.is_empty() {
        return None;
    }
    Some(with_forward_slashes(rel, std::path::MAIN_SEPARATOR))
}

fn with_forward_slashes(text: &str, sep: char) -> Cow<'_, str> {
    if sep == '/' || !text.contains(sep) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace(sep, "/"))
}

fn order_innermost_first(decor: &mut Decorations) {
    decor.sort_by_key(|(root, _)| std::cmp::Reverse(root.as_os_str().len()));
}

fn row_decoration(decor: &Decorations, entry: &TreeEntry) -> RowDeco {
    if entry.ignored {
        return RowDeco::default();
    }
    for (root, index) in decor {
        let Some(rel) = repo_relative(root, &entry.path) else {
            continue;
        };
        return if entry.is_dir {
            index.dir(&rel).map(RowDeco::dir).unwrap_or_default()
        } else {
            index.file(&rel).map(RowDeco::file).unwrap_or_default()
        };
    }
    RowDeco::default()
}

fn event_can_change_a_row(path: &Path, show_hidden: bool) -> bool {
    show_hidden
        || !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// Between the Files tab's two search sections.
const SEARCH_SECTION_GAP: f32 = 10.;

/// A heading over one of the Files tab's search sections — "File names",
/// "In file contents" — with its tally at the trailing edge, in caption ink.
pub fn search_section_heading(label: &str, tally: Option<String>, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .flex_none()
        .h(px(22.))
        .px(px(ROW_INSET))
        .items_center()
        .gap(px(8.))
        .text_size(gpui::rems(crate::right_panel::HEADING))
        .text_color(muted)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label.to_string()),
        )
        .children(tally.map(|n| {
            div()
                .flex_none()
                .font_features(anterm_ui_kit::theme::tabular_figures())
                .child(n)
        }))
        .into_any_element()
}
