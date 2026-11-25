use super::*;
use anterm_ui_kit::config::{CoreConfig, DiffViewMode, RightPanelTab};
use anterm_ui_kit::git::diff::DiffSource;
use anterm_core::git::status::{
    ChangeCode, ConflictKind, EntryKind, RepoOperation, RepoPath,
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
        conflict: (kind == EntryKind::Unmerged).then_some(ConflictKind::BothModified),
    }
}

fn groups_of(entry: &StatusEntry) -> Vec<ScmGroup> {
    ScmGroup::ORDER
        .into_iter()
        .filter(|g| in_group(entry, *g))
        .collect()
}

#[test]
fn a_file_staged_and_edited_again_lands_in_both_groups() {
    let e = entry(
        "a.rs",
        ChangeCode::Modified,
        ChangeCode::Modified,
        EntryKind::Tracked,
    );
    assert_eq!(groups_of(&e), vec![ScmGroup::Staged, ScmGroup::Changes]);
}

#[test]
fn each_other_kind_of_entry_lands_in_exactly_one_group() {
    let staged = entry(
        "a.rs",
        ChangeCode::Added,
        ChangeCode::None,
        EntryKind::Tracked,
    );
    assert_eq!(groups_of(&staged), vec![ScmGroup::Staged]);

    let unstaged = entry(
        "b.rs",
        ChangeCode::None,
        ChangeCode::Modified,
        EntryKind::Tracked,
    );
    assert_eq!(groups_of(&unstaged), vec![ScmGroup::Changes]);

    let untracked = entry(
        "c.rs",
        ChangeCode::None,
        ChangeCode::None,
        EntryKind::Untracked,
    );
    assert_eq!(groups_of(&untracked), vec![ScmGroup::Untracked]);

    let conflict = entry(
        "d.rs",
        ChangeCode::Unmerged,
        ChangeCode::Unmerged,
        EntryKind::Unmerged,
    );
    assert_eq!(groups_of(&conflict), vec![ScmGroup::Merge]);
}

#[test]
fn a_row_wears_the_letter_of_the_half_its_group_is_about() {
    let e = entry(
        "a.rs",
        ChangeCode::Added,
        ChangeCode::Modified,
        EntryKind::Tracked,
    );
    assert_eq!(row_status(&e, ScmGroup::Staged), ("A", DecoStatus::Added));
    assert_eq!(
        row_status(&e, ScmGroup::Changes),
        ("M", DecoStatus::Modified)
    );

    let untracked = entry(
        "c.rs",
        ChangeCode::None,
        ChangeCode::None,
        EntryKind::Untracked,
    );
    assert_eq!(
        row_status(&untracked, ScmGroup::Untracked),
        ("?", DecoStatus::Untracked)
    );
    let conflict = entry(
        "d.rs",
        ChangeCode::Unmerged,
        ChangeCode::Unmerged,
        EntryKind::Unmerged,
    );
    assert_eq!(
        row_status(&conflict, ScmGroup::Merge),
        ("U", DecoStatus::Conflict)
    );
}

#[test]
fn staged_rows_open_the_cached_diff_and_the_rest_the_working_tree() {
    assert_eq!(group_diff_source(ScmGroup::Staged), DiffSource::Staged);
    for group in [ScmGroup::Merge, ScmGroup::Changes, ScmGroup::Untracked] {
        assert_eq!(group_diff_source(group), DiffSource::Worktree);
    }
}

#[test]
fn only_a_long_untracked_list_starts_folded() {
    assert!(!starts_collapsed(
        ScmGroup::Untracked,
        UNTRACKED_AUTO_COLLAPSE
    ));
    assert!(starts_collapsed(
        ScmGroup::Untracked,
        UNTRACKED_AUTO_COLLAPSE + 1
    ));
    for group in [ScmGroup::Merge, ScmGroup::Staged, ScmGroup::Changes] {
        assert!(!starts_collapsed(group, 1_000));
    }
}

#[test]
fn every_button_a_row_offers_maps_to_the_verb_it_is_named_for() {
    let file = vec![RepoPath::from_bytes(b"a.rs")];
    assert!(matches!(
        verb_op(RowVerb::Stage, ScmGroup::Changes, file.clone()),
        Some(GitOp::Stage { .. })
    ));
    assert!(matches!(
        verb_op(RowVerb::Unstage, ScmGroup::Staged, file.clone()),
        Some(GitOp::Unstage { .. })
    ));
    assert!(matches!(
        verb_op(RowVerb::MarkResolved, ScmGroup::Merge, file.clone()),
        Some(GitOp::Stage { .. })
    ));
    assert!(matches!(
        verb_op(RowVerb::Discard, ScmGroup::Changes, file.clone()),
        Some(GitOp::DiscardWorktree { .. })
    ));
    assert!(matches!(
        verb_op(RowVerb::Discard, ScmGroup::Untracked, file.clone()),
        Some(GitOp::DiscardUntracked {
            directories: false,
            ..
        })
    ));
    assert!(matches!(
        verb_op(
            RowVerb::Discard,
            ScmGroup::Untracked,
            vec![RepoPath::from_bytes(b"vendor/")]
        ),
        Some(GitOp::DiscardUntracked {
            directories: true,
            ..
        })
    ));
    assert!(verb_op(RowVerb::OpenConflict, ScmGroup::Merge, file).is_none());
    assert!(verb_op(RowVerb::Stage, ScmGroup::Changes, Vec::new()).is_none());
}

#[test]
fn everything_that_can_lose_work_says_so_before_it_runs() {
    let paths = vec![RepoPath::from_bytes(b"a.rs")];
    for group in [ScmGroup::Changes, ScmGroup::Untracked] {
        let op = verb_op(RowVerb::Discard, group, paths.clone()).expect("discard has an op");
        assert!(op.destructive().is_some(), "{group:?} discard");
    }
    for verb in [RowVerb::Stage, RowVerb::Unstage, RowVerb::MarkResolved] {
        let op = verb_op(verb, ScmGroup::Changes, paths.clone()).expect("verb has an op");
        assert!(op.destructive().is_none(), "{verb:?}");
    }
}

#[test]
fn a_group_action_leaves_out_the_paths_git_cannot_be_told_about() {
    let good = entry(
        "a.rs",
        ChangeCode::None,
        ChangeCode::Modified,
        EntryKind::Tracked,
    );
    let mut bad = good.clone();
    bad.path = RepoPath::from_bytes(&[0xff, 0xfe, b'.', b'r', b's']);
    assert!(bad.path.pathspec().is_none(), "the fixture must be lossy");

    let paths = writable_paths(&[&good, &bad]);
    assert_eq!(paths.len(), 1, "the lossy path is dropped, not carried");
    let op = verb_op(RowVerb::Stage, ScmGroup::Changes, paths).expect("still has work to do");
    assert!(op.validate().is_ok());

    let all_bad = verb_op(RowVerb::Stage, ScmGroup::Changes, writable_paths(&[&bad]));
    assert!(all_bad.is_none(), "nothing to do means no operation at all");
}

#[test]
fn a_group_header_offers_no_button_that_only_makes_sense_for_one_file() {
    let verbs: Vec<RowVerb> = group_verbs(ScmGroup::Merge)
        .iter()
        .map(|(v, _)| *v)
        .collect();
    assert_eq!(verbs, vec![RowVerb::MarkResolved]);
    for group in [ScmGroup::Staged, ScmGroup::Changes, ScmGroup::Untracked] {
        for (verb, _) in group_verbs(group) {
            assert_ne!(*verb, RowVerb::OpenConflict);
        }
    }
}

#[test]
fn the_panels_type_is_named_rather_than_numbered() {
    let needle = concat!("text_size", "(px(");
    for (file, src) in [
        ("panel/mod.rs", include_str!("mod.rs")),
        ("panel/commit.rs", include_str!("commit.rs")),
        ("graph/mod.rs", include_str!("../graph/mod.rs")),
        ("graph/paint.rs", include_str!("../graph/paint.rs")),
        ("detail.rs", include_str!("../detail.rs")),
    ] {
        let offenders: Vec<&str> = src
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .filter(|line| line.contains(needle))
            .collect();
        assert!(
            offenders.is_empty(),
            "{file} sets type in pixels, which is how the panel fell a step \
             behind the rest of the window last time: {offenders:?}"
        );
    }
}

#[test]
fn the_group_chevron_stands_in_the_status_letters_column() {
    assert_eq!(BADGE_W, crate::right_panel::BADGE_W);
    assert!(GROUP_CHEVRON <= BADGE_W && BADGE_W - GROUP_CHEVRON <= 2.);
}

fn repo(root: &str) -> RepoKey {
    RepoKey {
        host: HostId::LOCAL,
        root: PathBuf::from(root),
    }
}

fn status_of_repo(root: &str, entries: Vec<StatusEntry>) -> WorkingTreeStatus {
    WorkingTreeStatus {
        root: PathBuf::from(root),
        home: PathBuf::from(root),
        head: HeadState::Branch {
            name: "main".into(),
            oid: "1111111".into(),
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

#[test]
fn the_commit_button_says_what_pressing_it_would_actually_do() {
    let staged = status_of_repo(
        "/a",
        vec![entry(
            "a.rs",
            ChangeCode::Modified,
            ChangeCode::None,
            EntryKind::Tracked,
        )],
    );
    let unstaged = status_of_repo(
        "/a",
        vec![entry(
            "a.rs",
            ChangeCode::None,
            ChangeCode::Modified,
            EntryKind::Tracked,
        )],
    );
    let clean = status_of_repo("/a", Vec::new());

    assert_eq!(
        commit_plan(&staged, false, "msg").label,
        L10nKey::ScmCommitButton
    );
    assert_eq!(
        commit_plan(&unstaged, false, "msg").label,
        L10nKey::ScmCommitAllButton
    );
    assert_eq!(
        commit_plan(&staged, true, "").label,
        L10nKey::ScmCommitAmendButton
    );

    assert!(commit_plan(&staged, false, "msg").enabled);
    assert!(
        !commit_plan(&staged, false, "   ").enabled,
        "an all-whitespace message is no message"
    );
    assert_eq!(
        commit_plan(&staged, false, "  ").reason,
        L10nKey::ScmCommitNeedsMessage
    );
    assert_eq!(
        commit_plan(&clean, false, "msg").reason,
        L10nKey::ScmNothingToCommit
    );
    assert!(
        commit_plan(&clean, true, "").enabled,
        "amending with no message keeps HEAD's own with --no-edit"
    );
    assert!(!commit_plan(&clean, false, "msg").enabled);
}

#[test]
fn a_head_without_a_pushable_branch_says_why() {
    let branch = HeadState::Branch {
        name: "main".into(),
        oid: "1111111".into(),
    };
    assert_eq!(pushable_branch(&branch), Ok("main"));
    assert_eq!(
        pushable_branch(&HeadState::Detached {
            oid: "1111111".into()
        }),
        Err(L10nKey::ScmPushDetached)
    );
    assert_eq!(
        pushable_branch(&HeadState::Unborn {
            branch: "main".into()
        }),
        Err(L10nKey::ScmPushNoCommits)
    );
}

#[test]
fn only_a_commit_with_an_empty_index_sweeps_the_working_tree() {
    let staged = status_of_repo(
        "/a",
        vec![entry(
            "a.rs",
            ChangeCode::Modified,
            ChangeCode::None,
            EntryKind::Tracked,
        )],
    );
    let unstaged = status_of_repo(
        "/a",
        vec![entry(
            "a.rs",
            ChangeCode::None,
            ChangeCode::Modified,
            EntryKind::Tracked,
        )],
    );
    assert!(commit_stages_everything(&unstaged, false));
    assert!(!commit_stages_everything(&staged, false));
    assert!(!commit_stages_everything(&unstaged, true));
}

#[test]
fn a_branch_level_with_its_upstream_says_nothing() {
    assert_eq!(tracking_chip(Some("origin/main"), Some((0, 0)), true), None);
    assert_eq!(
        tracking_chip(Some("origin/main"), Some((2, 0)), true).as_deref(),
        Some("↑2 ↓0")
    );
    assert_eq!(
        tracking_chip(Some("origin/main"), Some((0, 1)), true).as_deref(),
        Some("↑0 ↓1")
    );
    assert_eq!(
        tracking_chip(Some("origin/main"), Some((2, 1)), true).as_deref(),
        Some("↑2 ↓1")
    );
    assert_eq!(
        tracking_chip(None, None, true).as_deref(),
        Some(t(L10nKey::ScmPublishBranch))
    );
    assert_eq!(tracking_chip(None, None, false), None);
    assert_eq!(tracking_chip(Some("origin/main"), None, true), None);
}

#[test]
fn a_detached_head_shows_the_sha_git_would_print() {
    assert_eq!(
        head_label(&HeadState::Detached {
            oid: "0123456789abcdef".into()
        }),
        "0123456"
    );
    assert_eq!(
        head_label(&HeadState::Branch {
            name: "main".into(),
            oid: "0123456".into()
        }),
        "main"
    );
}

#[test]
fn every_parked_operation_has_something_to_say() {
    assert_eq!(
        operation_label(RepoOperation::RebaseInteractive),
        operation_label(RepoOperation::Rebase)
    );
    for op in [
        RepoOperation::Merge,
        RepoOperation::Rebase,
        RepoOperation::CherryPick,
        RepoOperation::Revert,
        RepoOperation::Bisect,
        RepoOperation::Am,
    ] {
        assert!(!t(operation_label(op)).is_empty(), "{op:?}");
    }
}

fn tab_from(json: &str) -> RightPanelTab {
    serde_json::from_str::<CoreConfig>(json)
        .expect("config deserializes")
        .right_panel_tab
}

#[test]
fn scm_tab_and_config_fields_round_trip() {
    let cfg = CoreConfig::default();
    assert_eq!(cfg.diff_view, DiffViewMode::Split);
    assert!(!cfg.scm_graph_expanded);

    let cfg = CoreConfig {
        right_panel_tab: RightPanelTab::Scm,
        ..Default::default()
    };
    let json = serde_json::to_value(&cfg).expect("config serializes");
    assert_eq!(json["right_panel_tab"], serde_json::json!("changes"));
    assert_eq!(
        tab_from(r#"{"right_panel_tab":"changes"}"#),
        RightPanelTab::Scm
    );
    assert_eq!(tab_from(r#"{"right_panel_tab":"scm"}"#), RightPanelTab::Scm);
    assert_eq!(tab_from(r#"{"right_panel_tab":"git"}"#), RightPanelTab::Scm);
    assert_eq!(
        tab_from(r#"{"right_panel_tab":"nonsense"}"#),
        RightPanelTab::Info
    );
}

/// Standalone `ScmPanelView` tests without booting `AntermApp`.
mod standalone_view_gpui_tests {
    use super::*;
    use crate::scm::state::ScmContext;
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
    use gpui_kit::component::Root;

    struct ScmHarness {
        view: Entity<ScmPanelView>,
    }

    impl gpui::Render for ScmHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            v_flex().size_full().child(self.view.clone())
        }
    }

    fn mount_scm(cx: &mut TestAppContext) -> (Entity<ScmPanelView>, VisualTestContext) {
        cx.executor().allow_parking();
        anterm_ui_kit::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(anterm_ui_kit::config::Config::default());
            anterm_ui_kit::theme::apply_theme(None, cx);
            anterm_ui_kit::keymap::init(cx);
        });
        let view = cx.new(|_| ScmPanelView::new(false));
        let view_for_root = view.clone();
        let window = cx.open_window(size(px(320.), px(520.)), move |window, cx| {
            let harness = cx.new(|_| ScmHarness {
                view: view_for_root,
            });
            Root::new(harness, window, cx)
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        (view, vcx)
    }

    #[gpui::test]
    fn commit_action_is_scoped_to_the_message_box(cx: &mut TestAppContext) {
        let (_view, mut vcx) = mount_scm(cx);
        let (in_box, outside, window_wide) = vcx.update(|window, _cx| {
            let scoped = gpui::KeyContext::parse(COMMIT_KEY_CONTEXT)
                .expect("the context the panel installs parses");
            (
                window.bindings_for_action_in_context(&anterm_ui_kit::actions::ScmCommit, scoped),
                window.bindings_for_action_in_context(
                    &anterm_ui_kit::actions::ScmCommit,
                    gpui::KeyContext::new_with_defaults(anterm_ui_kit::platform::current_os()),
                ),
                window.bindings_for_action_in_context(
                    &anterm_ui_kit::actions::ToggleFullscreen,
                    gpui::KeyContext::new_with_defaults(anterm_ui_kit::platform::current_os()),
                ),
            )
        });

        assert!(!in_box.is_empty());
        assert!(outside.is_empty());
        assert!(!window_wide.is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(in_box[0].keystrokes(), window_wide[0].keystrokes());
        }
    }

    #[gpui::test]
    fn commit_draft_follows_repository_and_merge_prefills(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_scm(cx);
        let (a, b) = (repo("/a"), repo("/b"));
        let status_a = status_of_repo("/a", Vec::new());
        let mut status_b = status_of_repo("/b", Vec::new());
        status_b.prefilled_message = Some("Merge branch 'topic'".into());

        view.update_in(&mut vcx, |view, window, cx| {
            let input = view.scm_commit_input(&a, &status_a, window, cx);
            input.update(cx, |state, cx| state.set_value("wip: a", window, cx));

            let input = view.scm_commit_input(&b, &status_b, window, cx);
            assert_eq!(input.read(cx).value(), "Merge branch 'topic'");
            input.update(cx, |state, cx| state.set_value("wip: b", window, cx));

            let input = view.scm_commit_input(&a, &status_a, window, cx);
            assert_eq!(input.read(cx).value(), "wip: a");
            assert_eq!(view.draft(&b), "wip: b");
        });
    }

    #[gpui::test]
    fn rejected_commit_keeps_message_until_head_moves(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_scm(cx);
        let key = repo("/a");
        let before = status_of_repo("/a", Vec::new());

        view.update(&mut vcx, |view, _cx| {
            view.committing = Some((key.clone(), before.head.clone(), "wip".into()));
            assert!(!view.scm_commit_landed(&key, &before, "wip"));

            let mut after = before.clone();
            after.head = HeadState::Branch {
                name: "main".into(),
                oid: "2222222".into(),
            };
            assert!(view.scm_commit_landed(&key, &after, "wip"));
            assert_eq!(view.draft(&key), "");
        });
    }

    #[gpui::test]
    fn folded_group_stays_folded_and_graph_toggle_persists(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_scm(cx);

        view.update(&mut vcx, |view, cx| {
            view.toggle_group(ScmGroup::Staged, 3, cx);
            view.toggle_group(ScmGroup::Untracked, 500, cx);
            view.toggle_graph(cx);
        });
        vcx.run_until_parked();

        view.read_with(&vcx, |view, cx| {
            assert!(view.group_collapsed(ScmGroup::Staged, 3));
            assert!(!view.group_collapsed(ScmGroup::Untracked, 500));
            assert!(view.graph.expanded);
            assert!(cx.global::<anterm_ui_kit::config::Config>().scm_graph_expanded);
        });
    }

    #[gpui::test]
    fn large_working_tree_is_virtualized_with_three_thousand_entries(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_scm(cx);
        let root = PathBuf::from("/virtual-repo");
        let host = anterm_core::host::local::LocalHost::new();

        let entries: Vec<StatusEntry> = (0..3_000)
            .map(|i| {
                entry(
                    &format!("src/mod_{:02}/file_{i:04}.rs", i / 100),
                    ChangeCode::None,
                    ChangeCode::Modified,
                    EntryKind::Tracked,
                )
            })
            .collect();
        let status = Arc::new(status_of_repo("/virtual-repo", entries));

        vcx.update(|_, cx| {
            cx.default_global::<anterm_ui_kit::git::data::ScmData>()
                .seed_status_for_test(HostId::LOCAL, root.clone(), status);
        });

        row_probe::reset();
        view.update(&mut vcx, |view, cx| {
            let key = RepoKey {
                host: HostId::LOCAL,
                root: root.clone(),
            };
            view.branches.insert(key, (0, vec!["main".into()]));
            view.roots
                .insert((HostId::LOCAL, root.clone()), (Instant::now(), Some(root.clone())));
            view.sync_context(ScmContext {
                active_tab: 0,
                pane_target: Some((host, root.clone())),
                diff_focus: None,
                panel_width: 320.,
            });
            cx.notify();
        });
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        let rendered = row_probe::last();
        assert!(
            rendered > 0 && rendered < 80,
            "expected virtualized SCM list to mount only visible rows for 3,000 entries, got {rendered}"
        );
        view.read_with(&vcx, |view, _| {
            assert_eq!(view.rendered_items.len(), 3_001);
        });
    }

    #[gpui::test]
    fn tree_view_and_filter_narrow_seeded_rows(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount_scm(cx);
        let root = PathBuf::from("/tree-repo");
        let host = anterm_core::host::local::LocalHost::new();
        let status = Arc::new(status_of_repo(
            "/tree-repo",
            vec![
                entry("src/ui/a.rs", ChangeCode::None, ChangeCode::None, EntryKind::Untracked),
                entry("src/ui/b.rs", ChangeCode::None, ChangeCode::None, EntryKind::Untracked),
                entry("docs/readme.md", ChangeCode::None, ChangeCode::None, EntryKind::Untracked),
            ],
        ));
        vcx.update(|_, cx| {
            cx.default_global::<anterm_ui_kit::git::data::ScmData>()
                .seed_status_for_test(HostId::LOCAL, root.clone(), status);
        });
        view.update(&mut vcx, |view, cx| {
            let key = RepoKey {
                host: HostId::LOCAL,
                root: root.clone(),
            };
            view.branches.insert(key, (0, vec!["main".into()]));
            view.roots
                .insert((HostId::LOCAL, root.clone()), (Instant::now(), Some(root.clone())));
            view.sync_context(ScmContext {
                active_tab: 0,
                pane_target: Some((host, root.clone())),
                diff_focus: None,
                panel_width: 320.,
            });
            cx.notify();
        });
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        let drawn = |vcx: &mut VisualTestContext, what: &'static str| {
            view.update_in(vcx, |_, _, cx| cx.notify());
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            vcx.debug_bounds(what).is_some()
        };
        let a = "scm-row-Untracked-src/ui/a.rs";
        let readme = "scm-row-Untracked-docs/readme.md";
        let src_ui = "scm-dir-Untracked-src/ui";

        assert!(drawn(&mut vcx, a));
        assert!(!drawn(&mut vcx, src_ui));

        view.update(&mut vcx, |view, cx| view.toggle_changes_tree(cx));
        assert!(drawn(&mut vcx, src_ui));
        assert!(drawn(&mut vcx, "scm-dir-Untracked-docs"));
        assert!(drawn(&mut vcx, a));

        view.update(&mut vcx, |view, cx| {
            view.toggle_dir(ScmGroup::Untracked, "src/ui".into());
            cx.notify();
        });
        assert!(drawn(&mut vcx, src_ui));
        assert!(!drawn(&mut vcx, a));
        assert!(drawn(&mut vcx, readme));

        view.update_in(&mut vcx, |view, window, cx| {
            let input = view.filter.clone().expect("filter input initialized");
            input.update(cx, |state, cx| state.set_value("UI A", window, cx));
        });
        assert!(drawn(&mut vcx, a));
        assert!(!drawn(&mut vcx, "scm-row-Untracked-src/ui/b.rs"));
        assert!(!drawn(&mut vcx, readme));
    }
}

/// The action strip is the one place in the panel where a hover reveal sits
/// under the pointer it is reacting to, so it is the one place where "is the
/// row hovered" and "can the row's hitbox be seen from here" can disagree.
mod action_strip_gpui_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
        PlatformInput, Point, Render, SharedString, TestAppContext, VisualTestContext, Window, div,
        point, prelude::*, px,
    };

    use super::super::ROW_H;
    use crate::right_panel::action_strip;
    use anterm_ui_kit::window_chrome::TILE_SIZE_XS;

    #[derive(Default)]
    struct Row {
        row_clicks: Rc<Cell<usize>>,
    }

    impl Render for Row {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let id = SharedString::from("row");
            let clicks = self.row_clicks.clone();
            div().pt(px(50.)).pl(px(50.)).child(
                div()
                    .id(id.clone())
                    .group(id.clone())
                    .relative()
                    .w(px(300.))
                    .h(px(ROW_H))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                    .child(
                        action_strip(&id, 0x00EEEEEE).child(
                            div()
                                .w(px(TILE_SIZE_XS))
                                .h(px(TILE_SIZE_XS))
                                .debug_selector(|| "scm-action".into()),
                        ),
                    ),
            )
        }
    }

    fn hover(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }),
                cx,
            );
            window.refresh();
        });
        vcx.run_until_parked();
    }

    fn click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        hover(vcx, at);
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                }),
                cx,
            );
            window.refresh();
        });
        vcx.run_until_parked();
    }

    #[gpui::test]
    fn the_strip_survives_the_pointer_reaching_it(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Row::default());
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("scm-action").is_none(),
            "an unhovered row draws no buttons"
        );

        hover(&mut vcx, point(px(70.), px(62.)));
        let strip = vcx
            .debug_bounds("scm-action")
            .expect("hovering the row reveals the buttons");

        hover(&mut vcx, strip.center());
        assert!(
            vcx.debug_bounds("scm-action").is_some(),
            "the buttons are still drawn with the pointer on top of them"
        );
    }

    #[gpui::test]
    fn a_click_on_the_strip_never_reaches_the_row(cx: &mut TestAppContext) {
        let row = Row::default();
        let clicks = row.row_clicks.clone();
        let window = cx.add_window(|_, _| row);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();

        click(&mut vcx, point(px(70.), px(62.)));
        assert_eq!(clicks.get(), 1, "clicking the row opens the diff");

        let strip = vcx
            .debug_bounds("scm-action")
            .expect("the row is hovered, so the strip is up");
        click(&mut vcx, strip.center());
        assert_eq!(
            clicks.get(),
            1,
            "but a click on the strip belongs to the strip"
        );
    }
}
