//! Where the source control actions and palette commands land.
//!
//! One `ScmIntent` match, so the four ways of asking for a verb — the action,
//! the key binding, the palette entry and the button on the row — cannot drift
//! into meaning different things.

use gpui::{Context, PromptButton, PromptLevel, Window};

use anterm_core::git::ops::{Destructive, GitOp, PullMode};

use anterm_ui_kit::config_store::ConfigStore;
use crate::scm::state::{RepoKey, ScmPanelView};
use anterm_ui_kit::host_registry::HostRegistry;
use anterm_i18n::{L10nKey, t, t_fmt};

/// One entry point for every source control verb.
///
/// A single enum rather than fourteen methods: the actions, the palette and
/// the row buttons all want the same behaviour, and routing them through one
/// match is what keeps the three from drifting apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScmIntent {
    Commit,
    CommitAmend,
    /// Commit, then send it on. Two operations rather than one, so the commit
    /// still stands if the network half fails — and strictly in that order:
    /// the send rides in the commit's [`ScmFollowUp`], because a push
    /// dispatched alongside the commit resolves the branch tip whenever the
    /// pool gets to it, and pushing the *old* tip reports success while
    /// sending nothing.
    CommitAndPush,
    CommitAndSync,
    StageAll,
    UnstageAll,
    DiscardAll,
    Refresh,
    Sync,
    Push,
    Pull,
    Fetch,
    CheckoutBranch,
    CreateBranch,
}

/// What to run once an operation has landed *successfully*.
///
/// Compound verbs — commit-and-push, pull-then-push, discard-all's two halves
/// — are sequences, not bundles: the second operation only makes sense against
/// the repository the first one produced. Dispatching both into the worker
/// pool at once lets them race, so the second rides here and is started from
/// the first one's landing closure instead. A failed or cancelled first half
/// drops the follow-up.
#[derive(Clone, Debug)]
pub enum ScmFollowUp {
    /// Push the current branch, re-reading the (by then updated) status.
    Push,
    /// Pull, then push — the whole sync sequence.
    Sync,
    /// One more operation, run without a second confirmation: the prompt that
    /// approved the first half covered this one too.
    Op(GitOp),
}

impl ScmPanelView {
    /// Fold the history section open or shut and remember it.
    pub fn toggle_graph(&mut self, cx: &mut Context<Self>) {
        let next = !self.graph.expanded;
        self.graph.expanded = next;
        let _ = ConfigStore::update(cx, |cfg| cfg.scm_graph_expanded = next);
        cx.notify();
    }

    /// Switch the changed-file list between flat and tree, and remember it.
    pub fn toggle_changes_tree(&mut self, cx: &mut Context<Self>) {
        let next = !cx.global::<anterm_ui_kit::config::Config>().scm_changes_tree;
        let _ = ConfigStore::update(cx, |cfg| cfg.scm_changes_tree = next);
        cx.notify();
    }

    /// Run one operation against the panel's repository, asking first when it
    /// can lose work.
    pub fn scm_op(
        &mut self,
        repo: RepoKey,
        op: GitOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scm_op_then(repo, op, None, window, cx);
    }

    /// [`scm_op`], with something to run once this operation has succeeded.
    /// Cancelling the confirmation drops the follow-up along with the op.
    pub fn scm_op_then(
        &mut self,
        repo: RepoKey,
        op: GitOp,
        then: Option<ScmFollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = HostRegistry::get(cx, repo.host) else {
            return;
        };
        let Some(loss) = op.destructive() else {
            self.run_git_op(host, repo.root, op, then, window, cx);
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &confirm_question(&op, loss),
            None,
            &guarded_answers(confirm_verb(&op, loss)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(1) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_git_op(host, repo.root, op, then, window, cx)
            });
        })
        .detach();
    }

    pub fn run_action(
        &mut self,
        intent: ScmIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo().cloned() else {
            return;
        };
        match intent {
            ScmIntent::Refresh => {
                anterm_ui_kit::git::data::scm_invalidate(repo.host, &repo.root, cx);
                cx.notify();
            }
            ScmIntent::StageAll => self.scm_op(repo, GitOp::StageAll, window, cx),
            ScmIntent::UnstageAll => self.scm_op(repo, GitOp::UnstageAll, window, cx),
            ScmIntent::DiscardAll => self.scm_discard_all(repo, window, cx),
            ScmIntent::Commit => {
                let amend = self.amend;
                self.scm_commit(repo, amend, None, window, cx);
            }
            ScmIntent::CommitAmend => self.scm_commit(repo, true, None, window, cx),
            ScmIntent::CommitAndPush => {
                let amend = self.amend;
                self.scm_commit(repo, amend, Some(ScmFollowUp::Push), window, cx);
            }
            ScmIntent::CommitAndSync => {
                let amend = self.amend;
                self.scm_commit(repo, amend, Some(ScmFollowUp::Sync), window, cx);
            }
            ScmIntent::Sync => self.scm_sync(repo, window, cx),
            ScmIntent::Push => self.scm_push(repo, false, window, cx),
            ScmIntent::Pull => self.scm_op(
                repo,
                GitOp::Pull {
                    mode: PullMode::FfOnly,
                },
                window,
                cx,
            ),
            ScmIntent::Fetch => self.scm_op(
                repo,
                GitOp::Fetch {
                    remote: None,
                    prune: false,
                },
                window,
                cx,
            ),
            ScmIntent::CreateBranch => self.begin_create_branch(window, cx),
            ScmIntent::CheckoutBranch => self.begin_checkout_branch(window, cx),
        }
    }

    /// Commit whatever the message box holds.
    pub fn scm_commit(
        &mut self,
        repo: RepoKey,
        amend: bool,
        then: Option<ScmFollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = anterm_ui_kit::git::data::status_of(cx, repo.host, &repo.root) else {
            return;
        };
        let message = self.scm_message(&repo, cx);
        let plan = crate::scm::panel::commit_plan(&status, amend, &message);
        if !plan.enabled {
            gpui_kit::component::WindowExt::push_notification(
                window,
                t(plan.reason).to_string(),
                cx,
            );
            return;
        }
        let all = crate::scm::panel::commit_stages_everything(&status, amend);
        self.scm_op_then(
            repo,
            GitOp::Commit {
                message,
                amend,
                signoff: false,
                no_verify: false,
                all,
            },
            then,
            window,
            cx,
        );
    }

    fn scm_message(&self, repo: &RepoKey, cx: &gpui::App) -> String {
        match (&self.commit_input, &self.commit_repo) {
            (Some(input), Some(showing)) if showing == repo => input.read(cx).value().to_string(),
            _ => self.draft(repo).to_string(),
        }
    }

    pub fn scm_stash_all(
        &mut self,
        repo: RepoKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match self.scm_message(&repo, cx) {
            m if m.trim().is_empty() => None,
            m => Some(m),
        };
        self.scm_op(
            repo,
            GitOp::Stash {
                message,
                include_untracked: true,
            },
            window,
            cx,
        );
    }

    fn scm_discard_all(&mut self, repo: RepoKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = anterm_ui_kit::git::data::status_of(cx, repo.host, &repo.root) else {
            return;
        };
        let (first, second) = match &discard_all_ops(&status)[..] {
            [] => return,
            [one] => (one.clone(), None),
            [a, b, ..] => (a.clone(), Some(b.clone())),
        };
        let Some(host) = HostRegistry::get(cx, repo.host) else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &t(L10nKey::ScmDiscardAllConfirm).to_string(),
            None,
            &guarded_answers(t(L10nKey::ScmDiscard)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(1) = answer.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_git_op(
                    host,
                    repo.root,
                    first,
                    second.map(ScmFollowUp::Op),
                    window,
                    cx,
                )
            });
        })
        .detach();
    }

    pub fn scm_push(
        &mut self,
        repo: RepoKey,
        force_with_lease: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = anterm_ui_kit::git::data::status_of(cx, repo.host, &repo.root) else {
            return;
        };
        let name = match crate::scm::panel::pushable_branch(&status.head) {
            Ok(name) => name.to_string(),
            Err(reason) => {
                gpui_kit::component::WindowExt::push_notification(window, t(reason).to_string(), cx);
                return;
            }
        };
        let (remote, branch) = match status.upstream.as_deref().and_then(split_upstream) {
            Some((remote, branch)) => (remote.to_string(), branch.to_string()),
            None => ("origin".to_string(), name),
        };
        let set_upstream = status.upstream.is_none();
        self.scm_op(
            repo,
            GitOp::Push {
                remote,
                branch,
                set_upstream,
                force_with_lease,
            },
            window,
            cx,
        );
    }

    pub fn scm_sync(&mut self, repo: RepoKey, window: &mut Window, cx: &mut Context<Self>) {
        let has_upstream = anterm_ui_kit::git::data::status_of(cx, repo.host, &repo.root)
            .is_some_and(|s| s.upstream.is_some());
        if has_upstream {
            self.scm_op_then(
                repo,
                GitOp::Pull {
                    mode: PullMode::FfOnly,
                },
                Some(ScmFollowUp::Push),
                window,
                cx,
            );
        } else {
            self.scm_push(repo, false, window, cx);
        }
    }

    pub fn scm_follow_up(
        &mut self,
        host: anterm_core::host::HostId,
        root: std::path::PathBuf,
        follow: ScmFollowUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo = RepoKey { host, root };
        match follow {
            ScmFollowUp::Push => self.scm_push(repo, false, window, cx),
            ScmFollowUp::Sync => self.scm_sync(repo, window, cx),
            ScmFollowUp::Op(op) => {
                let Some(shared) = HostRegistry::get(cx, repo.host) else {
                    return;
                };
                self.run_git_op(shared, repo.root, op, None, window, cx);
            }
        }
    }

    pub fn scm_commit_failed(
        &mut self,
        host: anterm_core::host::HostId,
        root: &std::path::Path,
    ) {
        if self
            .committing
            .as_ref()
            .is_some_and(|(r, _, _)| r.host == host && r.root == root)
        {
            self.committing = None;
        }
    }

    /// Change the repository, then let everyone notice.
    pub fn run_git_op(
        &mut self,
        host: anterm_ui_kit::host_ops::SharedHost,
        root: std::path::PathBuf,
        op: GitOp,
        then: Option<ScmFollowUp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = anterm_ui_kit::git::data::status_of(cx, host.id(), &root) else {
            return;
        };
        let head = status.head.clone();
        let id = host.id();

        let slot = if op.is_network() {
            match cx
                .default_global::<anterm_ui_kit::git::data::ScmData>()
                .take_network_slot(id, &root)
            {
                Some(slot) => Some(slot),
                None => {
                    gpui_kit::component::WindowExt::push_notification(
                        window,
                        t(L10nKey::ScmNetworkBusy).to_string(),
                        cx,
                    );
                    return;
                }
            }
        } else {
            None
        };

        let was_commit = matches!(op, GitOp::Commit { .. });
        if let GitOp::Commit { message, .. } = &op {
            self.amend = false;
            self.committing = Some((
                RepoKey {
                    host: id,
                    root: root.clone(),
                },
                head.clone(),
                message.clone(),
            ));
        }

        let op_root = root.clone();
        anterm_ui_kit::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let outcome = anterm_core::git::ops::run_op(h, &op_root, &op, &head);
                drop(slot);
                outcome
            },
            move |this, result, window, cx| {
                anterm_ui_kit::git::data::scm_invalidate(id, &root, cx);
                let ok = result.is_ok();
                if was_commit && !ok {
                    this.scm_commit_failed(id, &root);
                }
                if let Err(err) = result {
                    anterm_ui_kit::git::data::report_git_op_error(&err, window, cx);
                }
                cx.notify();
                if ok && let Some(follow) = then {
                    this.scm_follow_up(id, root, follow, window, cx);
                }
            },
        );
    }
}

/// What "discard all" actually runs, in order. Pure so a test can hold it up
/// against a status without a window.
fn discard_all_ops(status: &anterm_core::git::status::WorkingTreeStatus) -> Vec<GitOp> {
    let unstaged: Vec<_> = status
        .unstaged()
        .filter(|e| e.path.pathspec().is_some())
        .map(|e| e.path.clone())
        .collect();
    let untracked: Vec<_> = status
        .untracked()
        .filter(|e| e.path.pathspec().is_some())
        .map(|e| e.path.clone())
        .collect();
    let mut ops = Vec::new();
    if !unstaged.is_empty() {
        ops.push(GitOp::DiscardWorktree { paths: unstaged });
    }
    if !untracked.is_empty() {
        let directories = untracked.iter().any(|p| p.as_str().ends_with('/'));
        ops.push(GitOp::DiscardUntracked {
            paths: untracked,
            directories,
        });
    }
    ops
}

/// `origin/main` → `("origin", "main")`.
///
/// The first component is the remote: a branch name may contain slashes, a
/// remote name may not.
pub fn split_upstream(upstream: &str) -> Option<(&str, &str)> {
    let (remote, branch) = upstream.split_once('/')?;
    (!remote.is_empty() && !branch.is_empty()).then_some((remote, branch))
}

/// The question a destructive operation has to answer before it runs.
fn confirm_question(op: &GitOp, loss: Destructive) -> String {
    // Its own question, not the discard one: a hard reset to an older commit
    // drops commits off the branch, and a dialog that says "Discard every
    // change in this repository?" never mentions the part that hurts.
    if matches!(op, GitOp::Reset { .. }) {
        return t(L10nKey::ScmResetHardConfirm).to_string();
    }
    match loss {
        Destructive::RewritesHistory => t(L10nKey::ScmAmendConfirm).to_string(),
        // One file gets named; a whole group does not, because a list of two
        // hundred paths in a system dialog says less than the count does.
        _ => match op.paths() {
            [only] => t_fmt(L10nKey::ScmDiscardConfirm, &[("path", only.as_str())]),
            _ => t(L10nKey::ScmDiscardAllConfirm).to_string(),
        },
    }
}

fn confirm_verb(op: &GitOp, loss: Destructive) -> &'static str {
    if matches!(op, GitOp::Reset { .. }) {
        return t(L10nKey::ScmReset);
    }
    match loss {
        Destructive::RewritesHistory => t(L10nKey::ScmAmendLastCommit),
        _ => t(L10nKey::ScmDiscard),
    }
}

/// Cancel first, so Return lands on it and not on the loss, then the action.
///
/// Cancel is marked as one rather than passed as a bare label: gpui only
/// recognises a cancel by its label when that label is the English word, so
/// in any other language Escape had nothing to answer and the dialog could
/// not tell which button was the safe one.
fn guarded_answers(action: &str) -> [PromptButton; 2] {
    [
        PromptButton::cancel(t(L10nKey::Cancel)),
        PromptButton::new(action),
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    use anterm_core::git::status::{
        ChangeCode, EntryKind, HeadState, RepoPath, StatusEntry, WorkingTreeStatus,
    };

    fn entry(path: &str, index: ChangeCode, worktree: ChangeCode, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: RepoPath::from_bytes(path.as_bytes()),
            orig_path: None,
            index,
            worktree,
            kind,
            submodule: None,
            rename_score: None,
            conflict: None,
        }
    }

    fn status_with(entries: Vec<StatusEntry>) -> WorkingTreeStatus {
        WorkingTreeStatus {
            root: std::path::PathBuf::from("/repo"),
            home: std::path::PathBuf::from("/repo"),
            head: HeadState::Branch {
                name: "main".into(),
                oid: "0".repeat(40),
            },
            upstream: None,
            ahead_behind: None,
            total_entries: entries.len(),
            entries,
            truncated: false,
            stash_count: 0,
            operation: None,
            prefilled_message: None,
        }
    }

    /// `checkout --` restores from the *index*: a staged-only path either
    /// survives it (staged edit) or — a staged deletion, in neither index nor
    /// worktree — makes git reject the whole batch as an unmatched pathspec,
    /// taking every real discard down with it. Only unstaged paths go in.
    #[test]
    fn discard_all_sends_only_unstaged_paths_to_checkout() {
        let status = status_with(vec![
            // Staged edit, clean worktree: not `checkout --`'s business.
            entry(
                "staged.rs",
                ChangeCode::Modified,
                ChangeCode::None,
                EntryKind::Tracked,
            ),
            // Staged deletion: the pathspec that used to sink the batch.
            entry(
                "deleted.rs",
                ChangeCode::Deleted,
                ChangeCode::None,
                EntryKind::Tracked,
            ),
            // Staged and edited again: the worktree half is discardable.
            entry(
                "both.rs",
                ChangeCode::Modified,
                ChangeCode::Modified,
                EntryKind::Tracked,
            ),
            entry(
                "edited.rs",
                ChangeCode::None,
                ChangeCode::Modified,
                EntryKind::Tracked,
            ),
            entry(
                "new.rs",
                ChangeCode::None,
                ChangeCode::None,
                EntryKind::Untracked,
            ),
        ]);
        let ops = discard_all_ops(&status);
        assert_eq!(ops.len(), 2, "one checkout batch, one clean batch");
        match &ops[0] {
            GitOp::DiscardWorktree { paths } => {
                let names: Vec<_> = paths.iter().map(|p| p.as_str()).collect();
                assert_eq!(names, vec!["both.rs", "edited.rs"]);
            }
            other => panic!("expected DiscardWorktree first, got {:?}", other.label()),
        }
        match &ops[1] {
            GitOp::DiscardUntracked { paths, directories } => {
                let names: Vec<_> = paths.iter().map(|p| p.as_str()).collect();
                assert_eq!(names, vec!["new.rs"]);
                assert!(!directories);
            }
            other => panic!("expected DiscardUntracked second, got {:?}", other.label()),
        }

        // Nothing to discard means nothing to run — and no prompt to answer.
        assert!(discard_all_ops(&status_with(Vec::new())).is_empty());
    }

    #[test]
    fn an_upstream_splits_on_its_first_slash_only() {
        assert_eq!(split_upstream("origin/main"), Some(("origin", "main")));
        // Branch names carry slashes; remote names cannot.
        assert_eq!(
            split_upstream("origin/feature/auth-retry"),
            Some(("origin", "feature/auth-retry"))
        );
        assert_eq!(split_upstream("main"), None);
        assert_eq!(split_upstream("/main"), None);
        assert_eq!(split_upstream("origin/"), None);
    }
}
