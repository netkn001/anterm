//! Pure layout, commit-plan, row-verb, and painting helpers for the SCM panel.

use gpui::{AnyElement, SharedString, div, prelude::*, rems};
use gpui_kit::component::button::ButtonCustomVariant;
use gpui_kit::component::{ActiveTheme as _, IconName};

use anterm_core::git::ops::GitOp;
use anterm_core::git::status::{
    ChangeCode, DecoStatus, HeadState, RepoOperation, RepoPath, StatusEntry, WorkingTreeStatus,
};

use anterm_ui_kit::git::diff::DiffSource;
use anterm_i18n::{L10nKey, t};
use crate::right_panel::META_MONO;
use crate::scm::state::ScmGroup;
use crate::scm::status::status_glyph;

use super::UNTRACKED_AUTO_COLLAPSE;

/// Which sections an entry shows up in.
///
/// A file can be staged and unstaged at once (`XY == "MM"`), and then it
/// belongs in both — the same thing VS Code shows, and the honest reading of
/// what a commit right now would contain.
pub fn in_group(entry: &StatusEntry, group: ScmGroup) -> bool {
    match group {
        ScmGroup::Merge => entry.is_conflicted(),
        ScmGroup::Untracked => entry.is_untracked(),
        ScmGroup::Staged => entry.is_staged(),
        ScmGroup::Changes => entry.is_unstaged() && !entry.is_untracked(),
    }
}

/// The letter and colour a row wears, decided by the half of `XY` its group
/// is about — a file staged as added and then modified is `A` under Staged
/// and `M` under Changes, which is what `git status` itself says.
pub fn row_status(entry: &StatusEntry, group: ScmGroup) -> (&'static str, DecoStatus) {
    let deco = match group {
        ScmGroup::Merge => DecoStatus::Conflict,
        ScmGroup::Untracked => DecoStatus::Untracked,
        ScmGroup::Staged => code_deco(entry.index),
        ScmGroup::Changes => code_deco(entry.worktree),
    };
    let letter = match group {
        // The two groups whose letter is fixed use the shared glyph table, so
        // a conflict is `U` here and in the file tree alike.
        ScmGroup::Merge | ScmGroup::Untracked => status_glyph(deco),
        ScmGroup::Staged => letter_of(entry.index),
        ScmGroup::Changes => letter_of(entry.worktree),
    };
    (letter, deco)
}

/// `ChangeCode::letter` returns a `char`; rows want a `&'static str` so the
/// badge never allocates. `T` and `C` keep their own letters rather than being
/// folded into `M` and `R` — git shows them, and they mean different things.
fn letter_of(code: ChangeCode) -> &'static str {
    match code {
        ChangeCode::None => " ",
        ChangeCode::Modified => "M",
        ChangeCode::TypeChanged => "T",
        ChangeCode::Added => "A",
        ChangeCode::Deleted => "D",
        ChangeCode::Renamed => "R",
        ChangeCode::Copied => "C",
        ChangeCode::Unmerged => "U",
    }
}

fn code_deco(code: ChangeCode) -> DecoStatus {
    match code {
        ChangeCode::Deleted => DecoStatus::Deleted,
        ChangeCode::Added => DecoStatus::Added,
        ChangeCode::Renamed | ChangeCode::Copied => DecoStatus::Renamed,
        ChangeCode::Unmerged => DecoStatus::Conflict,
        _ => DecoStatus::Modified,
    }
}

/// Which patch a row's click opens.
///
/// Staged rows show `git diff --cached`; everything else shows the working
/// tree. Getting this wrong is not cosmetic — the file name would be right and
/// the hunks underneath it would be someone else's.
pub fn group_diff_source(group: ScmGroup) -> DiffSource {
    match group {
        ScmGroup::Staged => DiffSource::Staged,
        ScmGroup::Merge | ScmGroup::Changes | ScmGroup::Untracked => DiffSource::Worktree,
    }
}

pub(super) fn group_label(group: ScmGroup) -> L10nKey {
    match group {
        ScmGroup::Merge => L10nKey::ScmGroupMerge,
        ScmGroup::Staged => L10nKey::ScmGroupStaged,
        ScmGroup::Changes => L10nKey::ScmGroupChanges,
        ScmGroup::Untracked => L10nKey::ScmGroupUntracked,
    }
}

/// Whether a group nobody has touched starts folded.
pub fn starts_collapsed(group: ScmGroup, count: usize) -> bool {
    group == ScmGroup::Untracked && count > UNTRACKED_AUTO_COLLAPSE
}

/// What the branch row says where the branch name goes.
pub fn head_label(head: &HeadState) -> String {
    match head {
        // A detached HEAD has no name, so it wears its sha — shortened to the
        // seven characters git itself abbreviates to.
        HeadState::Detached { oid } => oid.chars().take(7).collect(),
        _ => head.label(),
    }
}

/// The `↑2 ↓0` reading, or nothing.
///
/// A branch that is level with its upstream says nothing at all: the quiet
/// state is the common one, and a token that is always there stops being read.
/// A branch with no upstream offers to publish instead — but only if there is
/// a branch to publish. A detached or unborn HEAD has no upstream *by
/// definition*, and the offer there is one the sync tile already refuses
/// (#545); it was the widest token on the row while naming the one thing that
/// could not happen.
///
/// Once there *is* something to say, both halves are said, zero included. The
/// pair is one reading of one distance, and dropping the empty half makes it
/// change shape as the numbers move — `↑2` becoming `↑2 ↓1` on a fetch is a
/// different-looking thing in the corner of the eye, where `↑2 ↓0` becoming
/// `↑2 ↓1` is the same thing with a digit changed.
pub fn tracking_chip(
    upstream: Option<&str>,
    ahead_behind: Option<(u32, u32)>,
    pushable: bool,
) -> Option<String> {
    if upstream.is_none() {
        return pushable.then(|| t(L10nKey::ScmPublishBranch).to_string());
    }
    match ahead_behind? {
        (0, 0) => None,
        (ahead, behind) => Some(format!("↑{ahead} ↓{behind}")),
    }
}

/// A quiet token on the branch row: the tracking distance, or the count of
/// other repositories.
///
/// Not `info_chip`. A chip's fill is what earns it the right to shout, and the
/// only two things on this row worth shouting are a state you could forget you
/// are in — detached, mid-rebase, armed to amend — and they already wear the
/// warning tint. `↑2 ↓0` is a reading, so it reads as text; the row's own 6px
/// gap is enough to keep it off the branch name without a box around it.
///
/// The same quiet mono step the panel's badges and chips are set on, minus the
/// fill, so a note and a chip still line up as one row of tokens.
/// The fill under the commit area's two shapes — the message box and the split
/// button's frame.
///
/// Half of the ramp's first rung, not the rung itself. `hover` is what a row
/// wears while the pointer is on it, which is a state that lasts a moment;
/// these two wear their fill permanently, and at full strength they were the
/// loudest thing on an idle panel. Both read from here so they cannot drift
/// apart: the commit area is one block, and two greys a shade off each other
/// look like a mistake rather than a hierarchy.
pub(super) fn field_fill(sf: anterm_ui_kit::presets::Surface) -> u32 {
    anterm_ui_kit::presets::mix(sf.base, sf.hover, 0.5)
}

/// One half of the split commit control: paints nothing, ever.
///
/// Every state is transparent so the frame around both halves is the only
/// thing that fills, and the control lights as one shape rather than one end.
/// Two of those states are worth naming, because a stock variant gets them
/// wrong here in opposite directions:
///
/// * `.ghost()` hovers to `sidebar_accent`, which sits close enough to the
///   frame's own fill that the half would light almost invisibly — the worst of
///   both, a highlight you can see is uneven but cannot read.
/// * gpui-component resolves a `Custom` variant's *selected* paint from its
///   `active` slot, and a dropdown marks its trigger selected for as long as
///   the menu is open. Anything but transparent there parks a block on the
///   chevron for the whole time the user is reading the menu.
pub(super) fn commit_half(ink: gpui::Hsla, cx: &gpui::App) -> ButtonCustomVariant {
    let clear = gpui::transparent_black();
    ButtonCustomVariant::new(cx)
        .color(clear)
        .foreground(ink)
        .hover(clear)
        .active(clear)
}

/// How the split commit control is painted.
#[derive(Clone, Copy)]
pub(super) struct CommitPaint {
    pub(super) fill: gpui::Hsla,
    pub(super) hover: gpui::Hsla,
    pub(super) ink: gpui::Hsla,
    pub(super) seam: gpui::Hsla,
}

/// Committable: an inverted neutral — the panel's ink as the fill and its own
/// surface as the label, so it is the one solid shape in a panel of rows, and
/// it stays neutral rather than borrowing the accent the sidebar and focus
/// rings already spend. Not committable: the message box's faint fill and the
/// panel's ink, a control that is there but is not asking for anything.
///
/// The label ink is the opaque surface rather than `theme.background`, which
/// carries the window's transparency when one is configured.
pub(super) fn commit_paint(live: bool, cx: &gpui::App) -> CommitPaint {
    let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
    let fg = cx.theme().foreground;
    match live {
        true => {
            let ink: gpui::Hsla = gpui::rgb(sf.base).into();
            CommitPaint {
                fill: fg,
                hover: fg.blend(ink.opacity(0.14)),
                ink,
                seam: ink.opacity(0.24),
            }
        }
        false => CommitPaint {
            fill: gpui::rgb(field_fill(sf)).into(),
            hover: gpui::rgb(sf.hover).into(),
            ink: fg,
            seam: fg.opacity(0.18),
        },
    }
}

pub(super) fn branch_note(text: &str, ink: gpui::Hsla, mono: &SharedString) -> AnyElement {
    div()
        .flex_none()
        .text_size(rems(META_MONO))
        .font_family(mono.clone())
        .text_color(ink)
        .child(text.to_string())
        .into_any_element()
}

/// Which sequencer operation is parked in the repository.
///
/// `RebaseInteractive` reads as "rebasing" on purpose: modern git writes
/// `rebase-merge/interactive` for every rebase, so the distinction the variant
/// name suggests is not one the repository on disk can actually make — and
/// `git status` does not draw it either.
pub fn operation_label(op: RepoOperation) -> L10nKey {
    match op {
        RepoOperation::Merge => L10nKey::ScmOpMerge,
        RepoOperation::Rebase | RepoOperation::RebaseInteractive => L10nKey::ScmOpRebase,
        RepoOperation::CherryPick => L10nKey::ScmOpCherryPick,
        RepoOperation::Revert => L10nKey::ScmOpRevert,
        RepoOperation::Bisect => L10nKey::ScmOpBisect,
        RepoOperation::Am => L10nKey::ScmOpAm,
    }
}

/// What the commit button says, and whether it can be pressed at all.
pub struct CommitPlan {
    pub label: L10nKey,
    pub enabled: bool,
    /// Why the button is disabled, when it is. "Nothing to commit" and "write
    /// a message" call for opposite actions, and one tooltip for both sends
    /// the user staging files they already staged.
    pub reason: L10nKey,
}

/// Decide both from the state of the index.
///
/// "Commit All" rather than a silent `-a`: with nothing staged, `git commit`
/// would commit nothing, and the honest thing is to say on the button that
/// every tracked change is about to go in. An armed amend needs no message —
/// `--no-edit` keeps the one that is already on HEAD.
pub fn commit_plan(status: &WorkingTreeStatus, amend: bool, message: &str) -> CommitPlan {
    let staged = status.staged().next().is_some();
    let tracked_edits = status.unstaged().next().is_some();
    let label = if amend {
        L10nKey::ScmCommitAmendButton
    } else if staged {
        L10nKey::ScmCommitButton
    } else {
        L10nKey::ScmCommitAllButton
    };
    let has_message = !message.trim().is_empty();
    // Mid-merge, an empty message is still committable: `ops` sends
    // `--allow-empty-message` for exactly this, because refusing would strand
    // the merge behind a message the user deliberately cleared.
    let merging = matches!(status.operation, Some(RepoOperation::Merge));
    let something = staged || tracked_edits || amend;
    CommitPlan {
        label,
        enabled: something && (has_message || amend || merging),
        reason: if something {
            L10nKey::ScmCommitNeedsMessage
        } else {
            L10nKey::ScmNothingToCommit
        },
    }
}

/// Whether a commit has to stage everything tracked first (`-a`).
pub fn commit_stages_everything(status: &WorkingTreeStatus, amend: bool) -> bool {
    // Amending with nothing staged means "fix the message", not "sweep the
    // working tree into the commit I already made".
    !amend && status.staged().next().is_none()
}

/// The branch a push would move, or why there is no push to make.
///
/// Both dead ends used to die in the same silent `let-else` in `scm_push`
/// (#545): a detached HEAD has no branch to push — and pushing a bare sha
/// needs a refspec the panel has no way to ask for — while an unborn branch
/// has a name but no commits to send yet. The key binding, the palette and
/// the follow-up half of "Commit and Push" all land in that guard, so the
/// reason is a key the caller can toast, not a log line.
pub fn pushable_branch(head: &HeadState) -> Result<&str, L10nKey> {
    match head {
        HeadState::Branch { name, .. } => Ok(name),
        HeadState::Detached { .. } => Err(L10nKey::ScmPushDetached),
        HeadState::Unborn { .. } => Err(L10nKey::ScmPushNoCommits),
    }
}

/// What a row's buttons do. Named rather than inlined because the same verb
/// appears on the row, on its group header and in its context menu, and the
/// three must not drift into meaning different things.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowVerb {
    Discard,
    Stage,
    Unstage,
    OpenConflict,
    MarkResolved,
}

/// Right to left, most-used last: the pointer travels to the right edge, so
/// the button under it should be the one nine hovers out of ten want.
pub fn row_verbs(group: ScmGroup) -> &'static [(RowVerb, IconName)] {
    match group {
        ScmGroup::Merge => &[
            (RowVerb::OpenConflict, IconName::Eye),
            (RowVerb::MarkResolved, IconName::Check),
        ],
        ScmGroup::Staged => &[(RowVerb::Unstage, IconName::Minus)],
        ScmGroup::Changes | ScmGroup::Untracked => &[
            (RowVerb::Discard, IconName::Undo2),
            (RowVerb::Stage, IconName::Plus),
        ],
    }
}

/// The header's buttons are the row's, minus the ones that only make sense
/// for one file: there is no group-wide "open the conflict".
pub fn group_verbs(group: ScmGroup) -> &'static [(RowVerb, IconName)] {
    match group {
        ScmGroup::Merge => &[(RowVerb::MarkResolved, IconName::Check)],
        _ => row_verbs(group),
    }
}

/// The operation a verb runs over one or many paths. `None` for the verbs
/// that change no state.
pub fn verb_op(verb: RowVerb, group: ScmGroup, paths: Vec<RepoPath>) -> Option<GitOp> {
    if paths.is_empty() {
        return None;
    }
    Some(match verb {
        // Resolving a conflict is `git add`, exactly as it is on the command
        // line — there is no separate "resolve" verb in git.
        RowVerb::Stage | RowVerb::MarkResolved => GitOp::Stage { paths },
        RowVerb::Unstage => GitOp::Unstage { paths },
        RowVerb::Discard if group == ScmGroup::Untracked => {
            let directories = paths.iter().any(|p| p.as_str().ends_with('/'));
            GitOp::DiscardUntracked { paths, directories }
        }
        RowVerb::Discard => GitOp::DiscardWorktree { paths },
        RowVerb::OpenConflict => return None,
    })
}

/// The paths in a group git can actually be told about.
///
/// A path that is not valid UTF-8 cannot be sent as a pathspec at all, and
/// `GitOp::validate` rejects the whole operation over one of them — so a group
/// action has to leave them out rather than fail for everyone. Their own rows
/// are greyed out and say why.
pub fn writable_paths(entries: &[&StatusEntry]) -> Vec<RepoPath> {
    entries
        .iter()
        .filter(|e| e.path.pathspec().is_some())
        .map(|e| e.path.clone())
        .collect()
}

pub(super) fn verb_tooltip(verb: RowVerb) -> &'static str {
    t(match verb {
        RowVerb::Discard => L10nKey::ScmDiscard,
        RowVerb::Stage => L10nKey::ScmStage,
        RowVerb::Unstage => L10nKey::ScmUnstage,
        RowVerb::OpenConflict => L10nKey::ScmOpenConflict,
        RowVerb::MarkResolved => L10nKey::ScmMarkResolved,
    })
}

pub(super) fn verb_all_tooltip(verb: RowVerb) -> &'static str {
    t(match verb {
        RowVerb::Discard => L10nKey::ScmDiscardAll,
        RowVerb::Stage => L10nKey::ScmStageAll,
        RowVerb::Unstage => L10nKey::ScmUnstageAll,
        RowVerb::OpenConflict => L10nKey::ScmOpenConflict,
        RowVerb::MarkResolved => L10nKey::ScmMarkResolved,
    })
}
