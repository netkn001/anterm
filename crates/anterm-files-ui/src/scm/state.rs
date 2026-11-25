//! Everything the source control panel remembers between frames.
//!
//! Hosted by standalone [`ScmPanelView`] (`impl Render`, `impl EventEmitter<ScmEvent>`).
//! Commit drafts are keyed by repository rather than by tab or pane: a working
//! tree has one pending message, no matter how many panes are looking at it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use anterm_core::git::log::{Commit, CommitFile, CommitPage, GraphScope};
use anterm_core::git::status::{HeadState, RepoPath, StatusEntry};
use gpui::{Entity, EventEmitter};
use gpui_kit::component::VirtualListScrollHandle;
use gpui_kit::component::input::{InputState, TextareaState};

use anterm_ui_kit::git::diff::DiffSource;
use anterm_ui_kit::host_ops::{HostId, SharedHost};

/// Which working tree a piece of state belongs to.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RepoKey {
    pub host: HostId,
    pub root: PathBuf,
}

/// The four sections of the file list, in the order they are rendered.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ScmGroup {
    Merge,
    Staged,
    Changes,
    Untracked,
}

impl ScmGroup {
    pub const ORDER: [ScmGroup; 4] = [
        ScmGroup::Merge,
        ScmGroup::Staged,
        ScmGroup::Changes,
        ScmGroup::Untracked,
    ];
}

/// Workspace context pushed into [`ScmPanelView`] before rendering.
#[derive(Clone, Default)]
pub struct ScmContext {
    pub active_tab: usize,
    pub pane_target: Option<(SharedHost, PathBuf)>,
    pub diff_focus: Option<(HostId, PathBuf, DiffSource, String)>,
    pub panel_width: f32,
}

/// Events emitted by [`ScmPanelView`] to its host workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScmEvent {
    OpenDiff {
        host: HostId,
        root: PathBuf,
        source: DiffSource,
        focus: Option<String>,
    },
    OpenFile(PathBuf),
}

/// One flattened item inside the virtualized changed-files list.
pub enum ScmListItem {
    Spacer,
    GroupHeader {
        group: ScmGroup,
        count: usize,
        collapsed: bool,
        writable: Vec<RepoPath>,
    },
    DirRow {
        group: ScmGroup,
        key: String,
        label: String,
        depth: usize,
        files: usize,
        collapsed: bool,
        foldable: bool,
    },
    FileRow {
        group: ScmGroup,
        entry: StatusEntry,
        depth: Option<usize>,
    },
    Note(String),
}

pub struct ScmPanelView {
    pub ctx: ScmContext,
    /// Which repository the panel is showing. Follows the active pane unless
    /// `repo_override` says otherwise.
    pub repo: Option<RepoKey>,
    /// Set when the user picks a repository from the multi-repo dropdown, and
    /// cleared whenever the active tab changes.
    pub repo_override: Option<RepoKey>,
    /// The tab the override was made on, so the jump away can be noticed.
    pub override_tab: Option<usize>,
    /// Local branch names per repository, with the epoch they were read at.
    pub branches: HashMap<RepoKey, (u64, Vec<String>)>,
    pub branches_loading: HashSet<RepoKey>,
    /// The inline "name your branch" input, present only while it is open.
    pub new_branch: Option<Entity<InputState>>,
    /// The inline "switch to which branch" input, present only while it is open.
    pub checkout_branch: Option<Entity<InputState>>,
    /// Unsent commit messages, one per working tree.
    pub drafts: HashMap<RepoKey, String>,
    /// The commit box, lazily initialized on first render.
    pub commit_input: Option<Entity<TextareaState>>,
    /// Which repository's draft the box is currently holding.
    pub commit_repo: Option<RepoKey>,
    /// A commit that has been dispatched: (repo, head_before, message).
    pub committing: Option<(RepoKey, HeadState, String)>,
    /// Whether the next commit rewrites HEAD.
    pub amend: bool,
    /// Groups the user folded shut, and the ones whose fold state they touched.
    pub collapsed: HashSet<ScmGroup>,
    pub toggled: HashSet<ScmGroup>,
    /// Directories folded shut in the tree view, per group.
    pub folded_dirs: HashSet<(ScmGroup, String)>,
    /// The filter over the changed files.
    pub filter: Option<Entity<InputState>>,
    pub filter_sub: Option<gpui::Subscription>,
    /// Working directory -> resolved repository root.
    pub roots: HashMap<(HostId, PathBuf), (std::time::Instant, Option<PathBuf>)>,
    pub root_lookups: HashSet<(HostId, PathBuf)>,
    /// When the panel last asked for a status that it did not get back.
    pub probe_attempt: HashMap<(HostId, PathBuf), std::time::Instant>,
    /// The status the last frame drew, as (cache key, `Arc` identity).
    pub seen: Option<((HostId, PathBuf), usize)>,
    pub watch: Option<gpui::Subscription>,
    /// The cheap per-tab git status the panel last reacted to.
    pub last_tab_status: Option<anterm_ui_kit::git::status::GitStatus>,
    pub graph: GraphState,
    /// When set, the panel body is replaced by a single commit's detail view.
    pub detail: Option<CommitDetailView>,
    pub groups_scroll: VirtualListScrollHandle,
    pub detail_scroll: gpui::ScrollHandle,
    pub rendered_items: Vec<ScmListItem>,
    pub rendered_repo: Option<RepoKey>,
}

impl Default for ScmPanelView {
    fn default() -> Self {
        Self::new(false)
    }
}

impl EventEmitter<ScmEvent> for ScmPanelView {}

impl ScmPanelView {
    pub fn new(graph_expanded: bool) -> Self {
        Self {
            ctx: ScmContext::default(),
            repo: None,
            repo_override: None,
            override_tab: None,
            branches: HashMap::new(),
            branches_loading: HashSet::new(),
            new_branch: None,
            checkout_branch: None,
            drafts: HashMap::new(),
            commit_input: None,
            commit_repo: None,
            committing: None,
            amend: false,
            collapsed: HashSet::new(),
            toggled: HashSet::new(),
            folded_dirs: HashSet::new(),
            filter: None,
            filter_sub: None,
            roots: HashMap::new(),
            root_lookups: HashSet::new(),
            probe_attempt: HashMap::new(),
            seen: None,
            watch: None,
            last_tab_status: None,
            graph: GraphState {
                expanded: graph_expanded,
                ..Default::default()
            },
            detail: None,
            groups_scroll: VirtualListScrollHandle::new(),
            detail_scroll: gpui::ScrollHandle::new(),
            rendered_items: Vec::new(),
            rendered_repo: None,
        }
    }

    pub fn sync_context(&mut self, ctx: ScmContext) {
        self.ctx = ctx;
    }

    /// Currently focused file path in the diff overlay if it matches `(host, root, source)`.
    pub fn diff_focus_for(
        &self,
        host: HostId,
        root: &std::path::Path,
        source: &DiffSource,
    ) -> Option<&str> {
        let (h, r, s, path) = self.ctx.diff_focus.as_ref()?;
        (*h == host && r == root && s == source).then_some(path.as_str())
    }

    /// Total working-tree entries for the active repository, if known.
    pub fn total_changes(&self, cx: &gpui::App) -> Option<usize> {
        let repo = self.active_repo()?;
        let status = anterm_ui_kit::git::data::status_of(cx, repo.host, &repo.root)?;
        (status.total_entries > 0).then_some(status.total_entries)
    }

    /// The repository the panel should act on: an explicit pick wins over
    /// whatever the active pane happens to be sitting in.
    pub fn active_repo(&self) -> Option<&RepoKey> {
        self.repo_override.as_ref().or(self.repo.as_ref())
    }

    /// Drop everything keyed to a host that left the registry.
    pub fn forget_host(&mut self, host: HostId) {
        self.roots.retain(|(h, _), _| *h != host);
        self.root_lookups.retain(|(h, _)| *h != host);
        self.probe_attempt.retain(|(h, _), _| *h != host);
        self.branches.retain(|k, _| k.host != host);
        self.branches_loading.retain(|k| k.host != host);
        if self.repo_override.as_ref().is_some_and(|k| k.host == host) {
            self.repo_override = None;
            self.override_tab = None;
        }
    }

    /// A probe just answered "no repository" for `root`: every directory that
    /// resolved to it has to re-ask.
    pub fn forget_root(&mut self, host: HostId, root: &std::path::Path) {
        self.roots
            .retain(|(h, _), (_, held)| !(*h == host && held.as_deref() == Some(root)));
    }

    pub fn draft(&self, repo: &RepoKey) -> &str {
        self.drafts.get(repo).map(String::as_str).unwrap_or("")
    }

    /// Whether a group renders folded. `count` decides it only for a group the
    /// user has never touched.
    pub fn group_collapsed(&self, group: ScmGroup, count: usize) -> bool {
        if self.toggled.contains(&group) {
            self.collapsed.contains(&group)
        } else {
            crate::scm::panel::starts_collapsed(group, count)
        }
    }

    /// Fold a tree directory shut, or open it again.
    pub fn toggle_dir(&mut self, group: ScmGroup, key: String) {
        let entry = (group, key);
        if !self.folded_dirs.remove(&entry) {
            self.folded_dirs.insert(entry);
        }
    }

    pub fn set_group_collapsed(&mut self, group: ScmGroup, collapsed: bool) {
        self.toggled.insert(group);
        if collapsed {
            self.collapsed.insert(group);
        } else {
            self.collapsed.remove(&group);
        }
    }
}

#[derive(Default)]
pub struct GraphState {
    pub expanded: bool,
    pub requested: usize,
    pub loading: bool,
    pub page: Option<Arc<CommitPage>>,
    pub page_key: Option<(RepoKey, u64, GraphScope)>,
    pub failed_key: Option<(RepoKey, u64, GraphScope)>,
    pub search: Option<Entity<InputState>>,
    pub search_sub: Option<gpui::Subscription>,
    pub filter_cache: Option<(Option<String>, Weak<CommitPage>, Arc<Vec<usize>>)>,
    pub naming: Option<(Entity<InputState>, String)>,
    pub scope: GraphScope,
    pub selected: Option<String>,
    pub scroll: gpui::ScrollHandle,
    pub height: std::rc::Rc<std::cell::Cell<f32>>,
    pub dragging: std::rc::Rc<std::cell::Cell<bool>>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommitDetailView {
    pub repo: RepoKey,
    pub oid: String,
    pub loading: bool,
    pub loaded: bool,
    pub commit: Option<Arc<Commit>>,
    pub files: Option<Arc<Vec<CommitFile>>>,
    pub files_failed: bool,
    pub body_expanded: bool,
}

impl CommitDetailView {
    pub fn new(repo: RepoKey, oid: String, seed: Option<Commit>) -> CommitDetailView {
        CommitDetailView {
            repo,
            oid,
            loading: false,
            loaded: false,
            commit: seed.map(Arc::new),
            files: None,
            files_failed: false,
            body_expanded: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(root: &str) -> RepoKey {
        RepoKey {
            host: HostId::LOCAL,
            root: PathBuf::from(root),
        }
    }

    #[test]
    fn an_explicit_repo_pick_outranks_the_active_pane() {
        let mut state = ScmPanelView::default();
        assert!(state.active_repo().is_none());
        state.repo = Some(key("/a"));
        assert_eq!(state.active_repo(), Some(&key("/a")));
        state.repo_override = Some(key("/b"));
        assert_eq!(state.active_repo(), Some(&key("/b")));
        state.repo_override = None;
        assert_eq!(state.active_repo(), Some(&key("/a")));
    }

    #[test]
    fn drafts_are_keyed_by_repository_not_by_path_alone() {
        let mut state = ScmPanelView::default();
        state.drafts.insert(key("/a"), "wip".into());
        assert_eq!(state.draft(&key("/a")), "wip");
        assert_eq!(state.draft(&key("/b")), "");
    }
}
