use anterm_core::machine::{LayoutDelta, Machine, Side};

use super::reconcile::{
    REHYDRATE_SETTLED, apply_to_mirror, finish_prime, hydrate_with, hydration_log_level,
    open_parked_path, owe_rehydration, rehydrate_backoff, set_gui_ratio, settle_rebuild,
    still_owed, take_rehydrate,
};
use super::*;

fn retiring(tab: TabId, panes: Vec<u64>) -> Retiring {
    Retiring {
        tab,
        panes,
        route: crate::terminal::PaneRoute::Local,
    }
}

#[test]
fn only_a_retiring_tab_is_closed_into_the_machines_list() {
    let (closed, moved) = (TabId::new(), TabId::new());
    let mut ops = vec![
        ControlRequest::TabClose { tab: moved },
        ControlRequest::TabClose { tab: closed },
    ];
    let mut waiting = vec![retiring(closed, vec![3, 4])];

    let sent = remember_closes(&mut ops, &mut waiting);

    assert_eq!(
        ops,
        vec![
            ControlRequest::TabClose { tab: moved },
            ControlRequest::TabCloseRemembered {
                tab: closed,
                panes: vec![3, 4],
            },
        ]
    );
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].tab, closed);
    assert!(
        waiting.is_empty(),
        "handed on, so nothing is left to fall back on"
    );
}

#[test]
fn a_retirement_with_no_close_in_the_sync_stays_waiting() {
    let mut ops = vec![ControlRequest::TabMove {
        tab: TabId::new(),
        to: 0,
    }];
    let tab = TabId::new();
    let mut waiting = vec![retiring(tab, vec![3])];

    assert!(remember_closes(&mut ops, &mut waiting).is_empty());
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].tab, tab);
}

#[test]
fn a_reopened_tab_reads_back_its_layout_and_what_each_pane_was() {
    use anterm_core::machine::{PaneRecord, ReopenedTab};
    let tab = TreeTab {
        name: Some("api".into()),
        root: PaneNode::Split {
            axis: TreeAxis::Vertical,
            ratio: 0.3,
            a: Box::new(PaneNode::Leaf { pane: 7 }),
            b: Box::new(PaneNode::Leaf { pane: 8 }),
        },
        ..TreeTab::leaf(0)
    };
    let mut agent_pane = PaneRecord::new(8);
    agent_pane.cwd = Some("/repo".into());
    agent_pane.agent = Some(AgentFacts {
        agent: crate::core::cli_agent::CLIAgent::Claude,
        session_id: Some("abc".into()),
        launch_argv: None,
        status: None,
    });
    let mut shell_pane = PaneRecord::new(7);
    shell_pane.cwd = Some("/srv".into());
    let reopened = ReopenedTab {
        tab: tab.clone(),
        closed_at: 1,
        panes: vec![shell_pane, agent_pane],
    };

    let st = session_tab_from_reopened(&reopened);

    assert_eq!(st.tree_id, Some(tab.id));
    assert_eq!(st.name.as_deref(), Some("api"));
    assert!(!st.hibernated);
    let SessionPane::Split { ratio, a, b, .. } = st.pane else {
        panic!("the split comes back a split");
    };
    assert_eq!(ratio, 0.3);
    let SessionPane::Leaf { pane_id, cwd, .. } = *a else {
        panic!("leaf");
    };
    assert_eq!(pane_id, Some(7));
    assert_eq!(cwd, Some(std::path::PathBuf::from("/srv")));
    let SessionPane::Leaf {
        pane_id,
        agent,
        agent_session_id,
        ..
    } = *b
    else {
        panic!("leaf");
    };
    assert_eq!(pane_id, Some(8));
    assert_eq!(agent, Some(crate::core::cli_agent::CLIAgent::Claude));
    assert_eq!(agent_session_id.as_deref(), Some("abc"));
}

#[test]
fn note_instance_reports_only_a_real_change() {
    let mut seen = String::new();

    assert!(
        !note_instance(&mut seen, "abc"),
        "a first sighting is not a restart"
    );
    assert!(
        !note_instance(&mut seen, "abc"),
        "the same process is not a restart"
    );
    assert!(note_instance(&mut seen, "def"), "a new process is");
    assert!(!note_instance(&mut seen, "def"));
}

#[test]
fn note_instance_ignores_a_server_that_predates_the_field() {
    let mut seen = String::from("abc");

    assert!(
        !note_instance(&mut seen, ""),
        "an unknown instance is never a restart"
    );
    assert_eq!(seen, "abc", "and it must not overwrite what was seen");

    let mut fresh = String::new();
    assert!(!note_instance(&mut fresh, ""));
    assert!(
        !note_instance(&mut fresh, "abc"),
        "the first real instance after an unknown one is a first sighting"
    );
}

#[cfg(unix)]
#[test]
fn a_peer_without_the_machine_tree_bit_classifies_as_unserved() {
    use anterm_core::daemon::control::ControlHello;
    use anterm_core::host::local::LocalHost;
    use anterm_core::host::server::{Services, serve_with};

    let connect = |services: Services| {
        let (server, client) = std::os::unix::net::UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let _ = serve_with(server, LocalHost::new(), services);
        });
        let hello = ControlHello::host_rpc("test-token", "test-host");
        Arc::new(
            anterm_core::daemon::control::ControlClient::over_unix(
                client,
                &hello,
                Box::new(|_| {}),
            )
            .unwrap(),
        )
    };

    let treeless = connect(Services::none());
    assert!(matches!(
        classify_tree_link(Some(treeless)),
        TreeLink::Unserved
    ));

    let dir = std::env::temp_dir().join(format!("anterm-treelink-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = anterm_core::machine::MachineStore::open(
        dir.join(anterm_core::machine::MACHINE_FILE),
    );
    let serving = connect(Services::with_machine(store));
    assert!(matches!(
        classify_tree_link(Some(serving)),
        TreeLink::Ready(_)
    ));

    assert!(matches!(classify_tree_link(None), TreeLink::Down));
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui::test]
fn a_parked_folder_outlives_the_pull_that_has_to_be_retried(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let path = std::path::PathBuf::from("/tmp/from-explorer");
        crate::ui::workspace::windows::WindowRegistry::init(cx);

        let parked = |cx: &mut gpui::App| -> Vec<std::path::PathBuf> {
            cx.default_global::<TreeSync>()
                .then_open
                .iter()
                .map(|open| match open {
                    ParkedOpen::Folder(cwd) => cwd.clone(),
                    ParkedOpen::Command { cwd, .. } => cwd.clone(),
                    ParkedOpen::Ssh(_) => std::path::PathBuf::from("<ssh>"),
                })
                .collect()
        };

        hydrate_window_then_open(cx, path.clone());
        assert_eq!(
            parked(cx),
            vec![path.clone()],
            "the request must be parked, not opened over a layout still in flight"
        );

        hydrate_with(cx, Adopt::IfEmpty, Vec::new());
        assert_eq!(
            parked(cx),
            vec![path.clone()],
            "a retry must still owe the folder"
        );

        assert!(park_command_while_pulling(cx, &path, "./build.sh"));
        assert_eq!(parked(cx), vec![path.clone()]);
        assert!(matches!(
            &cx.default_global::<TreeSync>().then_open[0],
            ParkedOpen::Command { command, .. } if command == "./build.sh"
        ));

        let script = std::path::PathBuf::from("/tmp/from-finder");
        assert!(park_command_while_pulling(cx, &script, "./deploy.sh"));
        assert_eq!(parked(cx), vec![path.clone(), script]);

        open_parked_path(cx);
        assert!(parked(cx).is_empty());

        cx.default_global::<TreeSync>().sync = SyncPhase::Primed(WsMirror::default());
        assert!(!park_command_while_pulling(cx, &path, "./build.sh"));
    });
}

#[test]
fn a_sync_closes_only_the_tabs_the_user_closed() {
    let tab = |pane| TreeTab {
        id: TabId::new(),
        name: None,
        tags: Vec::new(),
        root: PaneNode::Leaf { pane },
        hibernated: false,
    };
    let mirror = WsMirror {
        tabs: vec![tab(1), tab(2)],
        ..Default::default()
    };
    let (a, b) = (mirror.tabs[0].id, mirror.tabs[1].id);
    let sync = |closing: &mut Vec<TabId>| {
        let mut mirror = mirror.clone();
        let held = unaccounted(&mirror, &[], closing);
        diff(&mut mirror, &[], None, SyncScope::Full, &held)
    };

    assert!(
        sync(&mut Vec::new()).is_empty(),
        "an emptied window must not close tabs on the machine"
    );

    let mut closing = vec![b];
    assert_eq!(
        sync(&mut closing),
        vec![ControlRequest::TabClose { tab: b }],
        "the tab the user closed goes; the one merely missing stays"
    );
    assert_ne!(a, b);

    let mut closing = vec![TabId::new(), b];
    let _ = sync(&mut closing);
    assert_eq!(
        closing,
        vec![b],
        "an intent the machine has no tab for is spent; one it still has stays until it lands"
    );
}

#[gpui::test]
fn a_failed_daemon_change_stops_the_emptied_window_speaking_for_the_machine(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let tab = TabId::new();
        {
            let state = cx.default_global::<TreeSync>();
            state.informed = true;
            state.sync = SyncPhase::Primed(WsMirror::default());
            state.queue.push_back(ControlRequest::TabClose { tab });
        }

        resync_window_from_tree(cx);

        let state = cx.default_global::<TreeSync>();
        assert!(!state.informed);
        assert!(matches!(state.sync, SyncPhase::Unprimed { .. }));
        assert!(state.queue.is_empty());
    });
}

#[gpui::test]
fn a_hydration_that_died_on_a_stale_link_is_owed_back(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let epoch = {
            let state = cx.default_global::<TreeSync>();
            state.sync = SyncPhase::Unprimed {
                dirty: false,
                priming: true,
            };
            state.epoch
        };
        owe_rehydration(cx, epoch, Adopt::Replace);
        let state = cx.default_global::<TreeSync>();
        assert!(matches!(
            state.sync,
            SyncPhase::Unprimed { priming: false, .. }
        ));
        assert!(state.rehydrate.is_some());

        {
            let state = cx.default_global::<TreeSync>();
            state.rehydrate = None;
            state.epoch += 1;
        }
        owe_rehydration(cx, epoch, Adopt::Replace);
        assert!(cx.default_global::<TreeSync>().rehydrate.is_none());
    });
}

#[gpui::test]
fn an_owed_pull_is_retried_until_it_is_paid_or_superseded(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let epoch = cx.default_global::<TreeSync>().epoch;
        owe_rehydration(cx, epoch, Adopt::IfEmpty);
        assert!(still_owed(cx, epoch));

        cx.default_global::<TreeSync>().rehydrate = None;
        assert!(!still_owed(cx, epoch));

        let state = cx.default_global::<TreeSync>();
        state.rehydrate = Some(Adopt::IfEmpty);
        state.epoch += 1;
        assert!(!still_owed(cx, epoch));
        assert!(still_owed(cx, epoch + 1));
    });
}

#[test]
fn the_retry_backs_off_and_settles_at_a_cap() {
    let secs = |n| rehydrate_backoff(n).as_secs();
    assert_eq!(secs(1), 2, "the first retry is prompt: a window is empty");
    assert!(secs(1) < secs(2) && secs(2) < secs(3));
    assert_eq!(secs(REHYDRATE_SETTLED), 30);
    assert_eq!(secs(50), 30);
}

#[test]
fn a_standing_failure_stops_shouting_once_the_backoff_settles() {
    assert_eq!(hydration_log_level(1, log::Level::Warn), log::Level::Warn);
    assert_eq!(
        hydration_log_level(REHYDRATE_SETTLED, log::Level::Warn),
        log::Level::Debug
    );
    assert_eq!(
        hydration_log_level(REHYDRATE_SETTLED, log::Level::Info),
        log::Level::Debug
    );
}

#[gpui::test]
fn the_backoff_count_ends_with_the_run_of_failures(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let unprimed = |cx: &mut App| {
            cx.default_global::<TreeSync>().sync = SyncPhase::Unprimed {
                dirty: false,
                priming: true,
            };
        };
        let attempts = |cx: &mut App| cx.default_global::<TreeSync>().rehydrate_attempts;

        unprimed(cx);
        let epoch = cx.default_global::<TreeSync>().epoch;
        for expected in 1..=3 {
            unprimed(cx);
            owe_rehydration(cx, epoch, Adopt::IfEmpty);
            assert_eq!(attempts(cx), expected);
        }

        unprimed(cx);
        finish_prime(cx, epoch, Ok(WsMirror::default()));
        assert_eq!(attempts(cx), 0);

        {
            let state = cx.default_global::<TreeSync>();
            state.rehydrate = Some(Adopt::Replace);
            state.owed_over = Vec::new();
            state.rehydrate_attempts = 4;
        }
        assert!(take_rehydrate(cx, &[TabId::new()]).is_none());
        assert_eq!(attempts(cx), 0);
    });
}

#[gpui::test]
async fn a_retry_that_finds_no_window_stands_down(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        crate::ui::workspace::windows::WindowRegistry::init(cx);
        let epoch = cx.default_global::<TreeSync>().epoch;
        owe_rehydration(cx, epoch, Adopt::IfEmpty);
    });

    cx.executor().advance_clock(rehydrate_backoff(1) * 2);
    cx.executor().run_until_parked();

    cx.update(|cx| {
        assert!(cx.default_global::<TreeSync>().rehydrate.is_some());
    });
}

#[gpui::test]
fn a_window_that_filled_up_while_owed_keeps_what_it_has(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let stale = [TabId::new(), TabId::new()];
        let arm = |cx: &mut App, adopt, over: &[TabId]| {
            let state = cx.default_global::<TreeSync>();
            state.rehydrate = Some(adopt);
            state.owed_over = over.to_vec();
        };

        arm(cx, Adopt::Replace, &[]);
        assert!(take_rehydrate(cx, &[]).is_some());
        assert!(take_rehydrate(cx, &[]).is_none());

        arm(cx, Adopt::Replace, &stale);
        assert!(take_rehydrate(cx, &stale).is_some());

        arm(cx, Adopt::Replace, &stale);
        assert!(take_rehydrate(cx, &stale[..1]).is_some());

        arm(cx, Adopt::Replace, &stale);
        assert!(take_rehydrate(cx, &[stale[0], TabId::new()]).is_none());
        assert!(cx.default_global::<TreeSync>().rehydrate.is_none());

        arm(cx, Adopt::IfEmpty, &[]);
        assert!(take_rehydrate(cx, &[TabId::new()]).is_some());
    });
}

#[gpui::test]
fn a_resync_takes_back_the_licence_to_prune_until_a_pull_lands(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.default_global::<TreeSync>().informed = true;
        resync_window_from_tree(cx);
        assert!(!cx.default_global::<TreeSync>().informed);

        mark_window_informed(cx);
        assert!(cx.default_global::<TreeSync>().informed);
    });
}

#[gpui::test]
fn an_if_empty_pull_leaves_the_licence_where_it_found_it(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.default_global::<TreeSync>().informed = true;
        hydrate_window_from_tree(cx);
        assert!(cx.default_global::<TreeSync>().informed);
    });
}

#[gpui::test]
fn a_partial_rebuild_holds_the_tabs_it_could_not_put_up(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let (put_up, failed) = (TabId::new(), TabId::new());
        let settled = |cx: &mut App| {
            let state = cx.default_global::<TreeSync>();
            (state.informed, state.not_rebuilt.clone())
        };

        settle_rebuild(cx, 2, &[put_up, failed], &[put_up]);
        assert_eq!(settled(cx), (true, vec![failed]));

        settle_rebuild(cx, 2, &[put_up, failed], &[put_up, failed]);
        assert_eq!(settled(cx), (true, vec![]));

        cx.default_global::<TreeSync>().informed = false;
        settle_rebuild(cx, 2, &[put_up, failed], &[]);
        assert_eq!(settled(cx), (false, vec![put_up, failed]));

        settle_rebuild(cx, 0, &[], &[]);
        assert_eq!(settled(cx), (true, vec![]));

        cx.default_global::<TreeSync>().informed = false;
        settle_rebuild(cx, 2, &[], &[]);
        assert_eq!(settled(cx), (false, vec![]));
    });
}

#[gpui::test]
fn the_next_sync_leaves_a_tab_the_rebuild_could_not_put_up_on_the_machine(
    cx: &mut gpui::TestAppContext,
) {
    let (app, mut vcx, _pane_stream) = crate::ui::workspace::app::test_window::harness_with_pane(cx);
    let (put_up, failed) = (TabId::new(), TabId::new());
    app.update_in(&mut vcx, |app, _, cx| {
        app.tabs[0].tree_id.set(put_up);
        {
            let state = cx.default_global::<TreeSync>();
            state.sync = SyncPhase::Primed(WsMirror {
                tabs: vec![
                    TreeTab {
                        id: put_up,
                        name: None,
                        tags: Vec::new(),
                        root: PaneNode::Leaf { pane: 1 },
                        hibernated: false,
                    },
                    TreeTab {
                        id: failed,
                        name: None,
                        tags: Vec::new(),
                        root: PaneNode::Leaf { pane: 2 },
                        hibernated: false,
                    },
                ],
                active: Some(put_up),
                ..Default::default()
            });
            state.inflight = true;
        }
        settle_rebuild(cx, 2, &[put_up, failed], &[put_up]);
        assert!(cx.default_global::<TreeSync>().informed);

        sync_window(app, cx);

        let state = cx.default_global::<TreeSync>();
        assert!(
            !state
                .queue
                .iter()
                .any(|op| matches!(op, ControlRequest::TabClose { .. })),
            "the tab that failed to come back is not one the user closed: {:?}",
            state.queue
        );
        match &state.sync {
            SyncPhase::Primed(mirror) => assert_eq!(
                mirror.tabs.iter().map(|t| t.id).collect::<Vec<_>>(),
                vec![put_up, failed]
            ),
            SyncPhase::Unprimed { .. } => {
                panic!("the sync must not have thrown the mirror away")
            }
        }
    });
}

#[test]
fn a_ratio_delta_is_clamped_to_the_servers_band_not_a_narrower_one() {
    let mut pane = Pane::split_node(gpui::Axis::Horizontal, 0.5, Pane::Empty, Pane::Empty);
    assert!(set_gui_ratio(&mut pane, &[], 0.07));
    match &pane {
        Pane::Split { ratio, .. } => assert_eq!(ratio.get(), 0.07),
        _ => unreachable!("built as a split"),
    }
    assert!(set_gui_ratio(&mut pane, &[], 0.01));
    match &pane {
        Pane::Split { ratio, .. } => assert_eq!(ratio.get(), 0.05),
        _ => unreachable!("built as a split"),
    }
}

#[test]
fn a_tab_created_delta_that_straddled_a_repull_lands_once_in_the_window_mirror() {
    let mut mirror = WsMirror::default();
    let delta = LayoutDelta::TabCreated {
        at: 0,
        tab: TreeTab::leaf(1),
    };
    assert!(apply_to_mirror(&mut mirror, &delta));
    assert!(apply_to_mirror(&mut mirror, &delta));
    assert_eq!(mirror.tabs.len(), 1);
}

#[gpui::test]
fn a_superseded_prime_result_does_not_roll_the_mirror_back(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let stale_epoch = {
            let state = cx.default_global::<TreeSync>();
            state.sync = SyncPhase::Unprimed {
                dirty: false,
                priming: true,
            };
            state.epoch
        };
        let advanced = WsMirror {
            tabs: vec![TreeTab::leaf(7)],
            ..Default::default()
        };
        {
            let state = cx.default_global::<TreeSync>();
            state.epoch += 1;
            state.sync = SyncPhase::Primed(advanced.clone());
        }

        finish_prime(cx, stale_epoch, Ok(WsMirror::default()));

        match &cx.default_global::<TreeSync>().sync {
            SyncPhase::Primed(mirror) => assert_eq!(*mirror, advanced),
            _ => panic!("the mirror was dropped entirely"),
        }
    });
}

fn seed(pane: u64) -> PaneSeed {
    PaneSeed {
        pane,
        cwd: Some(format!("/work/{pane}")),
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

fn leaf(pane: u64) -> DesiredNode {
    DesiredNode::Leaf {
        pane,
        seed: seed(pane),
    }
}

fn split(axis: TreeAxis, ratio: f32, a: DesiredNode, b: DesiredNode) -> DesiredNode {
    DesiredNode::Split {
        axis,
        ratio,
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn tab(id: TabId, root: DesiredNode) -> DesiredTab {
    DesiredTab {
        id,
        name: None,
        tags: Vec::new(),
        root,
        hibernated: false,
    }
}

#[test]
fn seeded_records_carry_the_seed_and_the_windows_own_liveness() {
    let desired = vec![
        tab(
            TabId::new(),
            split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)),
        ),
        tab(TabId::new(), leaf(3)),
    ];
    let records = seeded_records(&desired, |pane| pane != 2);
    assert_eq!(
        records.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(records[0].cwd.as_deref(), Some("/work/1"));
    assert!(records[0].live);
    assert!(!records[1].live);
    assert!(records[2].live);
}

fn assert_converged(mirror: &WsMirror, desired: &[DesiredTab]) {
    assert_eq!(mirror.tabs.len(), desired.len());
    for (m, d) in mirror.tabs.iter().zip(desired) {
        assert_eq!(m.id, d.id);
        assert_eq!(m.name, d.name);
        assert_eq!(m.tags, d.tags);
        assert_eq!(m.root, d.root.to_pane_node());
    }
}

#[test]
fn opening_the_first_tab_emits_a_create_carrying_the_client_identity() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let desired = vec![tab(id, leaf(7))];

    let ops = diff(&mut mirror, &desired, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::TabCreate {
            at: Some(0),
            pane: seed(7),
            tab: Some(id),
        }],
    );
    assert_converged(&mirror, &desired);
    assert_eq!(mirror.active, Some(id));
}

#[test]
fn a_split_emits_one_pane_split_against_its_sibling() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let one = vec![tab(id, leaf(1))];
    diff(&mut mirror, &one, Some(id), SyncScope::Full, &[]);

    let two = vec![tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))];
    let ops = diff(&mut mirror, &two, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSplit {
            pane: 1,
            axis: TreeAxis::Vertical,
            ratio: 0.5,
            new: seed(2),
            first: false,
        }]
    );
    assert_converged(&mirror, &two);
}

#[test]
fn a_new_pane_on_the_upper_side_splits_with_first_set() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(&mut mirror, &[tab(id, leaf(1))], Some(id), SyncScope::Full, &[]);

    let want = vec![tab(id, split(TreeAxis::Horizontal, 0.4, leaf(2), leaf(1)))];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSplit {
            pane: 1,
            axis: TreeAxis::Horizontal,
            ratio: 0.4,
            new: seed(2),
            first: true,
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn closing_a_pane_emits_pane_close_and_the_split_collapses() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        &mut mirror,
        &[tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, leaf(1))];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(ops, vec![ControlRequest::PaneClose { pane: 2 }]);
    assert_converged(&mirror, &want);
}

#[test]
fn dragging_a_pane_across_the_layout_emits_one_pane_move() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let grid = |a: DesiredNode, b: DesiredNode| split(TreeAxis::Vertical, 0.5, a, b);
    let before = vec![tab(
        id,
        grid(split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)), leaf(3)),
    )];
    diff(&mut mirror, &before, Some(id), SyncScope::Full, &[]);

    let after = vec![tab(
        id,
        grid(leaf(2), split(TreeAxis::Vertical, 0.5, leaf(3), leaf(1))),
    )];
    let ops = diff(&mut mirror, &after, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneMove {
            pane: 1,
            to: 3,
            axis: TreeAxis::Vertical,
            first: false,
        }]
    );
    assert_converged(&mirror, &after);
}

#[test]
fn merging_a_tab_into_another_moves_its_panes_and_takes_the_tab_with_them() {
    let (host, guest) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = vec![
        tab(host, leaf(1)),
        tab(guest, split(TreeAxis::Horizontal, 0.5, leaf(2), leaf(3))),
    ];
    diff(&mut mirror, &before, Some(host), SyncScope::Full, &[]);

    let after = vec![tab(
        host,
        split(
            TreeAxis::Horizontal,
            0.5,
            leaf(1),
            split(TreeAxis::Vertical, 0.5, leaf(2), leaf(3)),
        ),
    )];
    let ops = diff(&mut mirror, &after, Some(host), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::PaneMove {
                pane: 2,
                to: 1,
                axis: TreeAxis::Horizontal,
                first: false,
            },
            ControlRequest::PaneMove {
                pane: 3,
                to: 2,
                axis: TreeAxis::Vertical,
                first: false,
            },
        ]
    );
    assert_converged(&mirror, &after);
    assert_eq!(mirror.active, Some(host));
}

#[test]
fn a_tab_grafted_above_a_whole_layout_still_converges() {
    let (host, guest) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = vec![
        tab(host, split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2))),
        tab(guest, leaf(3)),
    ];
    diff(&mut mirror, &before, Some(host), SyncScope::Full, &[]);

    let after = vec![tab(
        host,
        split(
            TreeAxis::Horizontal,
            0.33,
            leaf(3),
            split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
        ),
    )];
    diff(&mut mirror, &after, Some(host), SyncScope::Full, &[]);
    assert_converged(&mirror, &after);
}

#[test]
fn a_pane_leaving_for_a_tab_of_its_own_gives_it_up_before_it_asks_for_it() {
    let (held, fresh) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = vec![tab(
        held,
        split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
    )];
    diff(&mut mirror, &before, Some(held), SyncScope::Full, &[]);

    let after = vec![tab(fresh, leaf(2)), tab(held, leaf(1))];
    let ops = diff(&mut mirror, &after, Some(fresh), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::PaneClose { pane: 2 },
            ControlRequest::TabCreate {
                at: Some(0),
                pane: seed(2),
                tab: Some(fresh),
            },
        ]
    );
    assert_converged(&mirror, &after);
}

#[test]
fn a_move_that_lands_on_a_new_ratio_settles_it_after_the_move() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let before = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
            leaf(3),
        ),
    )];
    diff(&mut mirror, &before, Some(id), SyncScope::Full, &[]);

    let after = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            leaf(2),
            split(TreeAxis::Vertical, 0.25, leaf(3), leaf(1)),
        ),
    )];
    let ops = diff(&mut mirror, &after, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::PaneMove {
                pane: 1,
                to: 3,
                axis: TreeAxis::Vertical,
                first: false,
            },
            ControlRequest::PaneSetRatio {
                tab: id,
                path: vec![Side::B],
                ratio: 0.25,
            },
        ]
    );
    assert_converged(&mirror, &after);
}

#[test]
fn a_swap_no_single_op_expresses_rebuilds_the_tab_whole() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let before = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            split(TreeAxis::Horizontal, 0.5, leaf(1), leaf(2)),
            split(TreeAxis::Horizontal, 0.5, leaf(3), leaf(4)),
        ),
    )];
    diff(&mut mirror, &before, Some(id), SyncScope::Full, &[]);

    let after = vec![tab(
        id,
        split(
            TreeAxis::Vertical,
            0.5,
            split(TreeAxis::Horizontal, 0.5, leaf(4), leaf(2)),
            split(TreeAxis::Horizontal, 0.5, leaf(3), leaf(1)),
        ),
    )];
    let ops = diff(&mut mirror, &after, Some(id), SyncScope::Full, &[]);
    assert!(matches!(ops.first(), Some(ControlRequest::TabClose { .. })));
    assert_converged(&mirror, &after);
}

#[test]
fn a_revived_leaf_emits_pane_replace_with_the_successors_seed() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(
        &mut mirror,
        &[tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(9)))];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneReplace {
            old: 2,
            new: seed(9),
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn a_ratio_drag_emits_set_ratio_with_the_splits_path() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let nested = |r| {
        split(
            TreeAxis::Vertical,
            0.5,
            leaf(1),
            split(TreeAxis::Horizontal, r, leaf(2), leaf(3)),
        )
    };
    diff(
        &mut mirror,
        &[tab(id, nested(0.5))],
        Some(id),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(id, nested(0.7))];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::PaneSetRatio {
            tab: id,
            path: vec![Side::B],
            ratio: 0.7,
        }]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn closing_a_tab_emits_tab_close_and_heals_the_active_tab() {
    let (a, b) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    diff(
        &mut mirror,
        &[tab(a, leaf(1)), tab(b, leaf(2))],
        Some(b),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(a, leaf(1))];
    let ops = diff(&mut mirror, &want, None, SyncScope::Full, &[]);
    assert_eq!(ops, vec![ControlRequest::TabClose { tab: b }]);
    assert_converged(&mirror, &want);
    assert_eq!(mirror.active, Some(a));
}

#[test]
fn a_tab_reorder_emits_moves_that_land_the_windows_order() {
    let (a, b, c) = (TabId::new(), TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let before = [tab(a, leaf(1)), tab(b, leaf(2)), tab(c, leaf(3))];
    diff(&mut mirror, &before, Some(c), SyncScope::Full, &[]);

    let want = vec![tab(c, leaf(3)), tab(a, leaf(1)), tab(b, leaf(2))];
    let ops = diff(&mut mirror, &want, Some(c), SyncScope::Full, &[]);
    assert_eq!(ops, vec![ControlRequest::TabMove { tab: c, to: 0 }]);
    assert_converged(&mirror, &want);
}

#[test]
fn renaming_and_tagging_emit_their_label_ops() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(&mut mirror, &[tab(id, leaf(1))], Some(id), SyncScope::Full, &[]);

    let mut named = tab(id, leaf(1));
    named.name = Some("build".into());
    let tag = TagId::new();
    named.tags = vec![tag];
    let want = vec![named];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::TabRename {
                tab: id,
                name: Some("build".into()),
            },
            ControlRequest::TabSetTags {
                tab: id,
                tags: vec![tag],
            },
        ]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn switching_tabs_emits_only_set_active_tab() {
    let (a, b) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    let both = [tab(a, leaf(1)), tab(b, leaf(2))];
    diff(&mut mirror, &both, Some(b), SyncScope::Full, &[]);

    let ops = diff(&mut mirror, &both, Some(a), SyncScope::Full, &[]);
    assert_eq!(ops, vec![ControlRequest::SetActiveTab { tab: a }]);
    assert_eq!(mirror.active, Some(a));
}

#[test]
fn a_deep_tree_materializes_top_split_first_and_converges() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let want = vec![tab(
        id,
        split(
            TreeAxis::Horizontal,
            0.6,
            split(TreeAxis::Vertical, 0.3, leaf(1), leaf(2)),
            split(TreeAxis::Vertical, 0.7, leaf(3), leaf(4)),
        ),
    )];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::TabCreate {
                at: Some(0),
                pane: seed(1),
                tab: Some(id),
            },
            ControlRequest::PaneSplit {
                pane: 1,
                axis: TreeAxis::Horizontal,
                ratio: 0.6,
                new: seed(3),
                first: false,
            },
            ControlRequest::PaneSplit {
                pane: 1,
                axis: TreeAxis::Vertical,
                ratio: 0.3,
                new: seed(2),
                first: false,
            },
            ControlRequest::PaneSplit {
                pane: 3,
                axis: TreeAxis::Vertical,
                ratio: 0.7,
                new: seed(4),
                first: false,
            },
        ]
    );
    assert_converged(&mirror, &want);
}

#[test]
fn an_unchanged_window_emits_nothing() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let want = vec![tab(id, split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2)))];
    diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]),
        Vec::new()
    );
}

#[test]
fn a_tab_whose_panes_are_all_still_spawning_is_held_not_closed() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(&mut mirror, &[tab(id, leaf(1))], Some(id), SyncScope::Full, &[]);

    let ops = diff(&mut mirror, &[], None, SyncScope::Full, &[id]);
    assert_eq!(ops, Vec::new());
    assert_eq!(mirror.tabs.len(), 1);
}

#[test]
fn an_additive_diff_never_closes_tabs_the_window_has_not_seen() {
    let (a, b) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    diff(
        &mut mirror,
        &[tab(a, leaf(1)), tab(b, leaf(2))],
        Some(b),
        SyncScope::Full,
        &[],
    );

    let fresh = TabId::new();
    let ops = diff(
        &mut mirror,
        &[tab(fresh, leaf(9))],
        Some(fresh),
        SyncScope::Additive,
        &[],
    );
    assert_eq!(
        ops,
        vec![ControlRequest::TabCreate {
            at: Some(2),
            pane: seed(9),
            tab: Some(fresh),
        }]
    );
    assert_eq!(mirror.tabs.len(), 3);
}

#[test]
fn deltas_advance_the_mirror_exactly_as_the_writers_operations_did() {
    let id = TabId::new();
    let mut watcher = WsMirror::default();

    let tree_tab = TreeTab {
        id,
        name: None,
        tags: Vec::new(),
        root: PaneNode::Leaf { pane: 1 },
        hibernated: false,
    };
    assert!(apply_to_mirror(
        &mut watcher,
        &LayoutDelta::TabCreated {
            at: 0,
            tab: tree_tab,
        },
    ));
    assert!(apply_to_mirror(
        &mut watcher,
        &LayoutDelta::ActiveTabChanged { tab: id },
    ));
    assert!(apply_to_mirror(
        &mut watcher,
        &LayoutDelta::TabRestructured {
            tab: TreeTab {
                id,
                name: None,
                tags: Vec::new(),
                root: PaneNode::Split {
                    axis: TreeAxis::Vertical,
                    ratio: 0.5,
                    a: Box::new(PaneNode::Leaf { pane: 1 }),
                    b: Box::new(PaneNode::Leaf { pane: 2 }),
                },
                hibernated: false,
            },
            pane: None,
        },
    ));
    assert!(apply_to_mirror(
        &mut watcher,
        &LayoutDelta::RatioChanged {
            tab: id,
            path: Vec::new(),
            ratio: 0.7,
        },
    ));

    let mut writer = WsMirror::default();
    diff(&mut writer, &[tab(id, leaf(1))], Some(id), SyncScope::Full, &[]);
    let final_state = vec![tab(id, split(TreeAxis::Vertical, 0.7, leaf(1), leaf(2)))];
    diff(&mut writer, &final_state, Some(id), SyncScope::Full, &[]);

    assert_eq!(watcher, writer);
}

#[test]
fn a_delta_about_a_tab_the_mirror_does_not_hold_reports_itself() {
    let mut mirror = WsMirror::default();
    assert!(!apply_to_mirror(
        &mut mirror,
        &LayoutDelta::TabRenamed {
            tab: TabId::new(),
            name: Some("x".into()),
        },
    ));
    assert!(!apply_to_mirror(
        &mut mirror,
        &LayoutDelta::TabClosed { tab: TabId::new() },
    ));
}

#[test]
fn a_lowered_leaf_carries_its_pane_id_and_its_agent_whatever_live_says() {
    use anterm_core::cli_agent::CLIAgent;
    let tab_id = TabId::new();
    let tag = TagId::new();
    let machine = Machine {
        tabs: vec![TreeTab {
            id: tab_id,
            name: Some("build".into()),
            tags: vec![tag],
            root: PaneNode::Split {
                axis: TreeAxis::Vertical,
                ratio: 0.3,
                a: Box::new(PaneNode::Leaf { pane: 1 }),
                b: Box::new(PaneNode::Leaf { pane: 2 }),
            },
            hibernated: false,
        }],
        active_tab: Some(tab_id),
        panes: vec![
            PaneRecord {
                id: 1,
                cwd: Some("/work".into()),
                live: true,
                ..PaneRecord::new(1)
            },
            PaneRecord {
                id: 2,
                cwd: Some("/work/api".into()),
                live: false,
                agent: Some(AgentFacts {
                    agent: CLIAgent::Claude,
                    session_id: Some("sid".into()),
                    launch_argv: Some(vec!["claude".into()]),
                    status: None,
                }),
                ..PaneRecord::new(2)
            },
        ],
        ..Default::default()
    };

    let session = session_from_tree(&machine);
    assert_eq!(session.tabs.len(), 1);
    assert_eq!(session.active, 0);
    let tab = &session.tabs[0];
    assert_eq!(tab.tree_id, Some(tab_id));
    assert_eq!(tab.name.as_deref(), Some("build"));
    assert_eq!(tab.tags, vec![tag]);
    let SessionPane::Split { ratio, a, b, .. } = &tab.pane else {
        panic!("the split survives the lowering");
    };
    assert!((ratio - 0.3).abs() < 1e-6);
    match &**a {
        SessionPane::Leaf { pane_id, cwd, .. } => {
            assert_eq!(*pane_id, Some(1));
            assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/work")));
        }
        _ => panic!("leaf"),
    }
    match &**b {
        SessionPane::Leaf {
            pane_id,
            cwd,
            agent,
            agent_session_id,
            ..
        } => {
            assert_eq!(*pane_id, Some(2));
            assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/work/api")));
            assert_eq!(*agent, Some(CLIAgent::Claude));
            assert_eq!(agent_session_id.as_deref(), Some("sid"));
        }
        _ => panic!("leaf"),
    }
}

#[test]
fn a_pane_the_tree_calls_dead_still_goes_down_by_its_id() {
    let tab_id = TabId::new();
    let machine = Machine {
        tabs: vec![TreeTab {
            id: tab_id,
            name: None,
            tags: Vec::new(),
            root: PaneNode::Leaf { pane: 7 },
            hibernated: false,
        }],
        active_tab: Some(tab_id),
        panes: vec![PaneRecord {
            id: 7,
            cwd: Some("/work".into()),
            live: false,
            ..PaneRecord::new(7)
        }],
        ..Default::default()
    };

    match &session_from_tree(&machine).tabs[0].pane {
        SessionPane::Leaf { pane_id, .. } => assert_eq!(*pane_id, Some(7)),
        _ => panic!("leaf"),
    }
}

#[test]
fn a_dangling_active_tab_in_the_pulled_tree_falls_back_to_the_first() {
    let machine = Machine {
        tabs: vec![TreeTab {
            id: TabId::new(),
            name: None,
            tags: Vec::new(),
            root: PaneNode::Leaf { pane: 1 },
            hibernated: false,
        }],
        active_tab: Some(TabId::new()),
        ..Default::default()
    };
    assert_eq!(session_from_tree(&machine).active, 0);
}

#[test]
fn a_pane_id_reused_in_another_tab_is_never_read_as_a_replace() {
    let (a, b) = (TabId::new(), TabId::new());
    let mut mirror = WsMirror::default();
    diff(
        &mut mirror,
        &[tab(a, leaf(1)), tab(b, leaf(2))],
        Some(b),
        SyncScope::Full,
        &[],
    );

    let want = vec![tab(a, leaf(2)), tab(b, leaf(2))];
    let ops = diff(&mut mirror, &want, Some(b), SyncScope::Full, &[]);
    assert!(
        !ops.iter()
            .any(|op| matches!(op, ControlRequest::PaneReplace { .. })),
        "got {ops:?}"
    );
}

fn asleep(id: TabId, root: DesiredNode) -> DesiredTab {
    DesiredTab {
        hibernated: true,
        ..tab(id, root)
    }
}

#[test]
fn a_tab_going_to_sleep_says_so_and_nothing_else() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let root = || split(TreeAxis::Vertical, 0.5, leaf(1), leaf(2));
    diff(&mut mirror, &[tab(id, root())], Some(id), SyncScope::Full, &[]);

    let ops = diff(&mut mirror, &[asleep(id, root())], Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![ControlRequest::TabSetHibernated {
            tab: id,
            hibernated: true,
        }]
    );
    assert!(mirror.tabs[0].hibernated);
    assert!(diff(&mut mirror, &[asleep(id, root())], Some(id), SyncScope::Full, &[]).is_empty());
}

#[test]
fn a_waking_tab_clears_its_mark_before_its_panes_are_replaced() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    diff(&mut mirror, &[asleep(id, leaf(1))], Some(id), SyncScope::Full, &[]);

    let want = vec![tab(id, leaf(9))];
    let ops = diff(&mut mirror, &want, Some(id), SyncScope::Full, &[]);
    assert_eq!(
        ops,
        vec![
            ControlRequest::TabSetHibernated {
                tab: id,
                hibernated: false,
            },
            ControlRequest::PaneReplace {
                old: 1,
                new: seed(9),
            },
        ]
    );
    assert!(!mirror.tabs[0].hibernated);
    assert_converged(&mirror, &want);
}

#[test]
fn a_tab_created_asleep_is_marked_as_it_is_created() {
    let id = TabId::new();
    let mut mirror = WsMirror::default();
    let ops = diff(&mut mirror, &[asleep(id, leaf(4))], Some(id), SyncScope::Full, &[]);
    assert!(matches!(ops.first(), Some(ControlRequest::TabCreate { .. })));
    assert!(ops.contains(&ControlRequest::TabSetHibernated {
        tab: id,
        hibernated: true,
    }));
    assert!(mirror.tabs[0].hibernated);
}

#[test]
fn a_sleeping_layout_reads_back_as_the_tree_it_came_from() {
    use crate::core::session::SessionAxis;
    let leaf_of = |pane: u64, cwd: &str| SessionPane::Leaf {
        cwd: Some(std::path::PathBuf::from(cwd)),
        pane_id: Some(pane),
        shell: None,
        ssh_spec: None,
        agent: Some(crate::core::cli_agent::CLIAgent::Claude),
        agent_session_id: Some(format!("sess-{pane}")),
        agent_launch_argv: None,
    };
    let layout = SessionPane::Split {
        axis: SessionAxis::Horizontal,
        ratio: 0.3,
        a: Box::new(leaf_of(3, "/a")),
        b: Box::new(leaf_of(4, "/b")),
    };
    let node = desired_node_from_session(&layout).expect("both leaves have ids");
    assert_eq!(
        node.to_pane_node(),
        PaneNode::Split {
            axis: TreeAxis::Horizontal,
            ratio: 0.3,
            a: Box::new(PaneNode::Leaf { pane: 3 }),
            b: Box::new(PaneNode::Leaf { pane: 4 }),
        }
    );
    let seed = node.seed_of(4).expect("pane 4 is a leaf");
    assert_eq!(seed.cwd.as_deref(), Some("/b"));
    assert_eq!(
        seed.agent.as_ref().and_then(|a| a.session_id.as_deref()),
        Some("sess-4"),
    );

    let unknown = SessionPane::Split {
        axis: SessionAxis::Vertical,
        ratio: 0.5,
        a: Box::new(leaf_of(3, "/a")),
        b: Box::new(SessionPane::Leaf {
            cwd: None,
            pane_id: None,
            shell: None,
            ssh_spec: None,
            agent: None,
            agent_session_id: None,
            agent_launch_argv: None,
        }),
    };
    assert_eq!(
        desired_node_from_session(&unknown).map(|n| n.to_pane_node()),
        Some(PaneNode::Leaf { pane: 3 })
    );
}

#[test]
fn a_restore_brings_a_sleeping_tab_back_asleep_and_named() {
    let (awake, sleeping) = (TabId::new(), TabId::new());
    let machine = Machine {
        tabs: vec![
            TreeTab {
                id: awake,
                name: None,
                tags: Vec::new(),
                root: PaneNode::Leaf { pane: 1 },
                hibernated: false,
            },
            TreeTab {
                id: sleeping,
                name: None,
                tags: Vec::new(),
                root: PaneNode::Leaf { pane: 2 },
                hibernated: true,
            },
        ],
        active_tab: Some(awake),
        panes: vec![
            PaneRecord {
                cwd: Some("/work".into()),
                live: true,
                ..PaneRecord::new(1)
            },
            PaneRecord {
                cwd: Some("/work/api".into()),
                osc_title: Some("fixing the tab".into()),
                live: false,
                ..PaneRecord::new(2)
            },
        ],
        ..Default::default()
    };

    let session = session_from_tree(&machine);
    assert!(!session.tabs[0].hibernated);
    assert!(session.tabs[0].asleep_view.is_none());
    let tab = &session.tabs[1];
    assert!(tab.hibernated);
    match &tab.pane {
        SessionPane::Leaf { pane_id, cwd, .. } => {
            assert_eq!(*pane_id, Some(2));
            assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/work/api")));
        }
        _ => panic!("leaf"),
    }
    let view = tab.asleep_view.as_ref().expect("a sleeping tab is named");
    assert_eq!(view.osc_title.as_deref(), Some("fixing the tab"));
    assert!(!view.live);
}
