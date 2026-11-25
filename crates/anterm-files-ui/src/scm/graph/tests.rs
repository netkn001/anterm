use super::paint::{
    GRAPH_DOT_R, GRAPH_GUTTER_SHARE, GRAPH_LANE_W, GRAPH_LINE_W, GRAPH_MAX_LANES, GRAPH_MIN_LANES,
    band_of, column_ink, lane_center_x, project,
};
use super::*;
use anterm_ui_kit::presets::{LANE_SLOTS, Lanes};
use anterm_ui_kit::host_ops::HostId;
use anterm_core::git::log::{Edge, GraphRow, Lane};
use smallvec::smallvec;
use std::path::PathBuf;

fn repo() -> RepoKey {
    RepoKey {
        host: HostId::LOCAL,
        root: PathBuf::from("/tmp/anterm-graph-test"),
    }
}

fn lanes() -> Lanes {
    Lanes {
        ink: [0x111111, 0x222222, 0x333333, 0x444444, 0x555555, 0x666666],
        overflow: 0x999999,
    }
}

#[test]
fn lanes_inside_the_cap_keep_their_own_column() {
    for cap in GRAPH_MIN_LANES..=GRAPH_MAX_LANES {
        let columns: Vec<Lane> = (0..cap as Lane).map(|l| project(l, cap)).collect();
        let mut sorted = columns.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            columns.len(),
            sorted.len(),
            "cap {cap} collapsed a real lane"
        );
        assert_eq!(columns, (0..cap as Lane).collect::<Vec<_>>());
    }
}

#[test]
fn everything_past_the_cap_lands_in_the_overflow_column() {
    let cap = 4;
    for lane in 4u16..=anterm_core::git::log::MAX_LANES {
        assert_eq!(project(lane, cap), 3, "lane {lane} escaped the last column");
    }
    for lane in 0u16..8 {
        assert_eq!(project(lane, 1), 0);
    }
}

#[test]
fn lane_centres_rise_and_land_on_device_pixels() {
    for scale in [1.0f32, 1.25, 2.0, 3.0] {
        let mut previous = f32::MIN;
        for column in 0..GRAPH_MAX_LANES as Lane {
            let x = lane_center_x(column, scale);
            assert!(x > previous, "column {column} did not advance at {scale}x");
            previous = x;
            let physical = x * scale;
            assert!(
                (physical - physical.round()).abs() < 1e-4,
                "column {column} at {scale}x sits at {physical} device pixels"
            );
        }
    }
    assert_eq!(lane_center_x(0, 0.), lane_center_x(0, f32::NAN));
}

#[test]
fn the_gutter_narrows_with_the_panel() {
    assert_eq!(max_lanes(280.), 6);
    assert_eq!(max_lanes(260.), 5);
    assert_eq!(max_lanes(216.), 4);
    assert_eq!(max_lanes(320.), 6);
    assert_eq!(max_lanes(160.), GRAPH_MIN_LANES);
    for w in [120., 160., 216., 260., 320., 600.] {
        let cap = max_lanes(w);
        assert!(
            cap == GRAPH_MIN_LANES || gutter_width(cap) <= w * GRAPH_GUTTER_SHARE,
            "{w}px: a {cap}-lane gutter is {}px of a {}px budget",
            gutter_width(cap),
            w * GRAPH_GUTTER_SHARE
        );
    }
    assert_eq!(max_lanes(40.), GRAPH_MIN_LANES);
    assert_eq!(max_lanes(4000.), GRAPH_MAX_LANES);
    assert!(max_lanes(4000.) <= LANE_SLOTS);
}

#[test]
fn the_section_is_sized_in_commits() {
    let rows = |h: f32| (h - GRAPH_PAD_TOP - GRAPH_HEADER_H - GRAPH_PAD_BOTTOM) / GRAPH_ROW_H;
    let resting = rows(GRAPH_H_DEFAULT);
    assert!(
        (9.5..10.0).contains(&resting),
        "the resting section shows {resting} commits"
    );
    let shown = GRAPH_ROW_H * resting.fract();
    let cut = GRAPH_ROW_H - shown;
    assert!(
        shown >= 3. && cut >= 3.,
        "{resting} rows shows {shown}px of the last band and cuts {cut}px, \
         which is not a partial row a reader can see"
    );
    let floor = rows(GRAPH_H_MIN);
    assert!(
        floor >= 3.,
        "dragged all the way shut the section shows {floor} commits, which \
         is not enough of a graph to be one"
    );
    assert!(GRAPH_H_MIN < GRAPH_H_DEFAULT);
    assert_eq!((100f32 * GRAPH_H_MAX_RATIO).max(GRAPH_H_MIN), GRAPH_H_MIN);
}

#[test]
fn nodes_fit_inside_their_row_and_column() {
    let widest = GRAPH_DOT_R + 1.;
    assert!(widest * 2. <= GRAPH_ROW_H);
    assert!(lane_center_x(0, 1.) - widest >= 0.);
    assert!(widest + GRAPH_LINE_W / 2. < GRAPH_LANE_W);
    assert!(GRAPH_DOT_R - GRAPH_LINE_W >= 1.);
    assert!(GRAPH_DOT_R * 2. >= GRAPH_ROW_H * 0.25);
}

#[test]
fn conventional_prefixes_come_off_and_prose_does_not() {
    assert_eq!(
        split_conventional("feat(terminal): localize the menu"),
        (Some(("feat", false)), "localize the menu")
    );
    assert_eq!(
        split_conventional("fix: a thing"),
        (Some(("fix", false)), "a thing")
    );
    assert_eq!(
        split_conventional("feat!: drop the old dialect"),
        (Some(("feat", true)), "drop the old dialect")
    );
    assert_eq!(
        split_conventional("refactor(ui/scm)!: one entry point"),
        (Some(("refactor", true)), "one entry point")
    );
    for prose in [
        "no prefix here",
        "Merge pull request #1 from x",
        "fix:it",
        "see https://example.invalid for why",
        "feat(): nothing",
        "chore: ",
        "",
    ] {
        assert_eq!(
            split_conventional(prose),
            (None, prose),
            "{prose:?} should have been left alone"
        );
    }
}

#[test]
fn a_chinese_subject_survives_the_split_intact() {
    let subject = "修复终端右键菜单的本地化";
    assert_eq!(split_conventional(subject), (None, subject));
    let prefixed = "fix(terminal): 修复终端右键菜单的本地化";
    assert_eq!(
        split_conventional(prefixed),
        (Some(("fix", false)), "修复终端右键菜单的本地化")
    );
}

#[test]
fn a_band_keeps_one_segment_per_column() {
    let row = GraphRow {
        node: 0,
        color: 0,
        parents: 2,
        edges: smallvec![
            Edge::Pass { lane: 5, color: 5 },
            Edge::Pass { lane: 7, color: 7 },
            Edge::In { from: 0, color: 0 },
            Edge::Out { to: 0, color: 0 },
            Edge::Out { to: 6, color: 6 },
        ],
    };
    let band = band_of(&row, 3, true, &lanes());
    assert_eq!(band.top[0], Some(lanes().ink[0]));
    assert_eq!(band.top[2], Some(lanes().overflow));
    assert_eq!(band.bottom[0], Some(lanes().ink[0]));
    assert_eq!(band.bottom[2], Some(lanes().overflow));
    assert_eq!(band.top[1], None);
    assert_eq!(band.turns.len(), 1);
    assert_eq!(band.turns[0].0, 0);
    assert_eq!(band.turns[0].1, 2);
}

#[test]
fn a_wide_page_only_neutralises_the_column_that_is_shared() {
    let l = lanes();
    assert_eq!(column_ink(2, 2, 3, false, &l), l.ink[2]);
    assert_eq!(column_ink(2, 2, 3, true, &l), l.overflow);
    assert_eq!(column_ink(1, 1, 3, true, &l), l.ink[1]);
    assert_eq!(column_ink(0, 99, 3, false, &l), l.ink[LANE_SLOTS - 1]);
}

#[test]
fn load_more_growth_stops_at_the_cap() {
    assert_eq!(next_page_request(0), GRAPH_PAGE * 2);
    assert_eq!(next_page_request(GRAPH_PAGE), GRAPH_PAGE * 2);
    assert_eq!(next_page_request(MAX_GRAPH_COMMITS - 1), MAX_GRAPH_COMMITS);
    assert_eq!(next_page_request(MAX_GRAPH_COMMITS), MAX_GRAPH_COMMITS);
}

#[test]
fn the_filter_matches_what_a_row_can_show() {
    let commit = commit_named("feat(ui): the graph", "Ada Lovelace", "c0ffee1234");
    for hit in ["graph", "GRAPH", "feat", "ada", "Lovelace", "c0ffee"] {
        assert!(
            matches_query(&commit, &hit.to_lowercase()),
            "{hit:?} should have matched"
        );
    }
    assert!(!matches_query(&commit, "rationale"));
    assert!(!matches_query(&commit, "ffee"));
}

#[test]
fn the_scope_button_says_which_history_is_showing() {
    assert_eq!(
        scope_label(&GraphScope::HeadAndUpstream),
        t(L10nKey::ScmGraphCurrentBranch)
    );
    assert_eq!(
        scope_label(&GraphScope::All),
        t(L10nKey::ScmGraphAllBranches)
    );
    assert_eq!(
        scope_label(&GraphScope::Refs(vec!["refs/heads/feature/auth".into()])),
        "auth"
    );
}

fn commit_named(summary: &str, author: &str, oid: &str) -> Commit {
    use anterm_core::git::log::{OffsetTs, Signature};
    let who = Signature {
        name: author.to_string(),
        email: "a@b.invalid".into(),
        at: OffsetTs {
            unix: 0,
            offset_minutes: 0,
        },
    };
    Commit {
        oid: oid.to_string(),
        parents: smallvec![],
        author: who.clone(),
        committer: who,
        summary: summary.to_string(),
        body: "rationale goes in the body".into(),
        refs: Vec::new(),
    }
}

fn empty_page() -> CommitPage {
    CommitPage {
        commits: Vec::new(),
        rows: Vec::new(),
        max_lanes: 0,
        scope: GraphScope::HeadAndUpstream,
        requested: GRAPH_PAGE,
        complete: true,
        truncated_lanes: false,
        open_lanes: Vec::new(),
    }
}

mod standalone_graph_gpui_tests {
    use super::*;
    use crate::scm::state::{ScmContext, ScmPanelView};
    use anterm_core::git::status::{HeadState, WorkingTreeStatus};
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};
    use gpui_kit::component::Root;
    use std::time::Instant;

    struct GraphHarness {
        view: Entity<ScmPanelView>,
    }

    impl gpui::Render for GraphHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            v_flex().size_full().child(self.view.clone())
        }
    }

    fn mount(cx: &mut TestAppContext, expanded: bool) -> (Entity<ScmPanelView>, VisualTestContext) {
        cx.executor().allow_parking();
        anterm_ui_kit::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(anterm_ui_kit::config::Config::default());
            anterm_ui_kit::theme::apply_theme(None, cx);
        });
        let view = cx.new(|_| ScmPanelView::new(expanded));
        let view_for_root = view.clone();
        let window = cx.open_window(size(px(320.), px(520.)), move |window, cx| {
            let harness = cx.new(|_| GraphHarness {
                view: view_for_root,
            });
            Root::new(harness, window, cx)
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        (view, vcx)
    }

    #[gpui::test]
    fn only_a_breaking_prefix_gets_a_colour(cx: &mut TestAppContext) {
        let (_view, mut vcx) = mount(cx, false);
        vcx.update(|_, cx| {
            assert_eq!(type_tone(false, cx), cx.theme().muted_foreground);
            assert_eq!(type_tone(true, cx), cx.theme().danger);
            assert_ne!(type_tone(true, cx), type_tone(false, cx));
        });
    }

    #[gpui::test]
    fn opening_a_row_hands_that_commit_to_the_detail_view(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount(cx, false);

        view.update(&mut vcx, |view, cx| {
            view.graph_open_commit(repo(), "deadbeef".into(), None, cx)
        });
        let (selected, detail) = view.read_with(&vcx, |view, _| {
            (view.graph.selected.clone(), view.detail.clone())
        });
        assert_eq!(selected.as_deref(), Some("deadbeef"));
        let detail = detail.expect("the row opened a commit");
        assert_eq!(detail.oid, "deadbeef");
        assert_eq!(detail.repo, repo());
        assert!(detail.commit.is_none());

        let seed = commit_named("a subject", "someone", "deadbeef");
        view.update(&mut vcx, |view, cx| {
            view.graph_open_commit(repo(), "deadbeef".into(), Some(seed.clone()), cx)
        });
        let detail = view
            .read_with(&vcx, |view, _| view.detail.clone())
            .expect("the row opened a commit");
        assert_eq!(detail.commit.as_deref(), Some(&seed));
    }

    #[gpui::test]
    fn switching_scope_and_repo_resets_page_and_search_clears_query(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount(cx, false);

        view.update(&mut vcx, |view, cx| {
            view.graph.requested = GRAPH_PAGE * 3;
            view.graph.page = Some(Arc::new(empty_page()));
            view.graph.page_key = Some((repo(), 7, GraphScope::HeadAndUpstream));
            view.graph_set_scope(GraphScope::All, cx);
        });
        view.read_with(&vcx, |view, _| {
            assert_eq!(view.graph.scope, GraphScope::All);
            assert_eq!(view.graph.requested, GRAPH_PAGE);
            assert!(view.graph.page.is_none());
        });

        let other = RepoKey {
            host: HostId::LOCAL,
            root: PathBuf::from("/tmp/anterm-graph-test-other"),
        };
        view.update(&mut vcx, |view, cx| {
            view.graph.requested = GRAPH_PAGE * 3;
            view.graph.page = Some(Arc::new(empty_page()));
            view.graph.page_key = Some((repo(), 7, GraphScope::All));
            view.scm_load_graph(&other, cx);
        });
        view.read_with(&vcx, |view, _| {
            assert!(view.graph.page.is_none());
            assert_eq!(view.graph.requested, 0);
        });

        view.update_in(&mut vcx, |view, window, cx| {
            assert!(view.graph_query(cx).is_none());
            view.graph_toggle_search(window, cx);
            let input = view.graph.search.clone().expect("the field is open");
            input.update(cx, |state, cx| state.set_value("Feat", window, cx));
        });
        assert_eq!(
            view.read_with(&vcx, |view, cx| view.graph_query(cx)),
            Some("feat".to_string())
        );
        view.update_in(&mut vcx, |view, window, cx| {
            view.graph_toggle_search(window, cx);
        });
        assert!(view.read_with(&vcx, |view, cx| view.graph_query(cx).is_none()));
    }

    #[gpui::test]
    fn a_replaced_page_does_not_inherit_the_old_rows(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount(cx, false);
        let page_of = |n: usize| {
            let mut page = empty_page();
            page.commits = (0..n)
                .map(|i| commit_named("feat: x", "Ada", &format!("{i:040}")))
                .collect();
            Arc::new(page)
        };

        view.update(&mut vcx, |view, _| {
            let long = page_of(3);
            assert_eq!(*view.graph_visible_rows(&long, None), vec![0, 1, 2]);
            drop(long);

            let short = page_of(1);
            assert_eq!(*view.graph_visible_rows(&short, None), vec![0]);
        });
    }

    #[gpui::test]
    fn large_history_is_windowed_with_two_thousand_commits(cx: &mut TestAppContext) {
        let (view, mut vcx) = mount(cx, true);
        let root = PathBuf::from("/graph-virtual-repo");
        let host = anterm_core::host::local::LocalHost::new();
        let key = RepoKey {
            host: HostId::LOCAL,
            root: root.clone(),
        };

        let status = Arc::new(WorkingTreeStatus {
            root: root.clone(),
            home: root.clone(),
            head: HeadState::Branch {
                name: "main".into(),
                oid: "0".repeat(40),
            },
            upstream: None,
            ahead_behind: None,
            total_entries: 0,
            entries: Vec::new(),
            truncated: false,
            stash_count: 0,
            operation: None,
            prefilled_message: None,
        });
        let mut page = empty_page();
        page.commits = (0..2_000)
            .map(|i| commit_named(&format!("feat: commit {i}"), "Ada", &format!("{i:040}")))
            .collect();
        page.rows = (0..2_000)
            .map(|_| GraphRow {
                node: 0,
                color: 0,
                parents: 1,
                edges: smallvec![
                    Edge::In { from: 0, color: 0 },
                    Edge::Out { to: 0, color: 0 },
                ],
            })
            .collect();
        page.max_lanes = 1;
        page.requested = 2_000;

        vcx.update(|_, cx| {
            cx.default_global::<anterm_ui_kit::git::data::ScmData>()
                .seed_status_for_test(HostId::LOCAL, root.clone(), status);
        });

        row_probe::reset();
        view.update(&mut vcx, |view, cx| {
            view.branches.insert(key.clone(), (0, vec!["main".into()]));
            view.roots.insert(
                (HostId::LOCAL, root.clone()),
                (Instant::now(), Some(root.clone())),
            );
            view.graph.requested = 2_000;
            view.graph.page = Some(Arc::new(page));
            view.graph.page_key = Some((key, 0, GraphScope::HeadAndUpstream));
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
            "expected windowed commit graph to mount only visible rows for 2,000 commits, got {rendered}"
        );
    }
}
