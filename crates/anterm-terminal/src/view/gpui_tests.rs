use super::*;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use anterm_ui_kit::config::MouseZoomModifier;
use anterm_core::daemon::protocol::{ClientMsg, DaemonMsg};
use anterm_core::daemon::transport::Stream;
use crate::prompt::completion::CandidateKind;
use crate::remote::PtySource;
use gpui::{Entity, KeyDownEvent, Modifiers, ScrollEvent, TestAppContext, point};

fn harness(cx: &mut TestAppContext) -> (gpui::WindowHandle<TerminalView>, Stream) {
    let pty = if cfg!(windows) {
        PtySource::LocalConpty
    } else {
        PtySource::Raw
    };
    harness_on(cx, pty)
}

fn harness_on(
    cx: &mut TestAppContext,
    pty: PtySource,
) -> (gpui::WindowHandle<TerminalView>, Stream) {
    // Building a view reads the config. Whether that hit the real user
    // directory used to come down to which test happened to pin the
    // scratch dir first.
    anterm_ui_kit::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, daemon_side) = super::test_stream_pair();
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Config::default());
    });
    let window = cx.add_window(|window, cx| {
        let terminal = RemoteTerminal::from_stream_with(
            client_side,
            TermSize::new(80, 24),
            Vec::new(),
            pty,
        )
        .expect("socketpair-backed terminal");
        TerminalView::with_terminal(terminal, 1, window, cx)
    });
    (window, daemon_side)
}

/// The same pane, but hung under a `gpui_kit::component::Root` the way the real
/// window hangs it.
///
/// `harness` makes the view its own root, which is enough for anything
/// that never paints — but gpui-component's text input reaches for `Root`
/// while painting, so any test that lets a frame draw with the search bar
/// (or any other input) on screen needs this one instead.
fn rooted_harness(
    cx: &mut TestAppContext,
) -> (
    gpui::WindowHandle<gpui_kit::component::Root>,
    Entity<TerminalView>,
    Stream,
) {
    anterm_ui_kit::config::pin_test_config_dir();
    cx.executor().allow_parking();
    let (client_side, daemon_side) = super::test_stream_pair();
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Config::default());
    });
    let built: std::rc::Rc<std::cell::RefCell<Option<Entity<TerminalView>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let out = built.clone();
    let window = cx.add_window(move |window, cx| {
        let terminal = RemoteTerminal::from_stream(client_side, TermSize::new(80, 24))
            .expect("socketpair-backed terminal");
        let view = cx.new(|cx| TerminalView::with_terminal(terminal, 1, window, cx));
        *out.borrow_mut() = Some(view.clone());
        gpui_kit::component::Root::new(view, window, cx)
    });
    window
        .update(cx, |_, window, _| window.activate_window())
        .unwrap();
    cx.background_executor.run_until_parked();
    let view = built.borrow_mut().take().expect("the pane was built");
    (window, view, daemon_side)
}

fn prompt_ready(
    window: &gpui::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.at_prompt())
            .unwrap()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the prompt report never reached the view");
}

fn alt_screen_ready(
    window: &gpui::WindowHandle<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    DaemonMsg::Output(b"\x1b[?1049h".to_vec())
        .encode(daemon)
        .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        if window
            .update(cx, |view, _, _| view.on_alt_screen())
            .unwrap()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the alternate-screen switch never reached the grid");
}

#[gpui::test]
fn an_agent_that_has_finished_its_turn_is_not_busy(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Agent(Some(CLIAgent::Claude))
        .encode(&mut daemon)
        .unwrap();

    let report = |status: AgentStatus, daemon: &mut Stream| {
        DaemonMsg::AgentStatus(Some(AgentSessionState {
            status,
            message: None,
            session_id: Some("sid-abc".into()),
            launch_argv: Some(vec!["claude".into()]),
            rich: true,
            cwd: None,
            activity: 0,
            turns: 0,
            inferred: false,
            readout: Default::default(),
            since_ms: None,
        }))
        .encode(daemon)
        .unwrap();
    };
    let settled = |want: Option<PaneBusy>, cx: &mut TestAppContext| {
        for _ in 0..200 {
            if window.update(cx, |view, _, _| view.busy()).unwrap() == want {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    };

    report(AgentStatus::Working, &mut daemon);
    assert!(
        settled(Some(PaneBusy::Agent("Claude Code")), cx),
        "a turn in flight is work closing would cut short"
    );

    // The green Done badge is what sends a reader to close the tab. Asking
    // "Claude Code is still working" there is both false and in the way.
    report(AgentStatus::Done, &mut daemon);
    assert!(
        settled(None, cx),
        "a finished turn must not hold the tab open"
    );
}

#[gpui::test]
fn a_reported_session_id_asks_the_window_to_save(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::{AgentSessionState, AgentStatus};

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    let saves = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    {
        let saves = saves.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, _: &AgentSessionChanged, _| {
                saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .detach();
        });
    }

    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status: AgentStatus::Idle,
        message: None,
        session_id: Some("sid-abc".into()),
        launch_argv: Some(vec!["claude".into()]),
        rich: true,
        cwd: None,
        activity: 0,
        turns: 0,
        inferred: false,
        readout: Default::default(),
        since_ms: None,
    }))
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.agent_session().is_some())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            view.poll_agent_status(false, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        saves.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the id has to reach the layout on file"
    );

    window
        .update(cx, |view, window, cx| {
            view.poll_agent_status(false, window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        saves.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an unchanged session must not re-save on every poll"
    );
}

/// Report a session in `status` on `daemon` and wait for `pane` to see it.
fn report_agent_status(
    status: anterm_core::cli_agent::AgentStatus,
    pane: &gpui::Entity<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    report_agent_turn(status, 0, pane, cx, daemon);
}

/// The same, for a session that has finished `turns` turns so far.
fn report_agent_turn(
    status: anterm_core::cli_agent::AgentStatus,
    turns: u64,
    pane: &gpui::Entity<TerminalView>,
    cx: &mut TestAppContext,
    daemon: &mut Stream,
) {
    use anterm_core::cli_agent::AgentSessionState;

    let state = AgentSessionState {
        status,
        message: None,
        session_id: Some("sid-abc".into()),
        launch_argv: Some(vec!["claude".into()]),
        rich: true,
        cwd: None,
        activity: 0,
        turns,
        inferred: false,
        readout: Default::default(),
        since_ms: None,
    };
    DaemonMsg::AgentStatus(Some(state.clone()))
        .encode(daemon)
        .unwrap();
    for _ in 0..200 {
        if cx.update(|cx| pane.read(cx).terminal.agent_session()) == Some(state.clone()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the agent status never reached the pane");
}

/// Poll `pane`'s agent status inside `window` and read its badge back.
fn poll_unread(
    window: gpui::WindowHandle<TerminalView>,
    pane: &gpui::Entity<TerminalView>,
    cx: &mut TestAppContext,
) -> bool {
    // Through the untyped handle: the typed one leases the root view, and
    // `pane` may be that view.
    cx.update_window(window.into(), |_, window, cx| {
        pane.update(cx, |pane, cx| {
            pane.poll_agent_status(false, window, cx);
            pane.agent_result_unread()
        })
    })
    .unwrap()
}

/// Switching workspaces, or reopening a window from the tray, throws the
/// pane's view away and builds a new one on the same daemon pane (#870).
/// The new view's first look at a `Done` agent is not a turn finishing —
/// the reader watched that one finish before the old view went.
#[gpui::test]
fn a_rebuilt_pane_does_not_re_badge_a_turn_the_reader_already_saw(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &before, cx, &mut before_daemon);
    assert!(
        !poll_unread(window, &before, cx),
        "the reader watched it finish"
    );

    // The same daemon pane, rebuilt the way `tabs_from_session` rebuilds it,
    // with the reader's focus somewhere else.
    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &after, cx, &mut daemon);
    assert!(
        !poll_unread(window, &after, cx),
        "rebuilding the pane is not a turn finishing"
    );
}

/// What the rebuild must not swallow: a turn that was still running when
/// the view went away and finished before the new one arrived.
#[gpui::test]
fn a_turn_that_finished_while_the_pane_was_away_still_badges(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Working, 0, &before, cx, &mut before_daemon);
    assert!(!poll_unread(window, &before, cx));

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &after, cx, &mut daemon);
    assert!(
        poll_unread(window, &after, cx),
        "nobody saw this turn finish"
    );
}

/// Nor a whole later turn: the reader saw turn one, the agent was sent
/// another and finished it while the pane was away. The status reads
/// `Done` both times; only the turn count tells them apart.
#[gpui::test]
fn a_later_turn_that_finished_while_the_pane_was_away_still_badges(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &before, cx, &mut before_daemon);
    assert!(!poll_unread(window, &before, cx));

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 2, &after, cx, &mut daemon);
    assert!(
        poll_unread(window, &after, cx),
        "the second turn finished unseen"
    );
}

/// And a badge the reader had not cleared yet comes back with the pane.
#[gpui::test]
fn an_unread_turn_is_still_unread_after_the_pane_is_rebuilt(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    let (window, mut before_daemon) = harness(cx);
    let before = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    // Building another pane takes the window's focus off `before`.
    let (_elsewhere, _elsewhere_daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(5, window, cx))
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &before, cx, &mut before_daemon);
    assert!(poll_unread(window, &before, cx), "nobody was looking");

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(1, window, cx))
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    report_agent_turn(AgentStatus::Done, 1, &after, cx, &mut daemon);
    assert!(
        poll_unread(window, &after, cx),
        "rebuilding the pane is not reading it"
    );
}

/// The badge answers "did the reader see this?", so it has to read the
/// window's live focus rather than anything a focus callback left behind.
///
/// A second pane built into the same window is the case that goes wrong:
/// it holds no focus and was never told it lost any, so a flag a callback
/// maintains says whatever it was born saying — and the badge lands on
/// exactly the pane nobody is looking at.
#[gpui::test]
fn a_finished_turn_on_an_unfocused_pane_is_unread(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(2, window, cx))
        .unwrap();
    // Building a pane takes the window's focus (`with_terminal`), and the
    // reader's next click hands it back. The new pane is not in the element
    // tree, so nothing delivers it the blur that goes with losing it.
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| {
            assert!(
                !pane.read(cx).focus_handle.is_focused(window),
                "the focus went back to the pane the reader is on"
            );
        })
        .unwrap();

    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(pane.agent_result_unread(), "nobody was looking at the pane");
            });
        })
        .unwrap();
}

/// Reinstalling or restarting the app leaves the daemon — and every agent
/// in it — running, so each restored tab reattaches to a pane whose agent
/// finished its turn long ago. The daemon replays that status, and reading
/// it as a turn that just landed put an unread badge on every agent tab in
/// the window the moment it opened.
#[gpui::test]
fn a_restored_pane_does_not_badge_the_turn_it_reattached_to(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(2, window, cx)
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(
                    !pane.agent_result_unread(),
                    "the replayed status is where this pane starts, not a result that \
                     just arrived"
                );
            });
        })
        .unwrap();

    // And the pane is still armed: the next turn it actually watches finish
    // badges exactly as it would have without the reattach.
    report_agent_status(AgentStatus::Working, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
            });
        })
        .unwrap();
    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(
                    pane.agent_result_unread(),
                    "a turn that finished while the reader was elsewhere is unread"
                );
            });
        })
        .unwrap();
}

/// Build `pane_id`'s first view (unfocused), let it watch `status` at
/// `turns`, then rebuild the pane the way restoring a workspace does — a
/// reattach, whose head is the daemon replaying `replayed` — and read the
/// rebuilt view's badge.
fn rebuild_by_reattach(
    watched: (anterm_core::cli_agent::AgentStatus, u64),
    replayed: (anterm_core::cli_agent::AgentStatus, u64),
    cx: &mut TestAppContext,
) -> bool {
    let (window, _root_daemon) = harness(cx);
    let focus_root = |cx: &mut TestAppContext| {
        window
            .update(cx, |view, window, cx| {
                view.focus_handle.clone().focus(window, cx)
            })
            .unwrap();
        cx.run_until_parked();
    };
    let (before, mut before_daemon) = window
        .update(cx, |_, window, cx| super::quiet_test_pane(2, window, cx))
        .unwrap();
    focus_root(cx);
    report_agent_turn(watched.0, watched.1, &before, cx, &mut before_daemon);
    poll_unread(window, &before, cx);
    drop(before);

    let (after, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(2, window, cx)
        })
        .unwrap();
    focus_root(cx);
    report_agent_turn(replayed.0, replayed.1, &after, cx, &mut daemon);
    poll_unread(window, &after, cx)
}

/// A reattach whose pane this app already watched is a rebuild, not a
/// restart: the read mark, not the replay, says whether the turn is news.
#[gpui::test]
fn a_reattached_pane_takes_back_the_badge_its_reader_left(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;
    assert!(
        rebuild_by_reattach((AgentStatus::Done, 1), (AgentStatus::Done, 1), cx),
        "the reader never cleared that badge; rebuilding the pane is not reading it"
    );
}

#[gpui::test]
fn a_reattached_pane_badges_a_turn_that_was_running_when_its_view_went(
    cx: &mut TestAppContext,
) {
    use anterm_core::cli_agent::AgentStatus;
    assert!(
        rebuild_by_reattach((AgentStatus::Working, 0), (AgentStatus::Done, 1), cx),
        "nobody saw this turn finish"
    );
}

#[gpui::test]
fn a_reattached_pane_badges_a_later_turn_that_finished_while_away(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;
    // Turn one left a mark; turn two finishing before the reattach is a
    // different count, so the mark does not vouch for it.
    assert!(
        rebuild_by_reattach((AgentStatus::Done, 1), (AgentStatus::Done, 2), cx),
        "the second turn finished unseen"
    );
}

/// A relink keeps the view and what it last saw. A turn that was running
/// when the link dropped and finished before it came back reaches the view
/// only as the daemon's replay, and that replay has to badge: nobody saw
/// the turn finish.
#[gpui::test]
fn a_turn_that_finished_while_the_link_was_down_still_badges(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let (pane, mut daemon) = window
        .update(cx, |_, window, cx| {
            super::quiet_reattached_test_pane(2, window, cx)
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    report_agent_status(AgentStatus::Working, &pane, cx, &mut daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(!pane.agent_result_unread(), "the turn is still running");
            });
        })
        .unwrap();

    let (new_client, mut new_daemon) = super::test_stream_pair();
    pane.update(cx, |pane, cx| {
        pane.adopt_relink(
            new_client,
            Vec::new(),
            &crate::PaneRoute::Local,
            TermSize::new(80, 24),
            8,
            17,
            cx,
        )
        .expect("the swap itself cannot fail");
    });
    drop(daemon);

    report_agent_status(AgentStatus::Done, &pane, cx, &mut new_daemon);
    window
        .update(cx, |_, window, cx| {
            pane.update(cx, |pane, cx| {
                pane.poll_agent_status(false, window, cx);
                assert!(
                    pane.agent_result_unread(),
                    "the turn finished while the link was down, so nobody read it"
                );
            });
        })
        .unwrap();
}

#[gpui::test]
fn a_finished_turn_on_the_focused_pane_is_already_read(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::AgentStatus;

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    let pane = window.update(cx, |_, _, cx| cx.entity()).unwrap();
    window
        .update(cx, |view, window, cx| {
            view.focus_handle.clone().focus(window, cx);
        })
        .unwrap();
    cx.run_until_parked();

    report_agent_status(AgentStatus::Done, &pane, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            assert!(view.focus_handle.is_focused(window));
            view.poll_agent_status(false, window, cx);
            assert!(
                !view.agent_result_unread(),
                "the reader watched the turn finish"
            );
        })
        .unwrap();
}

/// An agent that moves into a git worktree does not `chdir` — the process
/// stays put and only its hook stream says where the work went. Every
/// panel that answers "where am I?" reads `effective_cwd`, so this is the
/// one place that has to prefer the agent's answer over the kernel's.
#[gpui::test]
fn a_pane_follows_its_agent_into_a_worktree(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::{AgentSessionState, AgentStatus};
    use std::io::Write as _;
    use std::path::PathBuf;

    let launched_in = PathBuf::from("/repo");
    let working_in = PathBuf::from("/repo/.claude/worktrees/wt");

    let (window, mut daemon) = harness(cx);
    DaemonMsg::Cwd(launched_in.clone())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status: AgentStatus::Working,
        message: None,
        session_id: Some("sid-wt".into()),
        launch_argv: Some(vec!["claude".into()]),
        rich: true,
        cwd: Some(working_in.clone()),
        activity: 0,
        turns: 0,
        inferred: false,
        readout: Default::default(),
        since_ms: None,
    }))
    .encode(&mut daemon)
    .unwrap();
    daemon.flush().unwrap();
    for _ in 0..200 {
        let seen = window
            .update(cx, |view, _, _| {
                view.cwd().is_some() && view.agent_session().is_some()
            })
            .unwrap();
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.cwd(),
                Some(launched_in.clone()),
                "the process really is still in the launch directory"
            );
            view.poll_foreground(window, cx);
            assert_eq!(
                view.effective_cwd(),
                Some(working_in.clone()),
                "the file tree, the cwd row and the SCM panel all root here"
            );
            assert_eq!(view.effective_host_cwd(), Some(working_in.clone()));
        })
        .unwrap();

    // Turn over: the agent is gone, and with it any claim about where the
    // work is. Falling back to a stale worktree would be worse than the
    // bug this fixes.
    DaemonMsg::AgentStatus(None).encode(&mut daemon).unwrap();
    daemon.flush().unwrap();
    for _ in 0..200 {
        let gone = window
            .update(cx, |view, _, _| view.agent_session().is_none())
            .unwrap();
        if gone {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, window, cx| {
            view.poll_foreground(window, cx);
            assert_eq!(view.effective_cwd(), Some(launched_in.clone()));
        })
        .unwrap();
}

/// A 1x1 red placement anchored at an absolute scrollback row, built the way
/// the decode worker hands one to the store.
fn placed_at(anchor_row: i64) -> crate::render::images::PlacedImage {
    use anterm_core::kitty_graphics::{Image, WireFormat};

    let mut img = Image {
        id: 1,
        number: 0,
        placement: 0,
        width: 1,
        height: 1,
        cols: 0,
        rows: 0,
        data: vec![0xff, 0x00, 0x00, 0xff],
        format: WireFormat::Rgba,
        compressed: false,
    };
    let (data, width_px, height_px) = crate::render::images::decode(&mut img).unwrap();
    crate::render::images::PlacedImage {
        data,
        anchor_row,
        anchor_col: 0,
        width_px,
        height_px,
        cols: 0,
        rows: 0,
        id: 1,
        placement: 0,
        painted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

#[gpui::test]
fn clearing_the_scrollback_drops_what_was_anchored_in_it(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    // Overflow the 24-row viewport so there is a scrollback to purge.
    let mut out = Vec::new();
    for i in 0..60 {
        out.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    DaemonMsg::Output(out).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        let filled = window
            .update(cx, |view, _, _| {
                view.terminal.term.lock().grid().history_size() > 0
            })
            .unwrap();
        if filled {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            let history = view.terminal.term.lock().grid().history_size();
            assert!(history > 0, "the test needs a scrollback to clear");

            // Both of these address rows `clear_history` is about to drop:
            // an image anchored by absolute row, and a selection reaching up
            // into the history.
            view.terminal.images().place(placed_at(history as i64));
            view.terminal.term.lock().selection = Some(Selection::new(
                SelectionType::Simple,
                Point::new(Line(-1), Column(0)),
                Side::Left,
            ));

            view.clear_scrollback(cx);

            assert!(
                view.terminal.images().snapshot().is_empty(),
                "a stale anchor blits the frame over live output, or off-screen \
                 entirely — the daemon never replays the frame to correct it"
            );
            assert!(
                view.terminal.term.lock().selection.is_none(),
                "a selection left pointing at purged rows clamps onto the \
                 viewport and copies whatever text moved into them"
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_scrollbar_moves_the_viewport_and_follows_it_back(cx: &mut TestAppContext) {
    use gpui_kit::component::scroll::ScrollbarHandle as _;

    let (window, mut daemon) = harness(cx);

    // Overflow the 24-row viewport so there is a scrollback to scroll.
    let mut out = Vec::new();
    for i in 0..60 {
        out.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    DaemonMsg::Output(out).encode(&mut daemon).unwrap();
    // Wait for the reader to go quiet, not just to start: a scrollback
    // still filling underneath would move every row this test names.
    let mut settled = 0;
    for _ in 0..200 {
        let now = window
            .update(cx, |view, _, _| {
                view.terminal.term.lock().grid().history_size()
            })
            .unwrap();
        if now > 0 && now == settled {
            break;
        }
        settled = now;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, _| {
            view.sync_scrollbar();
            let history = view.terminal.term.lock().grid().history_size();
            assert!(history > 0, "the test needs a scrollback to scroll");
            let row = view.render.line_height.as_f32();
            assert_eq!(
                view.pointer.scroll_handle.offset().y,
                px(-(history as f32) * row),
                "at the live edge the whole scrollback sits above the viewport"
            );

            // Drag the thumb a third of the way up its track. The bar only
            // records the row; the pane applies it on its next render.
            view.pointer.scroll_frac = 0.5;
            view.pointer.scroll_handle
                .set_offset(point(px(0.), px(-(history as f32) * row / 3.)));
            assert_eq!(
                view.terminal.term.lock().grid().display_offset(),
                0,
                "the bar does not reach into the terminal itself"
            );

            view.sync_scrollbar();
            let offset = view.terminal.term.lock().grid().display_offset();
            assert_eq!(
                offset,
                history - (history as f32 / 3.).round() as usize,
                "the viewport lands on the row the thumb was dropped on"
            );
            assert_eq!(
                view.pointer.scroll_frac, 0.,
                "a sub-line remainder left over from the wheel would paint \
                 the grid off the row the thumb picked"
            );
            assert_eq!(
                view.pointer.scroll_handle.offset().y,
                px(-((history - offset) as f32) * row),
                "and the thumb reports the row the grid actually reached"
            );
        })
        .unwrap();
}

/// The case from the bug report: a relative path printed by a tool, sitting
/// in a pane that reported its directory. It resolves; a name that is not
/// there does not, and comes back as an unresolved *candidate* so the click
/// can say why rather than doing nothing at all.
#[gpui::test]
fn a_relative_path_links_against_the_panes_own_directory(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("anterm-view-link-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("scratchpad")).expect("create scratchpad dir");
    let notes = dir.join("scratchpad/notes.md");
    std::fs::write(&notes, b"# notes").expect("create notes.md");

    let (window, mut daemon) = harness(cx);
    DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
    DaemonMsg::Output(b"ready (scratchpad/notes.md) and (scratchpad/gone.md)\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        let seen = window
            .update(cx, |view, _, _| {
                view.cwd().is_some()
                    && view.terminal.term.lock().grid()[Line(0)][Column(7)].c == 's'
            })
            .unwrap();
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.cwd().as_deref(), Some(dir.as_path()));
            assert!(
                view.hover_link_at(7, 0, true, cx),
                "the file is right there under the pane's directory"
            );
            assert!(
                view.links.hovered_link.as_ref().is_some_and(|link| link.armed),
                "with the modifier down it is ready to be clicked"
            );
            assert!(
                view.hover_link_at(7, 0, false, cx),
                "a path is pointed out before the modifier is down, not after"
            );
            assert!(
                view.links.hovered_link.as_ref().is_some_and(|link| !link.armed),
                "but held back, because a plain click will not follow it"
            );

            let gone = "ready (scratchpad/notes.md) and (".len();
            assert!(
                !view.hover_link_at(gone, 0, true, cx),
                "nothing underlines for a name that is not on disk"
            );
            match view.resolve_link_at(gone, 0, true, false, cx) {
                LinkAt::Unresolved { candidate, pending } => {
                    assert_eq!(candidate.path, "scratchpad/gone.md");
                    assert!(
                        candidate.looks_like_a_path(view.link_path_style()),
                        "so the click reports it instead of staying silent"
                    );
                    assert!(!pending, "a local pane answers on the spot");
                }
                _ => panic!("expected an unresolved path-shaped candidate"),
            }

            view.record_menu_link(7, 0, cx);
            assert!(
                matches!(
                    view.menu_link_path(),
                    Some(path) if path.ends_with("scratchpad/notes.md")
                ),
                "a right click over a path opens a menu about that path"
            );
            view.record_menu_link(0, 0, cx);
            assert!(
                view.menu_link_path().is_none(),
                "and a right click over `ready` opens the ordinary one"
            );

            let mut off = cx.global::<Config>().clone();
            off.link_url = false;
            cx.set_global(off);
            view.record_menu_link(7, 0, cx);
            assert!(
                view.menu_link_path().is_none(),
                "and with link detection turned off the menu offers nothing \
                 the underline and the click both refuse"
            );
            cx.set_global(Config::default());

            // `ready (scratchpad...`: the blank between the two words.
            assert!(!view.hover_link_at(5, 0, false, cx));
            assert!(view.links.hovered_link.is_none(), "a blank holds no link");
            assert_eq!(
                view.links.last_hover_cell,
                Some((5, 0)),
                "and the pointer is still remembered, so crossing a run \
                 of blanks costs one look each rather than one a frame"
            );
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

fn seed_history(view: &mut TerminalView, entries: &[&str], cx: &mut Context<TerminalView>) {
    let mut snap = crate::prompt::history::ScopeSnapshot::new();
    snap.entries = entries.iter().map(|s| s.to_string()).collect();
    cx.default_global::<HistoryStore>()
        .replace_scope_for_test(&view.prompt.history_scope, snap);
    view.invalidate_ghost_cache();
}

fn selected_reverse_search_line(view: &TerminalView, cx: &mut App) -> Option<String> {
    let rs = view.prompt.reverse_search.as_ref()?;
    let store = cx.default_global::<HistoryStore>();
    let snap = view.reverse_search_snapshot(rs.filter(), store);
    rs.selected_line(&snap.entries).map(str::to_string)
}

/// Coming back from an `ssh` session left the pane with no history at all:
/// the scope switch cleared the list, and the reload that refills it is a
/// background task, so ↑ recalled nothing until that landed (#817).
#[gpui::test]
fn a_pane_back_from_ssh_still_recalls_its_local_history(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["cargo build", "ssh box"], cx);
        })
        .unwrap();

    let away = anterm_core::daemon::protocol::RemoteContext {
        kind: anterm_core::daemon::protocol::RemoteKind::Ssh,
        argv: vec!["ssh".into(), "box".into()],
        target: "box".into(),
    };
    DaemonMsg::RemoteContext(Some(away))
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.remote_context().is_some())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, _, cx| view.follow_history_scope(cx))
        .unwrap();

    DaemonMsg::RemoteContext(None).encode(&mut daemon).unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.remote_context().is_none())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            view.follow_history_scope(cx);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "ssh box",
                "↑ right after the ssh session ended recalled nothing"
            );
        })
        .unwrap();
}

/// A seed that lands after the user already ran something must not take
/// that command back off the list: it stays, as the most recent entry.
#[gpui::test]
fn a_late_history_load_keeps_what_was_run_meanwhile(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["make test", "git push"], cx);
            let mut loaded = crate::prompt::history::ScopeSnapshot::new();
            loaded.entries = vec!["ls".to_string(), "make test".to_string()];
            view.install_loaded_history(loaded, cx);
            let store = cx.default_global::<HistoryStore>();
            assert_eq!(
                store.snapshot(&view.prompt.history_scope).entries,
                ["ls", "make test", "git push"]
            );
            assert!(store.is_scope_ready(&view.prompt.history_scope));
        })
        .unwrap();
}

/// The cache above must not hand a scope someone else's list: stepping into
/// an `ssh` session still starts from nothing until the far end's own
/// history is read.
#[gpui::test]
fn a_pane_going_out_to_ssh_does_not_inherit_the_local_history(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["rm -rf ./build"], cx);
        })
        .unwrap();

    DaemonMsg::RemoteContext(Some(anterm_core::daemon::protocol::RemoteContext {
        kind: anterm_core::daemon::protocol::RemoteKind::Ssh,
        argv: vec!["ssh".into(), "box".into()],
        target: "box".into(),
    }))
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.remote_context().is_some())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            view.follow_history_scope(cx);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "",
                "a local command was recalled onto a remote prompt"
            );
        })
        .unwrap();
}

/// Absolute paths in a pane that is `ssh`-ed somewhere used to be resolved
/// against *this* machine's filesystem, so `/etc/hosts` on the far end
/// opened the local copy without a word about it.
#[gpui::test]
fn a_pane_whose_paths_are_elsewhere_does_not_link_local_files(cx: &mut TestAppContext) {
    let file = std::env::temp_dir().join(format!("anterm-elsewhere-{}.txt", std::process::id()));
    std::fs::write(&file, b"local").expect("create local file");
    let line = format!("open {} now\r\n", file.display());

    let (window, mut daemon) = harness(cx);
    DaemonMsg::Output(line.clone().into_bytes())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::RemoteContext(Some(anterm_core::daemon::protocol::RemoteContext {
        kind: anterm_core::daemon::protocol::RemoteKind::Ssh,
        argv: vec!["ssh".into(), "box".into()],
        target: "box".into(),
    }))
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        let seen = window
            .update(cx, |view, _, _| view.remote_context().is_some())
            .unwrap();
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            assert!(
                !view.cwd_is_on_host(),
                "a local host cannot answer for the far side of an ssh session"
            );
            assert!(
                !view.hover_link_at(6, 0, true, cx),
                "the local copy of that path is not what the pane printed"
            );
        })
        .unwrap();
    let _ = std::fs::remove_file(&file);
}

/// A path a remote pane printed stays *wanted* until a host has actually
/// been asked about it.
///
/// `take_wanted` moves paths into the in-flight set on the promise that a
/// call is carrying them; a lookup that finds no host to make that call
/// must not have made the promise. Otherwise those paths sit "not answered
/// yet" for the life of the pane — no underline, and a click that says
/// nothing, which is the silence this whole path exists to remove.
///
/// Was unix-only because the path it prints is: `Path::new("/etc/hosts")`
/// is not absolute on Windows, so `FileCandidate::paths` measured it from
/// the roots rather than letting it stand alone — and a workspace that
/// never connected has no roots, so nothing was ever wanted. Which was
/// itself the divergence: a Windows anterm looking at a *remote* Linux pane
/// never probed the POSIX paths that pane printed.
///
/// #795 settled that. `paths` now asks the pane's own
/// [`super::search::PathStyle`] rather than this machine's, and a remote
/// pane that has not reported a cwd is read as `Posix`, so `/etc/hosts`
/// stands alone on every client. The gate is only still here because
/// nothing has run this test on Windows yet; lifting it belongs in a
/// change that can show it green, not in a merge.
#[gpui::test]
fn a_stale_hover_row_does_not_index_the_shrunken_grid(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.hover_link_at(0, 23, true, cx);
            view.terminal.resize(TermSize::new(80, 8), 8, 17);
            view.links.last_hover_cell = Some((0, 23));
            assert!(
                !view.refresh_link_hover(true, cx),
                "a row outside the grid can't hold a link"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_resize_forgets_the_hovered_cell(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.set_grid_size(80, 24, px(8.), px(17.), 1., cx);
            view.hover_link_at(0, 23, true, cx);
            assert_eq!(view.links.last_hover_cell, Some((0, 23)));
            view.links.hovered_link = Some(HoveredLink {
                runs: vec![(
                    Point::new(Line(23), Column(0)),
                    Point::new(Line(23), Column(3)),
                )],
                armed: true,
            });
            view.set_grid_size(80, 24, px(8.), px(17.), 1., cx);
            assert_eq!(view.links.last_hover_cell, Some((0, 23)));
            view.set_grid_size(80, 8, px(8.), px(17.), 1., cx);
            assert!(view.links.last_hover_cell.is_none(), "the cell is stale");
            assert!(view.links.hovered_link.is_none(), "so is the link it resolved");
        })
        .unwrap();
}

/// The seam is invisible to the user, so it has to be invisible to the
/// hover too: pointing at either half underlines the whole path, and the
/// span the element paints reaches across both rows.
#[gpui::test]
fn a_path_the_terminal_wrapped_is_hovered_as_one_link(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("anterm-view-wrap-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("a/bb/ccc/dddd")).expect("create dirs");
    std::fs::write(dir.join("a/bb/ccc/dddd/notes.md"), b"# notes").expect("create notes.md");

    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.set_grid_size(20, 6, px(8.), px(17.), 1., cx);
        })
        .unwrap();
    DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
    DaemonMsg::Output(b"see a/bb/ccc/dddd/notes.md here\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        let seen = window
            .update(cx, |view, _, _| {
                view.cwd().is_some()
                    && view.terminal.term.lock().grid()[Line(1)][Column(0)].c == 'e'
            })
            .unwrap();
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            // Row 0 holds `see a/bb/ccc/dddd/n`, row 1 the rest.
            for (col, row, where_) in [(6, 0, "before the seam"), (2, 1, "after it")] {
                assert!(
                    view.hover_link_at(col, row, true, cx),
                    "the wrapped path is a link from {where_}"
                );
                let link = view.links.hovered_link.as_ref().expect("a span");
                let (start, end) = (link.runs[0].0, link.runs[link.runs.len() - 1].1);
                assert_eq!(
                    (start.line, end.line),
                    (Line(0), Line(1)),
                    "and the span the element paints covers both rows"
                );
                assert_eq!(start.column, Column(4), "starting at the path itself");
            }
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A wide character owns two columns, and the second one holds a space.
/// The hover has to read that as part of the glyph, or the underline goes
/// out on every other column of a path written in CJK.
#[gpui::test]
fn the_second_column_of_a_wide_character_still_hovers(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("anterm-view-wide-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("文档")).expect("create dirs");
    std::fs::write(dir.join("文档/笔记.md"), b"# notes").expect("create notes");

    let (window, mut daemon) = harness(cx);
    DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
    DaemonMsg::Output("see 文档/笔记.md here\r\n".as_bytes().to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        let seen = window
            .update(cx, |view, _, _| {
                view.cwd().is_some()
                    && view.terminal.term.lock().grid()[Line(0)][Column(4)].c == '文'
            })
            .unwrap();
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            // `see 文档/…`: column 4 carries 文, column 5 is its spacer.
            assert!(!view.cell_is_blank(5, 0), "the spacer belongs to the glyph");
            for col in [4, 5] {
                assert!(
                    view.hover_link_at(col, 0, true, cx),
                    "column {col} of the same character is the same link"
                );
            }
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A full-screen application drew the grid and is watching the mouse
/// itself, so anterm stays out of it until asked.
#[gpui::test]
fn a_path_under_a_full_screen_application_waits_for_the_modifier(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("anterm-view-alt-link-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("scratchpad")).expect("create scratchpad dir");
    std::fs::write(dir.join("scratchpad/notes.md"), b"# notes").expect("create notes.md");

    let (window, mut daemon) = harness(cx);
    DaemonMsg::Cwd(dir.clone()).encode(&mut daemon).unwrap();
    DaemonMsg::Output(b"\x1b[?1049hready scratchpad/notes.md\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        let seen = window
            .update(cx, |view, _, _| {
                view.cwd().is_some()
                    && view.on_alt_screen()
                    && view.terminal.term.lock().grid()[Line(0)][Column(6)].c == 's'
            })
            .unwrap();
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            assert!(view.on_alt_screen(), "the application took the grid");
            assert!(
                !view.hover_link_at(6, 0, false, cx),
                "nothing is pointed out over somebody else's window"
            );
            assert!(
                view.hover_link_at(6, 0, true, cx),
                "asking for anterm's reading of the screen still gets it"
            );
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_link_menu_is_headed_by_the_file_it_is_about() {
    assert_eq!(
        link_menu_label(std::path::Path::new("/a/very/long/way/down/notes.md")),
        "notes.md",
        "the name, so the menu is not as wide as the path"
    );
    assert_eq!(link_menu_label(std::path::Path::new("/")), "/");
}

/// Runs out the wait a new title is held for.
fn settle(cx: &mut TestAppContext) {
    cx.executor().advance_clock(super::TITLE_SETTLE * 2);
    cx.run_until_parked();
}

#[gpui::test]
fn title_events_drive_the_tab_title(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.session.title, "anterm");
            view.handle_event(AlacEvent::Title("vim — main.rs".into()), cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.session.title, "vim — main.rs");
            view.handle_event(AlacEvent::ResetTitle, cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| assert_eq!(view.session.title, "anterm"))
        .unwrap();
}

/// The flash this wait exists to stop: a prompt framework names the command
/// it is about to run and puts the directory back at the next prompt, and
/// for anything that finishes in a blink both edges arrive within a few
/// frames. The tab used to show the command and snap back, which reads as a
/// glitch rather than as information.
#[gpui::test]
fn a_command_over_before_the_wait_never_reaches_the_tab(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.session.default_title = "~/dev".into();
            view.session.title = "~/dev".into();
            view.handle_event(AlacEvent::Title("ls".into()), cx);
            view.handle_event(AlacEvent::Title("~/dev".into()), cx);
            assert_eq!(view.session.title, "~/dev", "and never flashed on the way");
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.session.title, "~/dev",
                "the command was over before the tab could adopt its name"
            );
        })
        .unwrap();
}

/// A program that rewrites its own title faster than the wait — a download
/// reporting progress — still updates the tab. The wait already running
/// adopts the newest title rather than starting over, which is what would
/// leave the tab frozen on the directory for as long as the download ran.
#[gpui::test]
fn a_title_rewritten_faster_than_the_wait_still_lands(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.handle_event(AlacEvent::Title("wget 1%".into()), cx);
            view.handle_event(AlacEvent::Title("wget 40%".into()), cx);
            view.handle_event(AlacEvent::Title("wget 99%".into()), cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| assert_eq!(view.session.title, "wget 99%"))
        .unwrap();
}

/// An SSH pane answers to the host it dialled, and keeps answering to it
/// after the remote program hands the title back (#438). Before this every
/// SSH tab in the window read "anterm" until — and only if — the far shell
/// had integration enough to title itself.
#[gpui::test]
fn an_ssh_pane_is_named_after_its_host(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.session.default_title = "prod-web".into();
            view.session.title = "prod-web".into();
            view.handle_event(AlacEvent::Title("vim — main.rs".into()), cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.session.title, "vim — main.rs");
            view.handle_event(AlacEvent::ResetTitle, cx);
        })
        .unwrap();
    settle(cx);
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.session.title, "prod-web",
                "a reset goes back to the host, not to the app's own name"
            );
        })
        .unwrap();
}

fn next_input(daemon: &mut Stream) -> Vec<u8> {
    loop {
        match ClientMsg::read(daemon).expect("client socket stays open") {
            ClientMsg::Input(bytes) => return bytes,
            _ => continue,
        }
    }
}

fn type_char(
    view: &mut TerminalView,
    ch: &str,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    if cfg!(target_os = "macos") {
        let _ = window;
        view.commit_text(ch, cx);
    } else {
        let ev = KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers::default(),
                key: ch.to_string(),
                key_char: Some(ch.to_string()),
            },
            is_held: false,
            prefer_character_input: false,
        };
        view.on_key_down(&ev, window, cx);
    }
}

fn next_input_until_timeout(daemon: &mut Stream) -> Option<Vec<u8>> {
    use std::io::ErrorKind;

    daemon
        .set_read_timeout(Some(std::time::Duration::from_millis(250)))
        .unwrap();
    loop {
        match ClientMsg::read(daemon) {
            Ok(ClientMsg::Input(bytes)) => return Some(bytes),
            Ok(_) => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return None;
            }
            Err(e) => panic!("client socket failed before Input: {e}"),
        }
    }
}

/// `next_input_until_timeout` with the executor run between reads: a reply
/// crosses the reader thread and the view before it is written.
fn pumped_input(cx: &mut TestAppContext, daemon: &mut Stream, tries: usize) -> Option<String> {
    (0..tries)
        .find_map(|_| {
            cx.run_until_parked();
            next_input_until_timeout(daemon)
        })
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

#[gpui::test]
fn a_theme_change_reaches_only_the_panes_that_asked(cx: &mut TestAppContext) {
    let set_background = |cx: &mut TestAppContext, bg: gpui::Hsla| {
        cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).background = bg)
    };
    let (_asked_window, mut asked) = harness(cx);
    let (_plain_window, mut plain) = harness(cx);
    set_background(cx, gpui::white());

    DaemonMsg::Output(b"\x1b[?2031h\x1b[?996n".to_vec())
        .encode(&mut asked)
        .unwrap();
    assert_eq!(
        pumped_input(cx, &mut asked, 20).as_deref(),
        Some("\x1b[?997;2n"),
        "996 answers the current scheme"
    );

    set_background(cx, gpui::black());
    assert_eq!(
        pumped_input(cx, &mut asked, 20).as_deref(),
        Some("\x1b[?997;1n")
    );
    DaemonMsg::Output(b"\x1b]11;?\x07".to_vec())
        .encode(&mut asked)
        .unwrap();
    assert!(
        pumped_input(cx, &mut asked, 20)
            .is_some_and(|reply| reply.contains("11;rgb:0000/0000/0000")),
        "OSC 11 answers with the background the flip left"
    );

    set_background(cx, gpui::white());
    assert_eq!(
        pumped_input(cx, &mut asked, 20).as_deref(),
        Some("\x1b[?997;2n")
    );
    assert_eq!(
        pumped_input(cx, &mut plain, 2),
        None,
        "a pane that never set 2031 is not written to"
    );

    DaemonMsg::Output(b"\x1b[?2031l".to_vec())
        .encode(&mut asked)
        .unwrap();
    assert_eq!(pumped_input(cx, &mut asked, 1), None);
    set_background(cx, gpui::black());
    assert_eq!(pumped_input(cx, &mut asked, 2), None, "2031 switched off");
}

#[gpui::test]
fn a_reattach_that_replays_2031_reports_the_scheme_once(cx: &mut TestAppContext) {
    let (_window, mut daemon) = harness(cx);
    cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).background = gpui::black());
    // What a replay sends: the modes ahead of the ring, then each ring
    // segment behind its size, then the shell's state. Nothing live
    // follows: an idle pane still hears once.
    let size = anterm_core::daemon::protocol::WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
    };
    let mut replay = Vec::new();
    for frame in [
        DaemonMsg::Snapshot(b"\x1b[?2031h".to_vec()),
        DaemonMsg::Size(size),
        DaemonMsg::Snapshot(b"\x1b[?996n".to_vec()),
        DaemonMsg::Snapshot(b"$ ".to_vec()),
        DaemonMsg::Prompt {
            active: true,
            at_prompt: false,
            last_exit: None,
        },
    ] {
        frame.encode(&mut replay).unwrap();
    }
    std::io::Write::write_all(&mut daemon, &replay).unwrap();
    assert_eq!(
        pumped_input(cx, &mut daemon, 20).as_deref(),
        Some("\x1b[?997;1n"),
        "the theme may have flipped while detached"
    );
    assert_eq!(
        pumped_input(cx, &mut daemon, 2),
        None,
        "one report for the whole replay, and the replayed 996 is not answered"
    );
}

/// A program that switched 2031 on and died without `?2031l` leaves a
/// replay whose last word is still `?2031h`. The shell that owns the pane
/// now never asked, so neither a reattach nor a theme flip may type a
/// report into its command line.
#[gpui::test]
fn a_dead_programs_2031_never_reaches_the_shell(cx: &mut TestAppContext) {
    let set_background = |cx: &mut TestAppContext, bg: gpui::Hsla| {
        cx.update(|cx| gpui_kit::component::Theme::global_mut(cx).background = bg)
    };
    let dead = b"\x1b]133;C;claude\x07\x1b[?2031hclaude output\r\n";
    let prompt = b"\x1b]133;D;137\x07\x1b]133;A\x07$ \x1b]133;B\x07";
    let restored = anterm_core::daemon::pane::restore_preamble(Some("this shell is new"));

    for tail in [&prompt[..], &restored[..]] {
        let (_window, mut daemon) = harness(cx);
        set_background(cx, gpui::white());
        let mut replay = Vec::new();
        for frame in [
            DaemonMsg::Snapshot(dead.to_vec()),
            DaemonMsg::Snapshot(tail.to_vec()),
        ] {
            frame.encode(&mut replay).unwrap();
        }
        std::io::Write::write_all(&mut daemon, &replay).unwrap();
        assert_eq!(
            pumped_input(cx, &mut daemon, 2),
            None,
            "a reattach reports nothing"
        );
        set_background(cx, gpui::black());
        assert_eq!(pumped_input(cx, &mut daemon, 2), None, "nor does a flip");
    }

    // A ring takes many reads. One that ends after the segment holding
    // `?2031h` and before the one holding the prompt is not the end of
    // the replay.
    let (_window, mut daemon) = harness(cx);
    set_background(cx, gpui::white());
    let size = anterm_core::daemon::protocol::WinSize {
        cols: 80,
        rows: 24,
        cell_w: 8,
        cell_h: 16,
    };
    let mut head = Vec::new();
    DaemonMsg::Size(size).encode(&mut head).unwrap();
    DaemonMsg::Snapshot(dead.to_vec())
        .encode(&mut head)
        .unwrap();
    std::io::Write::write_all(&mut daemon, &head).unwrap();
    assert_eq!(
        pumped_input(cx, &mut daemon, 2),
        None,
        "half a replay reports nothing"
    );
    let mut rest = Vec::new();
    DaemonMsg::Size(size).encode(&mut rest).unwrap();
    DaemonMsg::Snapshot(prompt.to_vec())
        .encode(&mut rest)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(137),
    }
    .encode(&mut rest)
    .unwrap();
    std::io::Write::write_all(&mut daemon, &rest).unwrap();
    assert_eq!(pumped_input(cx, &mut daemon, 2), None, "nor does the rest");

    // Live, the same: the program's exit and the next prompt end it.
    let (_window, mut live) = harness(cx);
    set_background(cx, gpui::white());
    DaemonMsg::Output(dead.to_vec()).encode(&mut live).unwrap();
    assert_eq!(pumped_input(cx, &mut live, 1), None);
    set_background(cx, gpui::black());
    assert_eq!(
        pumped_input(cx, &mut live, 20).as_deref(),
        Some("\x1b[?997;1n")
    );
    DaemonMsg::Output(prompt.to_vec())
        .encode(&mut live)
        .unwrap();
    assert_eq!(pumped_input(cx, &mut live, 1), None);
    set_background(cx, gpui::white());
    assert_eq!(
        pumped_input(cx, &mut live, 2),
        None,
        "a shell at its prompt hears nothing"
    );
    DaemonMsg::Output(b"\x1b[?996n".to_vec())
        .encode(&mut live)
        .unwrap();
    assert_eq!(
        pumped_input(cx, &mut live, 20).as_deref(),
        Some("\x1b[?997;2n"),
        "a live 996 is still answered"
    );
}

#[gpui::test]
fn allowed_remote_clipboard_image_reaches_the_system_clipboard(cx: &mut TestAppContext) {
    use gpui::ClipboardEntry;

    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            let mut spec: anterm_core::daemon::protocol::NativeSshSpec = serde_json::from_str(
                r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
            )
            .unwrap();
            spec.remote_clipboard_write = true;
            view.session.ssh_spec = Some(Box::new(spec));
        })
        .unwrap();

    let pixel = image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixel)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let write = anterm_core::clipboard::ClipboardWrite {
        mime: "image/png".into(),
        data: png.clone(),
        id: Some("copy-1".into()),
    };
    DaemonMsg::ClipboardWrite(write.encode_frame())
        .encode(&mut daemon)
        .unwrap();

    for _ in 0..400 {
        cx.run_until_parked();
        let copied = cx.update(|cx| {
            cx.read_from_clipboard().and_then(|item| {
                item.entries().iter().find_map(|entry| match entry {
                    ClipboardEntry::Image(image) => Some(image.bytes.clone()),
                    _ => None,
                })
            })
        });
        if copied.as_deref() == Some(png.as_slice()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let copied = cx.update(|cx| cx.read_from_clipboard());
    assert!(copied.is_some_and(|item| {
        item.entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(image) if image.bytes == png))
    }));
    assert_eq!(
        next_input(&mut daemon),
        anterm_core::clipboard::response(Some("copy-1"), "DONE")
    );
}

#[gpui::test]
fn disabled_remote_clipboard_image_is_rejected_without_overwriting(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            view.session.ssh_spec = Some(Box::new(
                serde_json::from_str(
                    r#"{"host":"build-box","port":22,"user":"me","auth_mode":"auto"}"#,
                )
                .unwrap(),
            ));
        })
        .unwrap();
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("keep me".into())));

    let write = anterm_core::clipboard::ClipboardWrite {
        mime: "image/png".into(),
        data: vec![1, 2, 3],
        id: Some("copy-2".into()),
    };
    DaemonMsg::ClipboardWrite(write.encode_frame())
        .encode(&mut daemon)
        .unwrap();
    let mut reply = None;
    for _ in 0..100 {
        cx.run_until_parked();
        reply = next_input_until_timeout(&mut daemon);
        if reply.is_some() {
            break;
        }
    }

    assert_eq!(
        reply,
        Some(anterm_core::clipboard::response(
            Some("copy-2"),
            "EPERM"
        ))
    );
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("keep me")
    );
}

#[gpui::test]
fn ctrl_l_at_prompt_reaches_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let ctrl_l = gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                key: "l".to_string(),
                key_char: None,
            };
            view.handle_editor_key(&ctrl_l, cx);
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x0c]));
}

#[gpui::test]
fn passthrough_ctrl_c_discards_typeahead_before_the_shell_can_resume(cx: &mut TestAppContext) {
    assert_foreground_interrupt_does_not_wipe_prompt(cx, "agent input", "ctrl-c", 0x03);
}

#[gpui::test]
fn passthrough_ctrl_d_discards_typeahead_before_the_shell_can_resume(cx: &mut TestAppContext) {
    // Ctrl-D only ends input on an empty line; with text it is an edit.
    assert_foreground_interrupt_does_not_wipe_prompt(cx, "", "ctrl-d", 0x04);
}

fn assert_foreground_interrupt_does_not_wipe_prompt(
    cx: &mut TestAppContext,
    pending: &str,
    chord: &str,
    byte: u8,
) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground process owns input");
            view.prompt.typeahead.observe(RawInput::Text(pending), false);
            view.prompt.typeahead.observe(
                RawInput::Key {
                    key: "up",
                    plain: true,
                },
                false,
            );

            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key(chord),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
            // Exercise the consumers without draining their input first.
            view.adopt_typeahead();
            view.flush_typeahead();
            assert!(view.prompt.cmd.text().is_empty());
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![byte]));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "resuming the shell must not synthesize Ctrl-U after {chord}"
    );
}

#[gpui::test]
fn submitted_exit_typeahead_does_not_wipe_the_returned_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            // Ordinary SSH need not take the alternate screen or identify
            // as an agent. Its input reaches the passthrough recorder.
            assert!(!view.input_active());
            for ch in ["e", "x", "i", "t"] {
                type_char(view, ch, window, cx);
            }
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("enter"),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();
    for bytes in [b"e", b"x", b"i", b"t", b"\r"] {
        assert_eq!(next_input_until_timeout(&mut daemon), Some(bytes.to_vec()));
    }

    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _, _| {
            assert!(view.input_active());
            view.adopt_typeahead();
            view.flush_typeahead();
            assert!(
                view.prompt.cmd.text().is_empty(),
                "exit belongs to the finished session"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "returning from exit must not inject Ctrl-U into the local prompt"
    );
}

#[gpui::test]
fn ctrl_v_on_the_alternate_screen_reaches_the_pty_as_syn(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the full-screen program owns input");
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("ctrl-v"),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x16]));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the clipboard must stay where it is: vim's Ctrl+V is blockwise select, not paste"
    );
}

fn wait_for_input_active(window: &gpui::WindowHandle<TerminalView>, cx: &mut TestAppContext) {
    for _ in 0..200 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the local editor never engaged at the prompt");
}

/// Handing the line back to the shell walks the cursor left once per
/// character it sat before. Those are arrow keys like any other, so under
/// DECCKM they have to be SS3 — and zsh's zle does turn DECCKM on, so this
/// is the ordinary case rather than the exotic one.
#[gpui::test]
fn a_handoff_walks_the_cursor_back_in_ss3_under_app_cursor_mode(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Output(b"\x1b[?1h".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            assert!(
                view.key_flags().app_cursor(),
                "the shell asked for application cursor keys"
            );
            for ch in ["z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            view.handle_editor_key(&key("left"), cx);
            view.handle_editor_key(&key("left"), cx);
            view.complete_tab(true, cx);
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"zzqqx".to_vec())
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1bOD\x1bOD".to_vec()),
        "the cursor walks back in SS3, not CSI"
    );
}

#[gpui::test]
fn tab_with_no_candidates_hands_the_line_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            for ch in ["z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            assert_eq!(view.prompt.cmd.text(), "zzqqx");
            view.complete_tab(true, cx);
            assert_eq!(view.prompt.cmd.text(), "", "the line moved to the shell");
            assert!(
                !view.input_active(),
                "the shell owns the prompt after the handoff"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"zzqqx".to_vec()),
        "the edited line ships ahead of the Tab"
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\t".to_vec()),
        "the Tab reaches the PTY instead of being swallowed"
    );

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let applied = window
            .update(cx, |view, _, _| view.terminal.prompt_seq() >= 2)
            .unwrap();
        if applied {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, _, _| {
            assert!(
                !view.input_active(),
                "a same-prompt redraw must not re-engage the editor"
            );
        })
        .unwrap();

    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);
}

#[gpui::test]
fn ctrl_c_after_tab_handoff_returns_the_fresh_prompt_to_the_editor(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            for ch in ["z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            view.complete_tab(true, cx);
            assert!(!view.input_active(), "Tab handed this line to the shell");
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: key("ctrl-c"),
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"zzqqx".to_vec())
    );
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x03]));

    // Tab handoff never emitted C, so the shell integration reports the
    // interrupted prompt as another A/B while the daemon is still at_prompt.
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(130),
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            type_char(view, "n", window, cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "n",
                "the first character on the fresh line belongs to anterm's editor"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the fresh line must not keep going raw to the shell"
    );
}

#[gpui::test]
fn command_submitted_in_shell_after_tab_handoff_is_recorded_in_history(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            seed_history(view, &["hx old_file.rs"], cx);
            for ch in ["h", "x", " ", "z", "z", "q", "q", "x"] {
                type_char(view, ch, window, cx);
            }
            view.complete_tab(true, cx);
            assert!(!view.input_active(), "Tab handed this line to the shell");
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"hx zzqqx".to_vec())
    );
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));

    // Shell completes and executes the handed-off command, emitting OSC 133;C;<cmd>,
    // then finishes and returns to the next prompt.
    DaemonMsg::Output(b"\x1b]133;C;hx ~/Downloads/persona_manifest.json\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;B\x07".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            for ch in ["h", "x", " "] {
                type_char(view, ch, window, cx);
            }
            view.history_prev(cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "hx ~/Downloads/persona_manifest.json",
                "pressing Up with prefix `hx ` must recall the command executed after shell handoff"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_late_remote_listing_leaves_a_line_the_editor_no_longer_owns_alone(
    cx: &mut TestAppContext,
) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("ls /nope/");
            view.prompt.editor_handoff = Some(view.terminal.prompt_cycle());
            assert!(!view.input_active(), "the shell owns this prompt already");

            let req =
                super::completion::remote_path_request("ls /nope/", 9, "/home/u").unwrap();
            view.remote_path_results(req, "ls /nope/", 9, Vec::new(), true, cx);

            assert_eq!(
                view.prompt.cmd.text(),
                "ls /nope/",
                "an empty listing must not hand off a line the editor no longer drives"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "not one byte reached the wire"
    );
}

#[gpui::test]
fn tab_completion_off_sends_every_tab_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.tab_completion = false;
        cx.set_global(cfg);
    });
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            for ch in ["c", "d", " "] {
                type_char(view, ch, window, cx);
            }
            view.complete_tab(true, cx);
            assert!(view.prompt.completion.is_none(), "no anterm menu while opted out");
            assert_eq!(view.prompt.cmd.text(), "");
        })
        .unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"cd ".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
}

fn dir_candidate(text: &str, start: usize, end: usize) -> completion::Candidate {
    completion::Candidate {
        text: text.into(),
        kind: CandidateKind::Dir,
        start,
        end,
        description: None,
        icon: None,
    }
}

#[gpui::test]
fn accepting_a_candidate_quotes_it_for_the_shell(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            view.prompt.cmd.set("cd My");
            view.completion_insert(&dir_candidate("My Documents", 3, 5), 3);
            assert_eq!(
                view.prompt.cmd.text(),
                "cd 'My Documents'/",
                "an unquoted candidate resplits into two arguments and the command breaks"
            );

            view.prompt.cmd.set("cd ~/My");
            view.completion_insert(&dir_candidate("~/My Documents", 3, 6), 3);
            assert_eq!(
                view.prompt.cmd.text(),
                "cd ~/'My Documents'/",
                "the ~ stays outside the quotes so the shell still expands it"
            );

            view.prompt.cmd.set("git commit --mess");
            view.completion_insert(
                &completion::Candidate {
                    text: "--message".into(),
                    kind: CandidateKind::Flag,
                    start: 11,
                    end: 17,
                    description: None,
                    icon: None,
                },
                11,
            );
            assert_eq!(view.prompt.cmd.text(), "git commit --message ");
        })
        .unwrap();
}

#[gpui::test]
fn a_candidate_needing_escapes_is_never_half_applied(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("cd My");
            let offered = view.offer_candidates(
                "cd My",
                3,
                5,
                vec![
                    dir_candidate("My Documents", 3, 5),
                    dir_candidate("My Music", 3, 5),
                ],
                0,
                cx,
            );
            assert!(offered.is_some(), "two candidates open a menu");
            assert_eq!(
                view.prompt.cmd.text(),
                "cd My",
                "the common prefix here is `My ` — writing it raw would break the line \
                 and the trailing space would close the menu on the next keystroke"
            );
        })
        .unwrap();
}

fn parsed(text: &str) -> crate::prompt::generator::Parsed {
    crate::prompt::generator::Parsed {
        text: text.into(),
        description: None,
    }
}

#[gpui::test]
fn a_generator_that_supplies_no_match_closes_the_menu(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            // `git ckout<Tab>`: no subcommand matches, but git's alias generator
            // is in flight, so the session opens empty and waits for it.
            view.prompt.cmd.set("ckout");
            let generation =
                view.open_completion(CompletionSession::new(0, "ckout".into(), Vec::new(), 1));
            assert!(
                view.prompt.completion.is_some(),
                "the menu waits for its generator"
            );

            view.completion_merge(generation, Vec::new(), cx);
            assert!(
                view.prompt.completion.is_none(),
                "a menu that never got a candidate must not stay armed — it swallows \
                 every later Tab instead of handing the line to the shell"
            );
        })
        .unwrap();
}

#[gpui::test]
fn generator_results_that_match_nothing_close_the_menu(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("ckout");
            let generation =
                view.open_completion(CompletionSession::new(0, "ckout".into(), Vec::new(), 1));

            view.completion_merge(generation, vec![parsed("main"), parsed("release")], cx);
            assert!(
                view.prompt.completion.is_none(),
                "branches that match nothing typed are as good as no candidates at all"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_menu_waits_while_another_generator_is_still_in_flight(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("ck");
            let generation =
                view.open_completion(CompletionSession::new(0, "ck".into(), Vec::new(), 2));

            view.completion_merge(generation, Vec::new(), cx);
            assert!(
                view.prompt.completion.is_some(),
                "one generator came back empty, the other has not answered yet"
            );

            view.completion_merge(generation, vec![parsed("ckout-fix")], cx);
            let s = view
                .prompt
                .completion
                .as_ref()
                .expect("the second one supplied a match");
            assert_eq!(s.filtered.len(), 1);
        })
        .unwrap();
}

fn key(spec: &str) -> gpui::Keystroke {
    gpui::Keystroke::parse_with_os(spec, anterm_ui_kit::platform::current_os()).expect("valid keystroke spec")
}

#[test]
fn shim_detection_names_known_wrappers_only() {
    assert_eq!(known_pty_shim("zsh (kiro-cli-term)"), Some("kiro-cli-term"));
    assert_eq!(known_pty_shim("figterm"), Some("figterm"));
    assert_eq!(known_pty_shim("qterm"), Some("qterm"));
    assert_eq!(known_pty_shim("ssh"), None);
    assert_eq!(known_pty_shim("wezterm"), None);
    assert_eq!(known_pty_shim(""), None);
    assert!(integration_notice_message(Some("kiro-cli-term")).contains("kiro-cli-term"));
    assert!(!integration_notice_message(None).contains("intercepting"));
}

#[gpui::test]
fn ctrl_r_without_integration_raises_the_notice_once(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();

    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            let ctrl_r = KeyDownEvent {
                keystroke: key("ctrl-r"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.prompt.integration_notice.is_none(),
                "the grace window stays silent"
            );

            view.session.created_at = std::time::Instant::now() - INTEGRATION_GRACE * 2;
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.prompt.integration_notice.is_some(),
                "Ctrl+R raises the notice"
            );
            cx.notify();
        })
        .unwrap();

    cx.run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(
                view.prompt.integration_notice.is_some(),
                "the notice survives a real render pass"
            );

            let ctrl_r = KeyDownEvent {
                keystroke: key("ctrl-r"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.prompt.integration_notice.is_none(),
                "a keystroke dismisses the notice"
            );
            view.on_key_down(&ctrl_r, window, cx);
            assert!(
                view.prompt.integration_notice.is_none(),
                "the notice is one-shot per pane"
            );
        })
        .unwrap();
}

#[gpui::test]
fn insert_newline_action_extends_the_line_and_enter_submits_it(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("anterm-covtest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    anterm_ui_kit::config::set_config_dir(dir);

    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "the editor owns an idle prompt");
            view.commit_text("echo a", cx);
            view.insert_newline_action(cx);
            view.commit_text("echo b", cx);
            assert_eq!(view.prompt.cmd.text(), "echo a\necho b");

            view.handle_editor_key(&key("enter"), cx);
            assert!(view.prompt.cmd.is_empty(), "Enter submits the whole buffer");
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"echo a\recho b\r".to_vec()),
        "the multi-line command reaches the PTY in one submit"
    );
}

#[gpui::test]
fn insert_newline_action_closes_the_completion_menu_but_enter_still_accepts(
    cx: &mut TestAppContext,
) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    let candidate = |text: &str| completion::Candidate {
        text: text.to_string(),
        kind: CandidateKind::Command,
        start: 4,
        end: 4,
        description: None,
        icon: None,
    };

    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set_with_cursor("git ", 4);
            view.open_completion(CompletionSession::new(
                4,
                String::new(),
                vec![candidate("status")],
                0,
            ));

            view.insert_newline_action(cx);
            assert!(
                view.prompt.completion.is_none(),
                "the newline ends the completed word, so the menu closes"
            );
            assert_eq!(view.prompt.cmd.text(), "git \n");

            view.prompt.cmd.set_with_cursor("git ", 4);
            view.open_completion(CompletionSession::new(
                4,
                String::new(),
                vec![candidate("status")],
                0,
            ));
            view.handle_editor_key(&key("enter"), cx);
            assert_eq!(view.prompt.cmd.text(), "git status ");
        })
        .unwrap();
}

#[gpui::test]
fn shift_enter_reaches_a_foreground_tui_with_kitty_encoding(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness_on(cx, PtySource::LocalConpty);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    DaemonMsg::Output(b"\x1b[>1u".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        if window
            .update(cx, |view, _, _| view.key_flags().kitty_active())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground TUI owns input");
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("shift-enter");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[13;2u".to_vec())
    );

    vcx.simulate_keystrokes("ctrl-j");
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[106;5u".to_vec())
    );
}

#[gpui::test]
fn shift_enter_reaches_a_foreground_tui_as_lf_without_kitty(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness_on(cx, PtySource::Raw);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground TUI owns input");
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("shift-enter");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\n".to_vec()));

    vcx.simulate_keystrokes("ctrl-j");
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\n".to_vec()));
}

#[gpui::test]
fn newline_chords_reach_conpty_as_ctrl_j_without_kitty(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness_on(cx, PtySource::LocalConpty);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active());
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    for chord in ["shift-enter", "ctrl-j"] {
        vcx.simulate_keystrokes(chord);
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"\x1b[74;36;10;1;8;1_\x1b[74;36;10;0;8;1_".to_vec()),
            "{chord} must preserve Ctrl+J for native console readers"
        );
    }
}

#[gpui::test]
fn alt_enter_keeps_its_legacy_encoding_in_a_foreground_tui(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.input_active(), "the foreground TUI owns input");
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("alt-enter");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b\r".to_vec())
    );
}

#[gpui::test]
fn insert_newline_action_declines_when_the_editor_is_not_live(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("keep me");
            view.terminal.exited = true;
            assert!(!view.input_active());
            view.insert_newline_action(cx);
            assert_eq!(view.prompt.cmd.text(), "keep me", "no newline inserted");
        })
        .unwrap();
}

#[gpui::test]
fn the_keymap_routes_both_newline_chords_to_the_action(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
            view.commit_text("echo a", cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("shift-enter");
    vcx.simulate_keystrokes("alt-enter");
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.prompt.cmd.text(),
                "echo a\n\n",
                "both chords dispatched InsertNewline instead of submitting"
            );
        })
        .unwrap();

    cx.update(|cx| anterm_ui_kit::keymap::rebind(cx));
    vcx.simulate_keystrokes("shift-enter");
    window
        .update(cx, |view, _, _| {
            assert_eq!(
                view.prompt.cmd.text(),
                "echo a\n\n\n",
                "the chord survives a rebind"
            );
        })
        .unwrap();
}

/// The whole chain for #834, through the real dispatch tree: F3 is bound
/// to Find Next off macOS, and gpui matches bindings before the pane's key
/// handler. With no find bar open the action gives the key back, the pane
/// encodes it, and PSReadLine's CharacterSearch gets its `\EOR`.
///
/// F7 has no binding at all and is the control: it takes the same route
/// with nothing to fall through.
#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn an_unused_find_binding_gives_f3_back_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
            view.commit_text("echo a", cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    for (chord, seq) in [("f3", b"\x1bOR".to_vec()), ("f7", b"\x1b[18~".to_vec())] {
        window
            .update(cx, |view, _, _| {
                // The previous handoff gave this prompt to the shell for
                // good; take it back so both keys are tested from the
                // same starting state.
                view.prompt.editor_handoff = None;
                view.prompt.cmd.set("echo a");
                assert!(view.search_state.active.is_none(), "no find bar is open");
            })
            .unwrap();
        vcx.simulate_keystrokes(chord);
        window
            .update(cx, |view, _, _| {
                assert!(view.search_state.active.is_none(), "{chord} did not open the find bar");
                assert_eq!(view.prompt.cmd.text(), "", "{chord} handed the line over");
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"echo a".to_vec()),
            "{chord} puts the line on the shell's prompt first"
        );
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(seq),
            "{chord} reaches the PTY"
        );
    }
}

#[gpui::test]
fn ctrl_r_fuzzy_search_accepts_into_the_editor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(
                view,
                &["git status", "cargo build", "git commit -m x"],
                cx,
            );

            view.handle_editor_key(&key("ctrl-r"), cx);
            assert!(view.prompt.reverse_search.is_some(), "Ctrl+R opens the search");
            view.commit_text("gst", cx);
            assert_eq!(
                selected_reverse_search_line(view, cx).as_deref(),
                Some("git status")
            );
            view.handle_editor_key(&key("enter"), cx);
            assert!(view.prompt.reverse_search.is_none(), "Enter closes the search");
            assert_eq!(view.prompt.cmd.text(), "git status");
        })
        .unwrap();
}

#[gpui::test]
fn ctrl_r_steps_matches_and_cmd_enter_runs(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();

    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(
                view,
                &["git status", "cargo build", "git commit -m x"],
                cx,
            );

            view.handle_editor_key(&key("ctrl-r"), cx);
            view.commit_text("git", cx);
            assert_eq!(
                selected_reverse_search_line(view, cx).as_deref(),
                Some("git commit -m x")
            );
            view.handle_editor_key(&key("ctrl-r"), cx);
            assert_eq!(
                selected_reverse_search_line(view, cx).as_deref(),
                Some("git status")
            );
            view.handle_editor_key(&key("cmd-enter"), cx);
            assert!(view.prompt.reverse_search.is_none());
            assert!(view.prompt.cmd.is_empty(), "submit clears the editor");
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"git status\r".to_vec()),
        "Cmd+Enter ships the selected line to the PTY"
    );
}

/// The second half of #834. Even once the encoder knew the F keys, the
/// inline editor still ate them: `handle_editor_key` had no arm for a
/// named key it does not bind, so F8 fell out of the bottom of the match
/// and died on a `cx.notify()`. PSReadLine's HistorySearchBackward acts on
/// the line that is on the prompt, so the fix is the unknown-chord route —
/// the line goes over first, then the key.
#[gpui::test]
fn function_keys_hand_the_line_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    for (chord, seq) in [
        ("f8", b"\x1b[19~".to_vec()),
        ("shift-f8", b"\x1b[19;2~".to_vec()),
        ("alt-f7", b"\x1b[18;3~".to_vec()),
        ("f3", b"\x1bOR".to_vec()),
    ] {
        window
            .update(cx, |view, _, cx| {
                view.prompt.cmd.set("git st");
                view.handle_editor_key(&key(chord), cx);
                assert_eq!(view.prompt.cmd.text(), "", "{chord} handed the line over");
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(b"git st".to_vec()),
            "{chord} puts the line on the shell's prompt first"
        );
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(seq),
            "{chord} follows the line"
        );
    }
}

#[gpui::test]
fn ctrl_j_and_ctrl_m_submit_the_line_like_enter(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();

    let (window, mut daemon) = harness(cx);
    for (chord, line) in [("ctrl-j", "echo j"), ("ctrl-m", "echo m")] {
        window
            .update(cx, |view, _, cx| {
                view.prompt.cmd.set(line);
                view.handle_editor_key(&key(chord), cx);
                assert!(view.prompt.cmd.is_empty(), "{chord} clears the editor");
            })
            .unwrap();
        assert_eq!(
            next_input_until_timeout(&mut daemon),
            Some(format!("{line}\r").into_bytes()),
            "{chord} ships the line to the PTY"
        );
    }
}

#[gpui::test]
fn history_search_off_sends_ctrl_r_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.history_search = false;
        cx.set_global(cfg);
    });
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["git status"], cx);
            view.prompt.cmd.set("gi");
            view.handle_editor_key(&key("ctrl-r"), cx);
            assert!(
                view.prompt.reverse_search.is_none(),
                "no anterm menu while opted out"
            );
            assert_eq!(view.prompt.cmd.text(), "", "the line went to the shell");
        })
        .unwrap();
    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"gi".to_vec()));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(vec![0x12]),
        "the raw ^R follows the handed-over line"
    );
}

/// The whole point of `prompt_editor: false`: a prefix and an arrow key
/// reach the PTY at a live OSC 133 prompt, so the shell's own line editor
/// (zsh's ZLE, readline) runs the widget bound there — including a history
/// widget reading the shell's own, shared history.
#[gpui::test]
fn prompt_editor_off_hands_typing_and_arrows_to_the_shell(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    // The editor engages first: this is a pane with working shell
    // integration, which the mode has to keep working.
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            seed_history(view, &["quit"], cx);
            view.set_prompt_editor(false, cx);
            assert!(
                !view.input_active(),
                "the shell owns the prompt in native input mode"
            );
            assert!(
                view.terminal.at_prompt(),
                "OSC 133 prompt tracking stays live"
            );

            type_char(view, "h", window, cx);
            let up = KeyDownEvent {
                keystroke: key("up"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&up, window, cx);

            assert_eq!(view.prompt.cmd.text(), "", "nothing was typed into anterm's editor");
            assert!(
                view.prompt.history_nav.is_none(),
                "Up never touched anterm's own history"
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"h".to_vec()));
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"\x1b[A".to_vec()),
        "the physical Up key reaches the PTY for ZLE to bind"
    );

    window
        .update(cx, |view, _, cx| {
            view.set_prompt_editor(true, cx);
            assert!(
                view.input_active(),
                "turning it back on re-arms the editor at the same prompt"
            );
        })
        .unwrap();
}

/// Tab and Ctrl-R are the two keys with their own opt-outs; turning the
/// editor off has to hand them over too, without the missing-integration
/// notice that Ctrl-R raises when anterm *wanted* the key and could not have
/// it.
#[gpui::test]
fn prompt_editor_off_hands_over_tab_and_ctrl_r(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            view.set_prompt_editor(false, cx);
            view.session.created_at = std::time::Instant::now() - INTEGRATION_GRACE * 2;
            view.tab_pressed(true, cx);

            let ctrl_r = KeyDownEvent {
                keystroke: key("ctrl-r"),
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&ctrl_r, window, cx);
            assert!(view.prompt.completion.is_none(), "no anterm completion menu");
            assert!(view.prompt.reverse_search.is_none(), "no anterm history menu");
            assert!(
                view.prompt.integration_notice.is_none(),
                "the shell owning ^R is what was asked for, not a gap to report"
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(b"\t".to_vec()));
    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x12]));
}

/// Turning the setting off mid-line must not eat what is already typed —
/// the editor hands the line over the way an unknown chord does, so it is
/// still on the prompt for the shell to finish.
#[gpui::test]
fn turning_the_prompt_editor_off_hands_the_typed_line_over(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("git status");
            view.set_prompt_editor(false, cx);
            assert_eq!(view.prompt.cmd.text(), "", "the editor let the line go");
        })
        .unwrap();

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"git status".to_vec()),
        "the half-typed line is on the shell's prompt now"
    );
}

#[gpui::test]
fn reverse_search_menu_survives_a_real_render_pass(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        if window
            .update(cx, |view, _, _| view.terminal.at_prompt())
            .unwrap()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "prompt report engages the editor");
            let mut snap = crate::prompt::history::ScopeSnapshot::new();
            snap.entries = ["git status", "cargo build --release", "echo hello"]
                .into_iter()
                .map(String::from)
                .collect();
            snap.meta.insert(
                "cargo build --release".into(),
                crate::prompt::history::EntryMeta {
                    ts: Some(unix_now().saturating_sub(7200)),
                    exit: Some(1),
                    duration_ms: Some(1850),
                },
            );
            cx.default_global::<HistoryStore>()
                .replace_scope_for_test(&view.prompt.history_scope, snap);
            view.handle_editor_key(&key("ctrl-r"), cx);
            view.commit_text("c", cx);
            assert!(
                view.prompt.reverse_search
                    .as_ref()
                    .is_some_and(|rs| !rs.matches().is_empty()),
                "the query has matches for the menu to draw"
            );
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.prompt.reverse_search.is_some(), "search survives the frame");
        })
        .unwrap();
}

#[gpui::test]
fn submitted_command_backfills_its_exit_code(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let dir = anterm_ui_kit::config::config_dir_path().expect("a config dir resolves");

    let (window, mut daemon) = harness(cx);
    let wait = |cx: &mut TestAppContext, pred: &dyn Fn(&TerminalView) -> bool, what: &str| {
        for _ in 0..200 {
            if window.update(cx, |view, _, _| pred(view)).unwrap() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    };

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait(cx, &|v| v.terminal.at_prompt(), "the initial prompt report");

    let marker = format!("anterm_gpui_exit_marker_{}", std::process::id());
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set(&marker);
            view.submit_command(cx);
            assert!(view.prompt.pending_history.is_some(), "record defers for the exit");
        })
        .unwrap();

    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(3),
    }
    .encode(&mut daemon)
    .unwrap();
    wait(
        cx,
        &|v| v.terminal.at_prompt() && v.terminal.last_exit_code() == Some(3),
        "the post-command prompt report",
    );

    window
        .update(cx, |view, window, cx| {
            view.poll_foreground(window, cx);
            assert!(view.prompt.pending_history.is_none(), "poll flushed the record");
            let store = cx.default_global::<HistoryStore>();
            let meta = store
                .snapshot(&view.prompt.history_scope)
                .meta
                .get(&marker)
                .copied();
            assert_eq!(
                meta.and_then(|m| m.exit),
                Some(3),
                "in-memory metadata learned the exit code"
            );
            assert!(
                meta.and_then(|m| m.duration_ms).is_some(),
                "in-memory metadata learned elapsed duration"
            );
            store.ensure_worker().flush_sync();
        })
        .unwrap();

    let db = crate::prompt::history::HistoryDb::open_at(dir.join("history.db"))
        .expect("sqlite history db written");
    let persisted = db
        .load_scope_snapshot(
            &crate::prompt::history::Scope::Local,
            crate::prompt::history::MAX_HOT_ENTRIES,
        )
        .expect("snapshot loaded");
    let persisted_meta = persisted
        .meta
        .get(&marker)
        .expect("the submitted command was persisted to SQLite");
    assert!(persisted_meta.ts.is_some());
    assert_eq!(persisted_meta.exit, Some(3), "exit code field persisted");
    assert!(persisted_meta.duration_ms.is_some(), "duration_ms persisted");
}

#[gpui::test]
fn meta_word_chords_edit_the_prompt_line(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta = |key: &str| gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: key.to_string(),
                key_char: None,
            };
            view.prompt.cmd.set("echo hello");
            view.handle_editor_key(&meta("b"), cx);
            assert_eq!(view.prompt.cmd.cursor(), 5);
            view.handle_editor_key(&meta("d"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo ");
            view.handle_editor_key(&meta("b"), cx);
            assert_eq!(view.prompt.cmd.cursor(), 0);
            view.handle_editor_key(&meta("f"), cx);
            assert_eq!(view.prompt.cmd.cursor(), 4);
            view.handle_editor_key(&meta("z"), cx);
            assert_eq!(view.prompt.cmd.text(), "");
        })
        .unwrap();
}

fn scroll_into_history(view: &TerminalView, offset: usize) {
    let mut parser: alacritty_terminal::vte::ansi::Processor = Default::default();
    let mut term = view.terminal.term.lock();
    parser.advance(&mut *term, &b"line\r\n".repeat(60));
    term.scroll_display(Scroll::Delta(offset as i32));
    assert_eq!(
        term.grid().display_offset(),
        offset,
        "the viewport starts parked in the scrollback"
    );
}

fn display_offset(view: &TerminalView) -> usize {
    view.terminal.term.lock().grid().display_offset()
}

/// Shaped after what macOS actually delivers, measured on a wheel mouse and
/// a trackpad: both arrive as pixels, and only the trackpad ever reports a
/// phase. One wheel detent is ~103px, roughly five lines at a 21px line
/// height; a trackpad event is a fraction of that but can reach ~3 lines
/// when flicked, which is why phase and not size decides.
fn wheel(view: &TerminalView, lines: f32, phase: gpui::TouchPhase) -> ScrollEvent {
    ScrollEvent {
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(lines * view.render.line_height.as_f32()))),
        touch_phase: phase,
        ..Default::default()
    }
}

fn notch(view: &TerminalView, lines: f32) -> ScrollEvent {
    wheel(view, lines, gpui::TouchPhase::Moved)
}

/// The whole point of the animation: a detent must not land in one go.
#[gpui::test]
fn a_wheel_notch_is_spread_over_frames(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -4.9);
            view.on_scroll(&ev, w, cx);
            assert_eq!(
                display_offset(view),
                10,
                "the detent was applied whole, before a single frame ran"
            );
            assert_eq!(view.pointer.scroll_frac, 0., "and not even a sliver of it");
            assert!(view.pointer.scroll_anim.is_some(), "nothing was left to animate");
        })
        .unwrap();
}

/// Zooming has to take the wheel away from the buffer entirely, or the
/// grid would slide under the pointer while the font changed size.
#[gpui::test]
fn the_zoom_modifier_takes_the_wheel_off_the_scrollback(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::Platform);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut ev = notch(view, -4.9);
            ev.modifiers = anterm_ui_kit::platform::secondary_key();
            view.on_scroll(&ev, w, cx);
            assert_eq!(display_offset(view), 10, "the wheel reached the grid");
            assert!(view.pointer.scroll_anim.is_none(), "and queued more of it");
        })
        .unwrap();
}

/// #668: the platform modifier is the wrong key to hardwire a zoom to —
/// on a Mac it is ⌘, which people are already holding for something
/// else — so which key zooms is a setting, all the way down to none.
#[test]
fn the_zoom_modifier_is_configurable_and_can_be_turned_off() {
    let secondary = anterm_ui_kit::platform::secondary_key();
    let alt = Modifiers::alt();
    assert!(zoom_wheel(MouseZoomModifier::Platform, &secondary));
    assert!(
        !zoom_wheel(MouseZoomModifier::None, &secondary),
        "off is off"
    );
    assert!(!zoom_wheel(MouseZoomModifier::Alt, &secondary));
    assert!(zoom_wheel(MouseZoomModifier::Alt, &alt));
    assert!(!zoom_wheel(MouseZoomModifier::Platform, &alt));
    assert!(zoom_wheel(MouseZoomModifier::Ctrl, &Modifiers::control()));
    assert_eq!(
        zoom_wheel(MouseZoomModifier::Platform, &Modifiers::control()),
        !cfg!(target_os = "macos"),
        "off macOS the platform modifier is Ctrl itself"
    );
    // A second modifier is somebody else's gesture, whichever key is bound.
    let both = Modifiers { shift: true, ..alt };
    assert!(!zoom_wheel(MouseZoomModifier::Alt, &both));
    assert!(!zoom_wheel(MouseZoomModifier::Platform, &Modifiers::none()));
}

/// The point of turning it off: the modifier goes back to being an
/// ordinary scroll, rather than eating the wheel.
#[gpui::test]
fn zooming_off_hands_the_wheel_back_to_the_scrollback(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| {
        cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::None;
    });
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut ev = notch(view, -4.9);
            ev.modifiers = anterm_ui_kit::platform::secondary_key();
            view.on_scroll(&ev, w, cx);
            assert!(
                view.pointer.scroll_anim.is_some(),
                "the wheel never reached the scrollback"
            );
        })
        .unwrap();
}

/// The bug this guards: a two-finger flick coasts long after the fingers
/// are gone, and every coasting event carries whatever modifiers are down
/// when it lands. Grabbing ⌘ for something else mid-coast used to read as
/// a zoom and run the font down to its minimum in a blink (#912).
#[gpui::test]
fn a_modifier_pressed_mid_flick_does_not_hijack_it_into_a_zoom(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            view.on_scroll(&wheel(view, -0.5, gpui::TouchPhase::Started), w, cx);
            let before = display_offset(view);
            // The tail, now stamped with ⌘ the hand reached for.
            let mut tail = wheel(view, -3., gpui::TouchPhase::Moved);
            tail.modifiers = anterm_ui_kit::platform::secondary_key();
            view.on_scroll(&tail, w, cx);
            assert!(
                display_offset(view) != before || view.pointer.scroll_anim.is_some(),
                "the tail stayed a scroll, the way the gesture started"
            );
            assert_eq!(view.pointer.zoom_debt, 0., "and never paid into the zoom");
        })
        .unwrap();
}

/// The same latch the other way round: a zoom gesture that outlives the
/// modifier keeps zooming rather than dumping its tail into the buffer.
#[gpui::test]
fn a_zoom_gesture_keeps_zooming_after_the_modifier_is_released(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::Platform);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut start = wheel(view, -0.5, gpui::TouchPhase::Started);
            start.modifiers = anterm_ui_kit::platform::secondary_key();
            view.on_scroll(&start, w, cx);
            view.on_scroll(&wheel(view, -0.5, gpui::TouchPhase::Moved), w, cx);
            assert_eq!(display_offset(view), 10, "the grid never moved");
            assert!(view.pointer.scroll_anim.is_none(), "and nothing was queued for it");
        })
        .unwrap();
}

/// The latch is per gesture, not per stream: fingers going down again
/// start a fresh question. Carrying the last answer over would mean one
/// ⌘-zoom left every later flick zooming with no modifier held at all.
#[gpui::test]
fn a_new_gesture_is_not_bound_by_what_the_last_one_answered(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().mouse_zoom_modifier = MouseZoomModifier::Platform);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let mut zoom = wheel(view, -0.5, gpui::TouchPhase::Started);
            zoom.modifiers = anterm_ui_kit::platform::secondary_key();
            view.on_scroll(&zoom, w, cx);
            assert_eq!(display_offset(view), 10, "that one was a zoom");
            // Fingers down again, nothing held: a plain scroll.
            view.on_scroll(&wheel(view, -3., gpui::TouchPhase::Started), w, cx);
            assert_ne!(
                display_offset(view),
                10,
                "the new gesture asked the question again"
            );
        })
        .unwrap();
}

/// A detent is one step however many lines the platform bills it as —
/// macOS calls a single notch five.
#[test]
fn a_wheel_detent_zooms_by_exactly_one_step() {
    assert_eq!(zoom_scroll_steps(4.9, 0., false), (1, 0.));
    assert_eq!(zoom_scroll_steps(-4.9, 0., false), (-1, 0.));
    assert_eq!(zoom_scroll_steps(0., 0., false), (0, 0.));
}

/// A trackpad has no detents, so a flick arrives as a stream of slivers.
/// Paying out a step per sliver would run the font from end to end.
#[test]
fn a_trackpad_flick_adds_up_to_whole_steps() {
    let (mut debt, mut steps) = (0., 0);
    for _ in 0..20 {
        let (s, d) = zoom_scroll_steps(1. / 3., debt, true);
        steps += s;
        debt = d;
    }
    assert_eq!(steps, 2, "twenty thirds of a line is two steps, not twenty");
    assert!(debt > 0., "and the remainder was dropped instead of kept");
}

/// The one thing typing at a prompt must never do is land where the
/// person typing cannot see it.
#[gpui::test]
fn typing_brings_the_view_back_to_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _w, cx| {
            assert!(
                view.input_active(),
                "this is the branch that was leaving the view parked"
            );
            scroll_into_history(view, 10);
            view.commit_text("l", cx);
            assert_eq!(
                display_offset(view),
                0,
                "the character went in while the viewport stayed in the scrollback"
            );
            assert_eq!(view.prompt.cmd.text(), "l", "and it did reach the line");
        })
        .unwrap();
}

/// A paste is a larger change than a keystroke to make out of sight, and
/// it went the same way.
#[gpui::test]
fn pasting_brings_the_view_back_to_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _w, cx| {
            scroll_into_history(view, 10);
            view.paste("cargo test\n".to_string(), cx);
            assert_eq!(display_offset(view), 0, "the paste landed off screen");
            assert_eq!(
                view.prompt.cmd.text(),
                "cargo test",
                "and the trailing newline is still dropped"
            );
        })
        .unwrap();
}

/// A trackpad is already a continuous stream — animating it would only put
/// lag between the fingers and the grid. It is told apart by its phase, not
/// by its delta type or size: a flick moves further in one event than a
/// slowly inched wheel does.
#[gpui::test]
fn a_trackpad_gesture_scrolls_the_instant_it_is_touched(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = wheel(view, -3., gpui::TouchPhase::Started);
            view.on_scroll(&ev, w, cx);
            assert_eq!(display_offset(view), 7, "the gesture was held back");
            assert!(
                view.pointer.scroll_anim.is_none(),
                "a trackpad started an animation"
            );
        })
        .unwrap();
}

/// Lifting the fingers does not end the stream: macOS keeps sending Moved
/// events for the momentum tail, and they are *larger* than the gesture
/// that spawned them. Treating those as a wheel would smooth what the
/// system is already smoothing.
#[gpui::test]
fn a_momentum_tail_is_not_mistaken_for_a_wheel(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            for (lines, phase) in [
                (-0.5, gpui::TouchPhase::Started),
                (-1.2, gpui::TouchPhase::Moved),
                (0., gpui::TouchPhase::Ended),
                (-2.9, gpui::TouchPhase::Moved),
                (-2.4, gpui::TouchPhase::Moved),
            ] {
                let ev = wheel(view, lines, phase);
                view.on_scroll(&ev, w, cx);
                assert!(
                    view.pointer.scroll_anim.is_none(),
                    "the momentum tail was animated at {lines} lines"
                );
            }
            assert_eq!(display_offset(view), 3, "the tail did not all land");
        })
        .unwrap();
}

/// Inching the wheel one detent at a time reads as continuous already.
#[gpui::test]
fn a_scroll_too_small_to_see_jump_stays_direct(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -0.57);
            view.on_scroll(&ev, w, cx);
            assert!(view.pointer.scroll_anim.is_none(), "half a line was animated");
            assert!(view.pointer.scroll_frac > 0., "and it did not move either");
        })
        .unwrap();
}

#[gpui::test]
fn turning_smooth_scrolling_off_restores_the_direct_path(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    cx.update(|cx| cx.global_mut::<Config>().smooth_scroll = false);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -3.);
            view.on_scroll(&ev, w, cx);
            assert_eq!(display_offset(view), 7);
            assert!(view.pointer.scroll_anim.is_none());
        })
        .unwrap();
}

/// An animation that kept walking after the view was moved out from under
/// it would drag the user back off the prompt they just jumped to.
#[gpui::test]
fn moving_the_viewport_cancels_an_animation_in_flight(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -4.9);
            view.on_scroll(&ev, w, cx);
            assert!(view.pointer.scroll_anim.is_some());

            view.jump_to_prompt();
            assert!(
                view.pointer.scroll_anim.is_none(),
                "the animation outlived the jump"
            );
            assert_eq!(display_offset(view), 0);
        })
        .unwrap();
}

/// A frame callback that was already queued when the animation was
/// cancelled must not walk the viewport on its stale epoch.
#[gpui::test]
fn a_stale_frame_after_cancellation_does_not_walk(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, w, cx| {
            scroll_into_history(view, 10);
            let ev = notch(view, -4.9);
            view.on_scroll(&ev, w, cx);
            let epoch = view.pointer.scroll_anim_epoch;
            view.cancel_scroll_anim();

            assert!(
                !view.scroll_anim_frame(epoch, cx),
                "a stale frame callback kept the animation alive"
            );
            assert_eq!(display_offset(view), 10, "the stale frame moved");
        })
        .unwrap();
}

#[gpui::test]
fn history_recall_snaps_the_viewport_back_to_the_prompt(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["echo hello"], cx);
            scroll_into_history(view, 10);
            view.pointer.scroll_frac = 0.5;

            view.handle_editor_key(&key("up"), cx);

            assert_eq!(view.prompt.cmd.text(), "echo hello", "↑ recalled the entry");
            assert_eq!(display_offset(view), 0, "and the viewport followed it down");
            assert_eq!(view.pointer.scroll_frac, 0., "the sub-line remainder reset too");
        })
        .unwrap();
}

#[gpui::test]
fn ctrl_p_and_ctrl_n_walk_the_history(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["git status", "cargo build", "echo hello"], cx);

            view.handle_editor_key(&key("ctrl-p"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo hello");
            view.handle_editor_key(&key("ctrl-p"), cx);
            assert_eq!(view.prompt.cmd.text(), "cargo build");
            view.handle_editor_key(&key("ctrl-n"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo hello");
            view.handle_editor_key(&key("ctrl-n"), cx);
            assert_eq!(view.prompt.cmd.text(), "");
        })
        .unwrap();
}

#[gpui::test]
fn up_and_ctrl_p_recall_the_last_command_matching_the_typed_prefix(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["git status", "echo hello", "git log", "ls"], cx);
            view.prompt.cmd.set("git");

            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "git log",
                "↑ skips ls and echo hello, which do not start with git"
            );
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "git status");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.prompt.cmd.text(), "git log");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "git",
                "↓ past the newest match restores the prefix"
            );

            view.prompt.cmd.set("echo");
            view.handle_editor_key(&key("ctrl-p"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo hello");
        })
        .unwrap();
}

#[gpui::test]
fn history_search_takes_its_prefix_from_the_text_left_of_the_cursor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["git status", "echo hello", "git log", "ls"], cx);

            // Ctrl+A parks the cursor at the start. Nothing sits left of
            // it, so UP walks the whole list instead of filtering on a line
            // the user is about to edit in front of.
            view.prompt.cmd.set_with_cursor("git", 0);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "ls", "an empty prefix filters nothing");

            // A cursor parked mid-line searches on what is behind it, and
            // DOWN past the newest match restores the whole line, the part
            // right of the cursor included.
            view.prompt.history_nav = None;
            view.prompt.cmd.set_with_cursor("git hello", 4);
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "git log",
                "the prefix is `git `, not the whole `git hello`"
            );
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.prompt.cmd.text(), "git hello");
        })
        .unwrap();
}

#[gpui::test]
fn ctrl_e_accepts_a_ghost_suggestion_at_the_end(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            assert!(view.input_active(), "the local editor owns a fresh prompt");
            seed_history(view, &["git log --oneline"], cx);
            view.prompt.cmd.set("git l");

            view.handle_editor_key(&key("ctrl-e"), cx);

            assert_eq!(view.prompt.cmd.text(), "git log --oneline");
            assert!(
                view.prompt.editor_handoff.is_none(),
                "accepting a local suggestion must not hand input to the shell"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the local editor must consume Ctrl+E instead of forwarding 0x05"
    );
}

#[gpui::test]
fn ctrl_e_only_moves_to_the_end_when_no_ghost_is_visible(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["git log --oneline"], cx);
            view.prompt.cmd.set_with_cursor("git l", 2);

            view.handle_editor_key(&key("ctrl-e"), cx);

            assert_eq!(view.prompt.cmd.text(), "git l");
            assert_eq!(view.prompt.cmd.cursor(), view.prompt.cmd.len());
        })
        .unwrap();
}

#[gpui::test]
fn an_unknown_ctrl_chord_goes_to_the_shell_with_the_line(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("echo hi");
            view.handle_editor_key(&key("ctrl-t"), cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "",
                "the line left for the shell, so the local buffer is empty"
            );
            assert!(
                view.prompt.editor_handoff.is_some(),
                "the local editor stands down for the rest of the line"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input(&mut daemon), vec![0x14], "⌃T reached the shell");
}

#[gpui::test]
fn an_unknown_meta_chord_goes_to_the_shell_with_the_line(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("echo hi");
            view.handle_editor_key(
                &gpui::Keystroke {
                    modifiers: gpui::Modifiers {
                        alt: true,
                        ..Default::default()
                    },
                    key: "u".to_string(),
                    key_char: None,
                },
                cx,
            );
            assert_eq!(view.prompt.cmd.text(), "");
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input(&mut daemon), b"\x1bu".to_vec());
}

#[gpui::test]
fn ctrl_y_yanks_back_what_the_kill_chords_cut(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("echo hello world");
            view.handle_editor_key(&key("ctrl-w"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo hello ");
            view.handle_editor_key(&key("ctrl-y"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo hello world");
            assert!(
                view.prompt.editor_handoff.is_none(),
                "the line never left for the shell"
            );
        })
        .unwrap();
}

#[gpui::test]
fn meta_dot_walks_back_through_the_last_words(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(
                view,
                &["git status", "cargo build --release", "echo hello world"],
                cx,
            );
            view.prompt.cmd.set("ls ");

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.prompt.cmd.text(), "ls world", "newest entry's last word");
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.prompt.cmd.text(), "ls --release", "repeat steps one back");
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.prompt.cmd.text(), "ls status");
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.prompt.cmd.text(), "ls status");
            assert_eq!(view.prompt.cmd.cursor(), "ls status".chars().count());
        })
        .unwrap();
}

#[gpui::test]
fn an_intervening_key_restarts_the_last_word_walk(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(
                view,
                &["cargo build --release", "echo hello world"],
                cx,
            );

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.prompt.cmd.text(), "world");
            view.handle_editor_key(&key("left"), cx);
            view.handle_editor_key(&key("end"), cx);
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "worldworld",
                "a fresh walk appends rather than replacing the earlier word"
            );
        })
        .unwrap();
}

#[gpui::test]
fn an_intervening_ime_commit_restarts_the_last_word_walk(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for_input_active(&window, cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(
                view,
                &["cargo build --release", "echo hello world"],
                cx,
            );

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(view.prompt.cmd.text(), "world");
            view.commit_text("x", cx);
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "worldxworld",
                "the typed char survives; the walk starts over after it"
            );
        })
        .unwrap();
}

#[gpui::test]
fn meta_dot_over_a_selection_records_where_the_word_landed(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            let meta_dot = gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    alt: true,
                    ..Default::default()
                },
                key: ".".to_string(),
                key_char: None,
            };
            seed_history(
                view,
                &["cargo build --release", "echo hello world"],
                cx,
            );
            view.prompt.cmd.set("ls foo");
            view.prompt.cmd.set_cursor(3);
            view.prompt.cmd.extend_to(6);

            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "ls world",
                "the word replaced the selection"
            );
            view.handle_editor_key(&meta_dot, cx);
            assert_eq!(
                view.prompt.cmd.text(),
                "ls --release",
                "the repeat swapped the word, not some other span"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_shifted_meta_chord_hands_off_the_shifted_character(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("echo hi");
            view.handle_editor_key(
                &gpui::Keystroke {
                    modifiers: gpui::Modifiers {
                        alt: true,
                        shift: true,
                        ..Default::default()
                    },
                    key: "u".to_string(),
                    key_char: None,
                },
                cx,
            );
            assert_eq!(view.prompt.cmd.text(), "");
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input(&mut daemon), b"\x1bU".to_vec());
}

#[gpui::test]
fn a_known_ctrl_chord_stays_in_the_local_editor(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set("echo hi");
            view.handle_editor_key(&key("ctrl-w"), cx);
            assert_eq!(view.prompt.cmd.text(), "echo ", "⌃W cut the word locally");
            assert!(view.prompt.editor_handoff.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn pty_write_events_reach_the_daemon_as_input(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.handle_event(AlacEvent::PtyWrite("ping".into()), cx);
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"ping".to_vec());
}

#[gpui::test]
fn a_remote_listing_says_so_while_it_runs_and_when_it_fails(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, _| {
            assert!(view.remote_completion_notice_text().is_none());
            view.prompt.remote_completion_inflight = true;
            assert_eq!(
                view.remote_completion_notice_text().as_deref(),
                Some("listing remote…"),
                "a slow link must not read as a broken Tab key (#585)"
            );
            view.prompt.remote_completion_inflight = false;
            view.prompt.remote_completion_notice = Some("remote listing failed — boom".to_string());
            assert_eq!(
                view.remote_completion_notice_text().as_deref(),
                Some("remote listing failed — boom"),
                "a failed listing is not the same silence as an empty one"
            );
        })
        .unwrap();
}

#[gpui::test]
fn an_exited_local_pane_still_swallows_every_key(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            view.terminal.exited = true;
            let cmd_a = KeyDownEvent {
                keystroke: gpui::Keystroke {
                    modifiers: Modifiers {
                        platform: true,
                        ..Modifiers::default()
                    },
                    key: "a".into(),
                    key_char: None,
                },
                is_held: false,
                prefer_character_input: false,
            };
            view.on_key_down(&cmd_a, window, cx);
            assert!(
                view.terminal.term.lock().selection.is_none(),
                "an exited local pane is finished; its keyboard is unchanged"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_local_pane_types_exactly_as_it_always_did(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            assert!(view.accepts_input(cx));
            type_char(view, "z", window, cx);
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"z".to_vec());
}

#[gpui::test]
fn a_relink_moves_the_pane_onto_the_new_socket_and_resets_the_mirror(cx: &mut TestAppContext) {
    let (window, mut old_daemon) = harness(cx);
    let read_row = |cx: &mut TestAppContext, len: usize| -> String {
        window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                let grid = term.grid();
                (0..len)
                    .map(|c| grid[alacritty_terminal::index::Line(0)][Column(c)].c)
                    .collect()
            })
            .unwrap()
    };

    DaemonMsg::Output(b"before".to_vec())
        .encode(&mut old_daemon)
        .unwrap();
    let mut seen = String::new();
    for _ in 0..400 {
        cx.run_until_parked();
        seen = read_row(cx, 6);
        if seen == "before" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(seen, "before", "the pre-drop screen is what we relink over");

    let (new_client, mut new_daemon) = super::test_stream_pair();
    window
        .update(cx, |view, _, cx| {
            view.adopt_relink(
                new_client,
                Vec::new(),
                &crate::PaneRoute::Local,
                TermSize::new(100, 30),
                8,
                17,
                cx,
            )
            .expect("the swap itself cannot fail");
            assert_eq!(
                view.session.title, "anterm",
                "a relinked pane is not \"process exited\""
            );
        })
        .unwrap();
    assert_ne!(
        read_row(cx, 6),
        "before",
        "the mirror must be reset before the daemon replays onto it"
    );

    let resize = loop {
        match ClientMsg::read(&mut new_daemon).expect("the new socket is live") {
            ClientMsg::Resize(win) => break win,
            _ => continue,
        }
    };
    assert_eq!((resize.cols, resize.rows), (100, 30));

    window
        .update(cx, |view, _, cx| view.send_to_pty(b"after", cx))
        .unwrap();
    assert_eq!(next_input(&mut new_daemon), b"after".to_vec());

    let mut leftovers: Vec<Vec<u8>> = Vec::new();
    loop {
        match ClientMsg::read(&mut old_daemon) {
            Ok(ClientMsg::Input(bytes)) => leftovers.push(bytes),
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(
        leftovers.is_empty(),
        "the retired socket must never see another byte: {leftovers:?}"
    );
}

#[gpui::test]
fn buffer_search_honors_case_and_regex_toggles(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Output(b"Hello World\r\nhello world\r\nWORLD wide\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();

    for _ in 0..200 {
        let ready = window
            .update(cx, |v, _, _| {
                let term = v.terminal.term.lock();
                let grid = term.grid();
                (0..grid.screen_lines() as i32)
                    .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c == 'W'))
            })
            .unwrap();
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            fn set_query(
                view: &mut TerminalView,
                q: &str,
                window: &mut Window,
                cx: &mut Context<TerminalView>,
            ) {
                let input = view.search_state.active.as_ref().unwrap().input.clone();
                input.update(cx, |s, cx| s.set_value(q, window, cx));
                view.recompute_matches(cx);
            }

            view.open_search(window, cx);
            assert!(view.search_state.active.is_some(), "Cmd+F opens the bar");

            set_query(view, "world", window, cx);
            assert_eq!(view.search_state.active.as_ref().unwrap().matches.len(), 3);
            assert!(!view.search_state.regex_error);

            view.search_state.case_sensitive = true;
            view.recompute_matches(cx);
            assert_eq!(view.search_state.active.as_ref().unwrap().matches.len(), 1);
            view.search_state.case_sensitive = false;

            set_query(view, "wor.d", window, cx);
            assert_eq!(view.search_state.active.as_ref().unwrap().matches.len(), 0);
            view.search_state.regex = true;
            view.recompute_matches(cx);
            assert_eq!(view.search_state.active.as_ref().unwrap().matches.len(), 3);

            view.search_state.regex = true;
            set_query(view, "(", window, cx);
            assert!(view.search_state.regex_error);
            assert_eq!(view.search_state.active.as_ref().unwrap().matches.len(), 0);
            view.search_state.regex = false;
            view.recompute_matches(cx);
            assert!(!view.search_state.regex_error);

            view.close_search(window, cx);
            assert_eq!(view.search_state.last_query, "(");
            assert!(view.search_state.active.is_none());
        })
        .unwrap();
}

/// A match point is an absolute (line, column) against the width it was
/// scanned at, so a column change reflows the text out from under every
/// highlight. Output rescans them, but a quiet pane has none coming —
/// the resize itself has to rescan (#586).
#[gpui::test]
fn a_column_resize_rescans_the_open_searchs_highlights(cx: &mut TestAppContext) {
    // Rooted: the open bar's input reaches for `Root` when a frame draws
    // (see `rooted_harness`).
    let (window, view, mut daemon) = rooted_harness(cx);

    // 76 columns of text: one row at 80 wide, wrapped onto two at 40.
    let mut line = vec![b'a'; 70];
    line.extend_from_slice(b"needle\r\n");
    DaemonMsg::Output(line).encode(&mut daemon).unwrap();

    for _ in 0..200 {
        let ready = cx.update(|cx| {
            let v = view.read(cx);
            let term = v.terminal.term.lock();
            let grid = term.grid();
            (0..grid.screen_lines() as i32)
                .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c == 'n'))
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.open_search(window, cx);
                let input = v.search_state.active.as_ref().unwrap().input.clone();
                input.update(cx, |s, cx| s.set_value("needle", window, cx));
                v.recompute_matches(cx);
                let before = *v.search_state.active.as_ref().unwrap().matches[0].start();
                assert_eq!(
                    (before.line.0, before.column.0),
                    (0, 70),
                    "one unwrapped row at 80 columns"
                );

                // No output, no debounce: the resize alone has to move the
                // highlight to where the reflow put the text. Where exactly
                // the wrapped half lands (next row, or the first half pushed
                // into scrollback) is the grid's business — what matters is
                // that the highlight sits on the needle, not its old row.
                v.set_grid_size(40, 24, px(8.), px(17.), 1., cx);
                let after = *v.search_state.active.as_ref().unwrap().matches[0].start();
                assert_ne!(
                    (after.line.0, after.column.0),
                    (0, 70),
                    "column 70 does not even exist at 40 wide — a stale point"
                );
                let term = v.terminal.term.lock();
                let cell = term.grid()[after.line][after.column].c;
                drop(term);
                assert_eq!(cell, 'n', "the highlight follows the reflowed text");

                // A rows-only change reflows nothing; the scan must not move.
                v.set_grid_size(40, 12, px(8.), px(17.), 1., cx);
                let still = *v.search_state.active.as_ref().unwrap().matches[0].start();
                assert_eq!(
                    (still.line.0, still.column.0),
                    (after.line.0, after.column.0)
                );
            });
        })
        .unwrap();
}

/// The composer end to end on a live pane: laid over the agent's input
/// once the input is on screen, a message leaves as one paste and then its
/// own Enter, the box steps aside — keyboard and all — while the agent
/// shows something else where its input goes, comes back when the input
/// does, and is gone when the agent quits.
#[gpui::test]
fn the_composer_stands_in_for_the_agents_input_and_steps_aside_for_its_prompts(
    cx: &mut TestAppContext,
) {
    use anterm_core::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};

    let (window, view, mut daemon) = rooted_harness(cx);
    let draw = |cx: &mut TestAppContext| {
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
    };
    let wait_for = |cx: &mut TestAppContext, want: &dyn Fn(&TerminalView) -> bool| {
        for _ in 0..200 {
            if cx.update(|cx| want(view.read(cx))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the pane never got there");
    };
    let screen_has = |v: &TerminalView, needle: char| {
        let term = v.terminal.term.lock();
        let grid = term.grid();
        (0..grid.screen_lines() as i32)
            .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c == needle))
    };
    // Let the window lay the grid out first: the agent draws for the size
    // it is given, as Claude Code does after every resize.
    draw(cx);
    let (cols, lines) = cx.update(|cx| {
        let term = view.read(cx).terminal.term.lock();
        (term.columns(), term.screen_lines())
    });
    let rule = "─".repeat(cols);
    // Claude Code's input, as it draws it: the prompt between two rules
    // at the bottom of the screen, its status row under them.
    let input = format!(
        "\x1b[2J\x1b[H⏺ Ready.\x1b[{};1H{rule}\x1b[{};1H❯ \x1b[{};1H{rule}\x1b[{};1H  ? for shortcuts",
        lines - 3,
        lines - 2,
        lines - 1,
        lines,
    );

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| v.toggle_composer(window, cx));
        })
        .unwrap();
    assert!(
        cx.update(|cx| view.read(cx).composer_state.active.is_none()),
        "a shell has nobody to compose for"
    );

    DaemonMsg::Agent(Some(CLIAgent::Claude))
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Output(format!("\x1b[?2004h{input}").into_bytes())
        .encode(&mut daemon)
        .unwrap();
    wait_for(cx, &|v| {
        v.agent() == Some(CLIAgent::Claude)
            && screen_has(v, '❯')
            && v.terminal
                .term
                .lock()
                .mode()
                .contains(TermMode::BRACKETED_PASTE)
    });
    let rows = cx.update(|cx| view.read(cx).terminal.size().rows);

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| v.toggle_composer(window, cx));
        })
        .unwrap();
    draw(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                assert!(
                    v.composer_shown(),
                    "the input is on screen, so the box is over it"
                );
                let input = v.composer_state.active.as_ref().unwrap().input.clone();
                assert!(
                    input.read(cx).focus_handle(cx).is_focused(window),
                    "opening the box puts the caret in it"
                );
                input.update(cx, |s, cx| s.set_value("one\ntwo", window, cx));
            });
        })
        .unwrap();
    assert_eq!(
        cx.update(|cx| view.read(cx).terminal.size().rows),
        rows,
        "laid over the input, the box leaves the grid its rows"
    );

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| v.submit_composer(window, cx));
        })
        .unwrap();
    draw(cx);
    assert_eq!(next_input(&mut daemon), b"\x1b[200~one\ntwo\x1b[201~");
    assert_eq!(
        next_input(&mut daemon),
        b"\r",
        "Enter goes as its own write"
    );
    assert_eq!(
        cx.update(|cx| view
            .read(cx)
            .composer_state
            .active
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .value()),
        "",
        "a sent message leaves the box"
    );

    // A click on the grid takes the keyboard out of the box, and it
    // stays out: the box does not pull it back on the next frame.
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| window.focus(&v.focus_handle, cx));
        })
        .unwrap();
    draw(cx);
    draw(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, _| {
                assert!(
                    v.focus_handle.is_focused(window),
                    "the keyboard stays where the click put it"
                );
                assert!(!v.composer_state.focused);
            });
        })
        .unwrap();

    // A paste at the grid is for the input under the box: it goes into
    // the box, and the keyboard with it — never unseen into the agent.
    view.update(cx, |v, cx| v.paste("pasted\nhere".into(), cx));
    draw(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                let input = v.composer_state.active.as_ref().unwrap().input.clone();
                assert_eq!(input.read(cx).value(), "pasted\nhere");
                assert!(input.read(cx).focus_handle(cx).is_focused(window));
                input.update(cx, |s, cx| s.set_value("", window, cx));
            });
        })
        .unwrap();

    // A picker takes the input's place. Past the grace period the box
    // steps aside and the keyboard goes to the TUI.
    DaemonMsg::Output(
        b"\x1b[2J\x1b[H Select model\r\n \xe2\x9d\xaf 1. Opus\r\n   2. Sonnet".to_vec(),
    )
    .encode(&mut daemon)
    .unwrap();
    wait_for(cx, &|v| !screen_has(v, '─'));
    draw(cx);
    std::thread::sleep(super::super::composer::STEP_ASIDE_AFTER);
    draw(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, _| {
                assert!(!v.composer_shown(), "the picker is not covered");
                assert!(
                    v.focus_handle.is_focused(window),
                    "the picker has the keyboard"
                );
            });
        })
        .unwrap();

    // The input comes back, and so do the box and its caret.
    DaemonMsg::Output(input.clone().into_bytes())
        .encode(&mut daemon)
        .unwrap();
    wait_for(cx, &|v| screen_has(v, '─'));
    draw(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                assert!(v.composer_shown());
                let input = v.composer_state.active.as_ref().unwrap().input.clone();
                assert!(input.read(cx).focus_handle(cx).is_focused(window));
            });
        })
        .unwrap();

    // A question the hooks report steps it aside at once, and nothing in
    // the box is sent as the answer.
    DaemonMsg::AgentStatus(Some(AgentSessionState {
        status: AgentStatus::Waiting,
        rich: true,
        ..Default::default()
    }))
    .encode(&mut daemon)
    .unwrap();
    wait_for(cx, &|v| {
        v.agent_session()
            .is_some_and(|s| s.status == AgentStatus::Waiting)
    });
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                let input = v.composer_state.active.as_ref().unwrap().input.clone();
                input.update(cx, |s, cx| s.set_value("yes", window, cx));
                v.submit_composer(window, cx);
            });
        })
        .unwrap();
    draw(cx);
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "a question in the TUI is not answered from the box"
    );
    assert!(cx.update(|cx| !view.read(cx).composer_shown()));
    assert_eq!(
        cx.update(|cx| view
            .read(cx)
            .composer_state
            .active
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .value()),
        "yes",
        "the held message stays in the box"
    );

    DaemonMsg::Agent(None).encode(&mut daemon).unwrap();
    wait_for(cx, &|v| v.agent().is_none());
    draw(cx);
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, _| {
                assert!(!v.composer_shown(), "no agent, no box");
                assert!(
                    v.focus_handle.is_focused(window),
                    "the keyboard goes back to the terminal"
                );
            });
        })
        .unwrap();
}

/// The selection that seeds the search query is the thing being searched
/// for — opening the bar must not erase it, and closing the bar must not
/// either (#584). Only *changing* the query retires it.
#[gpui::test]
fn the_search_bar_opens_and_closes_around_a_grid_selection(cx: &mut TestAppContext) {
    let (window, view, mut daemon) = rooted_harness(cx);

    DaemonMsg::Output(b"some needle in the haystack\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        let ready = cx.update(|cx| {
            let v = view.read(cx);
            let term = v.terminal.term.lock();
            let grid = term.grid();
            (0..grid.screen_lines() as i32)
                .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c == 'n'))
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                // Select "needle" (row 0, columns 5..=10 — the end side
                // includes its cell) — the seed the bar picks up.
                let mut sel = Selection::new(
                    SelectionType::Simple,
                    Point::new(Line(0), Column(5)),
                    Side::Left,
                );
                sel.update(Point::new(Line(0), Column(10)), Side::Right);
                v.terminal.term.lock().selection = Some(sel);

                v.open_search(window, cx);
                assert_eq!(
                    v.search_state.active.as_ref().unwrap().input.read(cx).value(),
                    "needle",
                    "the bar opens on the selection as its query"
                );
                assert!(
                    v.terminal.term.lock().selection.is_some(),
                    "opening the bar must not eat the selection that seeded it"
                );

                v.close_search(window, cx);
                assert!(
                    v.terminal.term.lock().selection.is_some(),
                    "closing the bar must not eat it either"
                );

                // But a query the user *changed* retires the old selection:
                // it no longer names what the search is about.
                v.open_search(window, cx);
                let input = v.search_state.active.as_ref().unwrap().input.clone();
                input.update(cx, |s, cx| s.set_value("haystack", window, cx));
                v.recompute_matches(cx);
                assert!(
                    v.terminal.term.lock().selection.is_none(),
                    "a changed query retires the stale selection"
                );
            });
        })
        .unwrap();
}

#[gpui::test]
fn output_under_an_open_search_bar_is_searched_too(cx: &mut TestAppContext) {
    let (window, view, mut daemon) = rooted_harness(cx);

    fn wait_for(cx: &mut TestAppContext, view: &Entity<TerminalView>, needle: char) {
        for _ in 0..200 {
            let seen = cx.update(|cx| {
                let v = view.read(cx);
                let term = v.terminal.term.lock();
                let grid = term.grid();
                (0..grid.screen_lines() as i32)
                    .any(|l| (0..grid.columns()).any(|c| grid[Line(l)][Column(c)].c == needle))
            });
            if seen {
                return;
            }
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the pane never printed {needle:?}");
    }

    DaemonMsg::Output(b"world 1\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    wait_for(cx, &view, '1');

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.open_search(window, cx);
                let input = v.search_state.active.as_ref().unwrap().input.clone();
                input.update(cx, |s, cx| s.set_value("world", window, cx));
                v.recompute_matches(cx);
                assert_eq!(v.search_state.active.as_ref().unwrap().matches.len(), 1);
            });
        })
        .unwrap();

    // A second line arrives while the bar is up. Until it is rescanned the
    // count still says 1, and the one highlight it does draw has slid onto
    // whatever line took the old one's place.
    DaemonMsg::Output(b"world 2\r\n".to_vec())
        .encode(&mut daemon)
        .unwrap();
    wait_for(cx, &view, '2');
    for _ in 0..8 {
        cx.executor()
            .advance_clock(super::super::search::SCAN_DEBOUNCE * 2);
        cx.run_until_parked();
    }

    cx.update(|cx| {
        let v = view.read(cx);
        let search = v.search_state.active.as_ref().expect("the bar is still open");
        assert_eq!(
            search.matches.len(),
            2,
            "the line printed under the open bar has to be counted too"
        );
        assert_eq!(
            search.current_index,
            Some(0),
            "and the match the user was standing on is still the one they are on"
        );
        assert!(
            !v.search_state.scan_armed,
            "the debounce has to settle, not rescan forever"
        );
    });
}

/// Print lines into the grid and post the wakeup the reader thread would
/// have posted behind them.
///
/// Going through the parser directly rather than the socket keeps the two
/// tests below off both real time and the clock: the pane has printed by
/// the time this returns, and no timer has had a chance to fire.
fn print(cx: &mut TestAppContext, view: &Entity<TerminalView>, lines: Vec<String>) {
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            {
                use alacritty_terminal::vte::ansi::Handler as _;
                let mut term = v.terminal.term.lock();
                for line in lines {
                    for c in line.chars() {
                        term.input(c);
                    }
                    term.carriage_return();
                    term.linefeed();
                }
            }
            v.handle_event(AlacEvent::Wakeup, cx);
        })
    });
}

/// Filler that scrolls the grid without ever matching the query.
fn filler(lines: usize) -> Vec<String> {
    (0..lines).map(|i| format!("filler {i}")).collect()
}

/// What the grid holds where the current match says its word starts, so a
/// test can ask the question the eye asks: is the highlight still on it?
fn word_at_the_match(
    cx: &mut TestAppContext,
    view: &Entity<TerminalView>,
    len: usize,
) -> String {
    cx.update(|cx| {
        let v = view.read(cx);
        let start = *v
            .search_state
            .active
            .as_ref()
            .and_then(|s| s.current())
            .expect("a match to stand on")
            .start();
        let term = v.terminal.term.lock();
        let grid = term.grid();
        (0..len)
            .map(|i| grid[start.line][Column(start.column.0 + i)].c)
            .collect()
    })
}

/// Print the searched-for word and bury it in the scrollback, then open the
/// bar on it. Returns with exactly one match, sitting in history.
fn search_a_buried_word(
    window: &gpui::WindowHandle<gpui_kit::component::Root>,
    cx: &mut TestAppContext,
    view: &Entity<TerminalView>,
) {
    let rows = cx.update(|cx| view.read(cx).terminal.term.lock().grid().screen_lines());
    print(cx, view, vec!["world 1".to_string()]);
    print(cx, view, filler(rows + 8));
    cx.update(|cx| {
        assert!(
            view.read(cx).terminal.term.lock().grid().history_size() > 0,
            "the word has to be in the scrollback for the grid to scroll under it"
        );
    });

    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.open_search(window, cx);
                let input = v.search_state.active.as_ref().unwrap().input.clone();
                input.update(cx, |s, cx| s.set_value("world", window, cx));
                v.recompute_matches(cx);
                assert_eq!(v.search_state.active.as_ref().unwrap().matches.len(), 1);
            });
        })
        .unwrap();
}

/// Drawing must never queue for the grid lock.
///
/// One UI thread paints every pane in every window, and the thread holding
/// this lock is the pane's own reader part-way through feeding a batch of
/// output into the emulator. A draw that waited for it would wire one
/// pane's write speed to the frame rate of the whole window — the read-side
/// twin of #709, which was this same thread parked in `write(2)`.
///
/// No second thread and no timing: `try_lock` fails against a lock this
/// thread already holds, so "the reader has it" is reproduced exactly, with
/// nothing to race. The cost of that trade is what a regression looks like
/// — put `lock()` back and this test hangs on the re-entry rather than
/// failing, which reads as a CI timeout on exactly this name.
#[gpui::test]
fn a_frame_that_cannot_have_the_grid_leaves_the_previous_one_alone(cx: &mut TestAppContext) {
    use super::super::element::PaintColors;

    let (_window, view, _daemon) = rooted_harness(cx);
    let element = TerminalElement::new(view.clone());
    let mut buf = Vec::new();
    let build = |cx: &mut TestAppContext, buf: &mut Vec<RenderCell>, must_block: bool| {
        cx.update(|cx| {
            let colors = PaintColors::resolve(cx.theme(), cx);
            element.build_grid(
                &colors,
                buf,
                24,
                80,
                false,
                cx,
                1.,
                gpui::Rgba::default(),
                must_block,
            )
        })
    };

    assert!(
        build(cx, &mut buf, true).is_some(),
        "the first frame has no previous grid to stand in for it, so it waits and builds"
    );
    assert_eq!(buf.len(), 24 * 80);

    // Shortened so the next call cannot touch the buffer without saying so:
    // building would `clear` and `resize` it back to a full grid.
    buf.truncate(3);
    let term = cx.update(|cx| view.read(cx).terminal.term.clone());
    let held = term.lock();
    let refused = build(cx, &mut buf, false);
    drop(held);

    assert!(
        refused.is_none(),
        "a frame that cannot have the lock says so instead of waiting for it"
    );
    assert_eq!(
        buf.len(),
        3,
        "the previous frame's cells have to survive for that frame to be painted again"
    );

    assert!(
        build(cx, &mut buf, false).is_some(),
        "with the lock free, a frame builds without being told to wait"
    );
    assert_eq!(buf.len(), 24 * 80);
}

#[gpui::test]
fn a_highlight_follows_its_text_as_output_scrolls_under_it(cx: &mut TestAppContext) {
    let (window, view, _daemon) = rooted_harness(cx);
    search_a_buried_word(&window, cx, &view);
    assert_eq!(
        word_at_the_match(cx, &view, 5),
        "world",
        "the scan has to name its own word"
    );

    // Output arrives and the clock never moves, so nothing rescans: the
    // stored point is all the highlight has to go on. Every line printed
    // here slid the grid up under it, and it has to have come along.
    print(cx, &view, filler(7));
    cx.update(|cx| {
        assert!(
            view.read(cx).search_state.scan_armed,
            "the rescan is still waiting for the pane to go quiet"
        );
    });
    assert_eq!(
        word_at_the_match(cx, &view, 5),
        "world",
        "a match point has to name the row its text scrolled to, \
         not the row it was read from"
    );
}

#[gpui::test]
fn a_pane_that_never_pauses_is_rescanned_anyway(cx: &mut TestAppContext) {
    let (window, view, _daemon) = rooted_harness(cx);
    search_a_buried_word(&window, cx, &view);

    // A pane mid-flood: something new lands every half debounce window, so
    // the pause the rescan is waiting for never comes.
    for i in 0..(super::super::search::SCAN_LAG_LIMIT * 2 + 2) {
        print(cx, &view, vec![format!("world {}", i + 2)]);
        cx.executor()
            .advance_clock(super::super::search::SCAN_DEBOUNCE / 2);
        cx.run_until_parked();
    }

    cx.update(|cx| {
        let search = view
            .read(cx)
            .search_state
            .active
            .as_ref()
            .expect("the bar is still open");
        assert!(
            search.matches.len() > 1,
            "printing without a pause must not hold the scan off forever, \
             got {} matches",
            search.matches.len()
        );
    });
}

#[gpui::test]
fn child_exit_marks_the_view_exited(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.handle_event(AlacEvent::Exit, cx);
            assert!(view.terminal.exited);
            assert_eq!(view.session.title, "anterm — process exited");
        })
        .unwrap();
}

/// Holding Backspace on an empty bash prompt (or Tab with nothing to
/// complete) rings the bell at key-repeat rate. Every flash used to arm its
/// own clear timer, so the first bell's timer blanked a flash the fifth bell
/// had just re-lit, and the pane strobed for as long as the key was held
/// (#874).
#[gpui::test]
fn a_bell_rung_at_key_repeat_rate_holds_one_steady_flash(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    let lit =
        |cx: &mut TestAppContext| window.update(cx, |view, _, _| view.render.bell_flash).unwrap();

    let repeat = std::time::Duration::from_millis(33);
    let mut dark = Vec::new();
    for i in 0..30 {
        window
            .update(cx, |view, _, cx| view.handle_event(AlacEvent::Bell, cx))
            .unwrap();
        cx.executor().advance_clock(repeat);
        cx.run_until_parked();
        if !lit(cx) {
            dark.push(i);
        }
    }
    assert!(
        dark.is_empty(),
        "the flash went dark between bells after repeats {dark:?}"
    );

    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert!(!lit(cx), "the flash outlived the last bell");
}

#[gpui::test]
fn text_area_size_request_replies_with_the_current_geometry(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    let want = window
        .update(cx, |view, _, cx| {
            let size = view.terminal.size();
            let fmt = std::sync::Arc::new(|ws: alacritty_terminal::event::WindowSize| {
                format!("{}x{}", ws.num_cols, ws.num_lines)
            });
            view.handle_event(AlacEvent::TextAreaSizeRequest(fmt), cx);
            format!("{}x{}", size.cols, size.rows)
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), want.into_bytes());
}

#[gpui::test]
fn daemon_output_reaches_the_grid_through_the_event_pump(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    let read_row = |cx: &mut TestAppContext, len: usize| -> String {
        window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                let grid = term.grid();
                (0..len)
                    .map(|c| grid[alacritty_terminal::index::Line(0)][Column(c)].c)
                    .collect()
            })
            .unwrap()
    };
    let wait_for = |cx: &mut TestAppContext, want: &str| {
        let mut got = String::new();
        for _ in 0..400 {
            cx.run_until_parked();
            got = read_row(cx, want.chars().count());
            if got == want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        got
    };

    DaemonMsg::Output(b"hello".to_vec())
        .encode(&mut daemon)
        .unwrap();
    assert_eq!(wait_for(cx, "hello"), "hello");

    DaemonMsg::Output(b" again".to_vec())
        .encode(&mut daemon)
        .unwrap();
    assert_eq!(wait_for(cx, "hello again"), "hello again");
}

#[gpui::test]
fn copy_on_select_writes_the_clipboard_at_mouse_up(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Output(b"hello world".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let row: String = window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                (0..11)
                    .map(|c| term.grid()[alacritty_terminal::index::Line(0)][Column(c)].c)
                    .collect()
            })
            .unwrap();
        if row == "hello world" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let drag_hello = |cx: &mut TestAppContext| {
        window
            .update(cx, |view, _, cx| {
                view.on_select_start(0, 0, true, 1, false, cx);
                view.on_select_update(4, 0, false, cx);
                view.on_select_end(cx);
            })
            .unwrap();
    };
    drag_hello(cx);
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard()),
        None,
        "default-off must never write the clipboard"
    );

    cx.update(|cx| cx.update_global::<Config, _>(|cfg, _| cfg.copy_on_select = true));
    drag_hello(cx);
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("hello"));

    let selected = window
        .update(cx, |view, _, _| {
            view.terminal.term.lock().selection.is_some()
        })
        .unwrap();
    assert!(
        selected,
        "copy-on-select must keep the selection highlighted"
    );
}

#[gpui::test]
fn ctrl_c_copy_consumes_the_selection_so_the_next_press_is_sigint(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Output(b"hello world".to_vec())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let row: String = window
            .update(cx, |view, _, _| {
                let term = view.terminal.term.clone();
                let term = term.lock();
                (0..11)
                    .map(|c| term.grid()[alacritty_terminal::index::Line(0)][Column(c)].c)
                    .collect()
            })
            .unwrap();
        if row == "hello world" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            view.on_select_start(0, 0, true, 1, false, cx);
            view.on_select_update(4, 0, false, cx);
            view.on_select_end(cx);
            assert!(view.has_selection(), "the drag must leave a selection");

            let consumed = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(consumed, CmdKey::Consumed));
            assert!(
                !view.has_selection(),
                "the Ctrl+C copy must consume the selection"
            );

            let fell_through = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(fell_through, CmdKey::FallThrough));
        })
        .unwrap();
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("hello"));
}

#[gpui::test]
fn cmd_v_pastes_on_the_alternate_screen(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, window, cx| {
            let pasted = view.handle_cmd_shortcut(&key("cmd-v"), window, cx);
            assert!(
                matches!(pasted, CmdKey::Consumed),
                "Cmd+V carries no control code and pastes on every screen"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
    assert_eq!(next_input_until_timeout(&mut daemon), None);
}

/// The Ctrl+V half of the same key, through the real keymap: whether it
/// pastes is the `AlternatePaste` binding's decision, not this view's, so
/// these two drive it the way a user does rather than calling in.
#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn ctrl_v_pastes_at_a_prompt(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    window
        .update(cx, |view, window, cx| {
            assert!(!view.on_alt_screen());
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-v");

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"echo hi".to_vec())
    );
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn ctrl_v_reaches_a_full_screen_program_as_syn(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));
    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-v");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x16]));
}

/// The other half of that rule, which the binding's context cannot state.
///
/// gpui matches a keystroke against the frame it last *painted*, so
/// `!alt_screen` outlives the switch by a frame: launch a full-screen
/// program and hit Ctrl+V before the next paint and the keymap still
/// believes the pane is at a prompt. The action asks the grid itself, so
/// the clipboard never lands in a program that would run it as commands.
#[gpui::test]
fn alternate_paste_asks_the_grid_and_not_the_last_frame(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("echo hi".into())));

    window
        .update(cx, |view, _window, cx| {
            assert!(!view.on_alt_screen());
            view.alternate_paste(cx);
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());

    alt_screen_ready(&window, cx, &mut daemon);
    window
        .update(cx, |view, _window, cx| view.alternate_paste(cx))
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        None,
        "the paste is withheld even when the frame that matched said otherwise"
    );
}

/// `Ctrl-^`, which used to die in a hardcoded block that swallowed every
/// Ctrl+digit off macOS — nothing has claimed those chords since tabs
/// moved to Alt+1..9.
#[gpui::test]
fn ctrl_6_reaches_the_pty_as_rs(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    cx.update(|cx| anterm_ui_kit::keymap::init(cx));
    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_keystrokes("ctrl-6");

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x1e]));
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn cmd_backspace_reaches_a_foreground_tui_as_ctrl_u(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, window, cx| {
            assert!(
                !view.input_active(),
                "the foreground process, not anterm's editor, owns input"
            );
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        modifiers: Modifiers {
                            platform: true,
                            ..Modifiers::default()
                        },
                        key: "backspace".into(),
                        key_char: None,
                    },
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();

    assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![0x15]));
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn cmd_navigation_reaches_a_foreground_tui_as_readline_controls(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);

    for (key, expected) in [("left", 0x01), ("right", 0x05), ("delete", 0x0b)] {
        window
            .update(cx, |view, window, cx| {
                assert!(!view.input_active(), "the foreground TUI owns input");
                view.on_key_down(
                    &KeyDownEvent {
                        keystroke: gpui::Keystroke {
                            modifiers: Modifiers {
                                platform: true,
                                ..Modifiers::default()
                            },
                            key: key.into(),
                            key_char: None,
                        },
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .unwrap();
        assert_eq!(next_input_until_timeout(&mut daemon), Some(vec![expected]));
    }
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn cmd_backspace_releases_held_input_before_ctrl_u(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let gap = window
            .update(cx, |view, _, _| {
                view.terminal.shell_active() && !view.terminal.at_prompt()
            })
            .unwrap();
        if gap {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            view.commit_text("ls", cx);
            view.on_key_down(
                &KeyDownEvent {
                    keystroke: gpui::Keystroke {
                        modifiers: Modifiers {
                            platform: true,
                            ..Modifiers::default()
                        },
                        key: "backspace".into(),
                        key_char: None,
                    },
                    is_held: false,
                    prefer_character_input: false,
                },
                window,
                cx,
            );
        })
        .unwrap();

    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(b"ls".to_vec()),
        "held text must reach the PTY before the line-kill"
    );
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(vec![0x15]),
        "Ctrl-U must follow the text it clears"
    );
}

#[gpui::test]
fn paste_to_the_pty_consumes_the_selection(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.select_all(cx);
            assert!(view.has_selection());
            view.paste("echo hi".into(), cx);
            assert!(
                !view.has_selection(),
                "a PTY paste must consume the selection"
            );
        })
        .unwrap();
    assert_eq!(next_input(&mut daemon), b"echo hi".to_vec());
}

#[gpui::test]
fn ctrl_c_copy_consumes_the_editor_selection_at_the_prompt(cx: &mut TestAppContext) {
    let (window, mut daemon) = harness(cx);

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    for _ in 0..400 {
        cx.run_until_parked();
        let active = window.update(cx, |view, _, _| view.input_active()).unwrap();
        if active {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    window
        .update(cx, |view, window, cx| {
            assert!(view.input_active(), "the inline editor must be active");
            view.prompt.cmd.insert_str("echo hi");
            view.prompt.cmd.select_all();

            let consumed = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(consumed, CmdKey::Consumed));
            assert!(
                view.prompt.cmd.selection().is_none(),
                "the Ctrl+C copy must consume the editor selection"
            );

            let fell_through = view.handle_cmd_shortcut(&key("ctrl-c"), window, cx);
            assert!(matches!(fell_through, CmdKey::FallThrough));
        })
        .unwrap();
    let text = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("echo hi"));
}

#[gpui::test]
fn hidden_cursor_at_prompt_anchors_the_editor_at_the_real_cell_not_top_left(
    cx: &mut TestAppContext,
) {
    use alacritty_terminal::vte::ansi::CursorShape;

    let (window, mut daemon) = harness(cx);

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(0),
    }
    .encode(&mut daemon)
    .unwrap();
    DaemonMsg::Output(b"\x1b[4;11H\x1b[?25l".to_vec())
        .encode(&mut daemon)
        .unwrap();

    let mut state = (false, false, None);
    for _ in 0..400 {
        cx.run_until_parked();
        state = window
            .update(cx, |view, _, _| {
                let hidden = matches!(
                    view.terminal.term.lock().renderable_content().cursor.shape,
                    CursorShape::Hidden
                );
                (view.input_active(), hidden, view.cursor_cell())
            })
            .unwrap();
        if state == (true, true, Some((3, 10))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let (active, hidden, cell) = state;
    assert!(
        active,
        "shell at its prompt must make the inline editor active"
    );
    assert!(
        hidden,
        "the TUI's `?25l` must leave the cursor shape Hidden"
    );
    assert_eq!(
        cell,
        Some((3, 10)),
        "a Hidden shape must not collapse the editor anchor to the top-left corner"
    );
}

/// #844: a TUI that resets DECTCEM and draws its own reverse-video caret
/// gets no terminal caret painted over it — focused, unfocused, and after
/// a re-attach replays its screen — and `?25h` brings the caret back.
///
/// The stream is the reporter's shape: an alternate screen and 69 `?25l`
/// interleaved with 75 `?25h`, the last one a hide. What is checked is the
/// snapshot a real paint left behind, through the same `painted_cursor`
/// the element paints from.
#[gpui::test]
fn dectcem_reset_paints_no_terminal_caret_over_the_tuis_own(cx: &mut TestAppContext) {
    use anterm_ui_kit::config::CursorStyle;

    // An Ink-style frame: our own caret as one reverse-video cell at
    // row 5 column 13, and the real cursor parked on it.
    const FRAME: &[u8] = b"\x1b[5;1H\x1b[2K> type here \x1b[7m \x1b[27m\x1b[5;13H";
    let mut stream = b"\x1b[?1049h".to_vec();
    stream.extend(std::iter::repeat_n(&b"\x1b[?25h"[..], 7).flatten());
    for _ in 0..68 {
        stream.extend_from_slice(b"\x1b[?25l");
        stream.extend_from_slice(FRAME);
        stream.extend_from_slice(b"\x1b[?25h");
    }
    stream.extend_from_slice(b"\x1b[?25l");
    stream.extend_from_slice(FRAME);
    assert_eq!(stream.windows(6).filter(|w| w == b"\x1b[?25l").count(), 69);
    assert_eq!(stream.windows(6).filter(|w| w == b"\x1b[?25h").count(), 75);

    type Painted = Option<(usize, usize, CursorStyle)>;
    // Paints frames until the one the pane settles on matches `want`, and
    // returns the last painted caret either way.
    let paint_until = |window: &gpui::WindowHandle<TerminalView>,
                       cx: &mut TestAppContext,
                       focused: bool,
                       want: &dyn Fn(Painted) -> bool| {
        let mut painted = None;
        for _ in 0..400 {
            window
                .update(cx, |view, window, cx| {
                    if focused {
                        window.activate_window();
                        view.focus_handle.focus(window, cx);
                    } else {
                        window.blur(cx);
                    }
                    cx.notify();
                })
                .unwrap();
            let mut vcx = gpui::VisualTestContext::from_window((*window).into(), cx);
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let (text, snap) = window
                .update(cx, |view, window, _| {
                    assert_eq!(view.focus_handle.is_focused(window), focused);
                    use alacritty_terminal::grid::Dimensions as _;
                    use alacritty_terminal::index::{Column, Line};
                    let term = view.terminal.term.lock();
                    let row = &term.grid()[Line(4)];
                    let text = (0..term.grid().columns())
                        .map(|col| row[Column(col)].c)
                        .collect::<String>();
                    (text, view.render.grid_snap.as_ref().map(|s| s.painted_cursor()))
                })
                .unwrap();
            if text.starts_with("> type here")
                && let Some(p) = snap
            {
                painted = p;
                if want(p) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        painted
    };

    let (window, mut daemon) = harness(cx);
    DaemonMsg::Output(stream.clone())
        .encode(&mut daemon)
        .unwrap();
    let focused = paint_until(&window, cx, true, &|p| p.is_none());
    assert_eq!(
        focused, None,
        "a focused pane painted its caret over a TUI that reset DECTCEM"
    );
    let unfocused = paint_until(&window, cx, false, &|p| p.is_none());
    assert_eq!(
        unfocused, None,
        "an unfocused pane painted its hollow caret over a TUI that reset DECTCEM"
    );

    DaemonMsg::Output(b"\x1b[?25h".to_vec())
        .encode(&mut daemon)
        .unwrap();
    let shown = paint_until(&window, cx, true, &|p| p.is_some());
    assert_eq!(
        shown.map(|(row, col, _)| (row, col)),
        Some((4, 12)),
        "`?25h` must bring the caret back where the program parked it"
    );

    // Switching back to the tab: a brand new view replays the screen the
    // daemon kept, then the prompt state, which says a program is running.
    let (window, mut daemon) = harness(cx);
    DaemonMsg::Snapshot(stream).encode(&mut daemon).unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    let replayed = paint_until(&window, cx, true, &|p| p.is_none());
    assert_eq!(
        replayed, None,
        "a re-attached pane painted its caret over a TUI that reset DECTCEM"
    );
}

#[gpui::test]
fn child_exit_emits_the_close_event_but_disconnect_does_not(cx: &mut TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;

    let subscribe = |window: &gpui::WindowHandle<TerminalView>, cx: &mut TestAppContext| {
        let got = Rc::new(Cell::new(false));
        let seen = got.clone();
        window
            .update(cx, |_, _, cx| {
                let this = cx.entity();
                cx.subscribe(&this, move |_, _, _: &ChildExited, _| seen.set(true))
                    .detach();
            })
            .unwrap();
        got
    };
    let wait_exited = |window: &gpui::WindowHandle<TerminalView>, cx: &mut TestAppContext| {
        for _ in 0..400 {
            cx.run_until_parked();
            let exited = window
                .update(cx, |view, _, _| view.terminal.exited)
                .unwrap();
            if exited {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the view never noticed the exit");
    };

    let (window, mut daemon) = harness(cx);
    let got = subscribe(&window, cx);
    DaemonMsg::Exited { code: Some(0) }
        .encode(&mut daemon)
        .unwrap();
    wait_exited(&window, cx);
    assert!(got.get(), "a genuine child exit must emit ChildExited");

    let (window, daemon) = harness(cx);
    let got = subscribe(&window, cx);
    drop(daemon);
    wait_exited(&window, cx);
    assert!(!got.get(), "a daemon disconnect must not emit ChildExited");
}

#[gpui::test]
fn ssh_drop_mid_tui_recovers_at_the_next_prompt(cx: &mut TestAppContext) {
    use alacritty_terminal::vte::ansi::CursorShape;

    let (window, mut daemon) = harness(cx);

    DaemonMsg::Output(b"\x1b[?1049h\x1b[?25l".to_vec())
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: Some(255),
    }
    .encode(&mut daemon)
    .unwrap();

    let mut state = (false, true, true);
    for _ in 0..400 {
        cx.run_until_parked();
        state = window
            .update(cx, |view, _, _| {
                let hidden = matches!(
                    view.terminal.term.lock().renderable_content().cursor.shape,
                    CursorShape::Hidden
                );
                (view.at_shell_prompt(), view.on_alt_screen(), hidden)
            })
            .unwrap();
        if state == (true, false, false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let (at_prompt, on_alt, hidden) = state;
    assert!(at_prompt, "the host shell is back at its prompt");
    assert!(
        !on_alt,
        "the prompt report must pull the grid off the stranded alt screen"
    );
    assert!(
        !hidden,
        "the prompt report must re-show the DECTCEM-hidden cursor"
    );

    window
        .update(cx, |view, _, _| {
            assert!(
                view.input_active(),
                "off the alt screen and at the prompt, the editor is live"
            );
        })
        .unwrap();
}

#[gpui::test]
fn generator_results_merge_into_the_open_menu(cx: &mut TestAppContext) {
    use crate::prompt::generator::Parsed;

    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set_with_cursor("git checkout ma", 15);
            let session = CompletionSession::new(13, String::new(), Vec::new(), 1);
            let generation = view.open_completion(session);

            let results = vec![
                Parsed {
                    text: "main".into(),
                    description: Some("branch".into()),
                },
                Parsed {
                    text: "mainline".into(),
                    description: Some("branch".into()),
                },
                Parsed {
                    text: "feature".into(),
                    description: None,
                },
            ];
            view.completion_merge(generation, results, cx);

            let s = view.prompt.completion.as_ref().expect("menu still open");
            let shown: Vec<&str> = s.filtered.iter().map(|&i| s.all[i].text.as_str()).collect();
            assert_eq!(shown, vec!["main", "mainline"]);
            assert_eq!(s.selected().unwrap().text, "main");
        })
        .unwrap();
}

#[gpui::test]
fn generator_result_for_a_closed_menu_is_dropped(cx: &mut TestAppContext) {
    use crate::prompt::generator::Parsed;

    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set_with_cursor("git checkout ", 13);
            let session = CompletionSession::new(13, String::new(), Vec::new(), 1);
            let stale = view.open_completion(session);
            view.close_completion();

            view.completion_merge(
                stale,
                vec![Parsed {
                    text: "main".into(),
                    description: None,
                }],
                cx,
            );
            assert!(
                view.prompt.completion.is_none(),
                "a result for a closed session never reopens the menu"
            );

            let fresh =
                view.open_completion(CompletionSession::new(13, String::new(), Vec::new(), 1));
            assert_ne!(stale, fresh);
            view.completion_merge(
                stale,
                vec![Parsed {
                    text: "main".into(),
                    description: None,
                }],
                cx,
            );
            let s = view.prompt.completion.as_ref().unwrap();
            assert!(
                s.all.is_empty(),
                "the stale result stayed out of the new menu"
            );
        })
        .unwrap();
}

/// #541: the history menu floats over live grid cells, and a `div` that
/// carries no handler of its own inserts no hitbox — so the press went
/// straight through to the grid behind the menu, which cleared the
/// selection, dragged out a new one, or (with the link modifier held)
/// opened whatever link the menu was covering.
///
/// The second half is the other side of the pair `src/ui/app.rs` keeps for
/// the resize handle: with the menu gone the very same press must still
/// reach the grid, or this would pass on a pane that never sees a mouse.
#[gpui::test]
fn a_press_on_the_history_menu_never_reaches_the_grid(cx: &mut TestAppContext) {
    use gpui::{MouseMoveEvent, PlatformInput};

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    wait_for_input_active(&window, cx);

    window
        .update(cx, |view, window, cx| {
            window.activate_window();
            view.focus_handle.focus(window, cx);
            seed_history(view, &["echo one", "echo two"], cx);
            view.start_reverse_search(cx);
            cx.notify();
        })
        .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();

    // The menu is laid out one row under the cursor, plus the gap
    // `render_reverse_search_menu` leaves; this lands in the middle of its
    // first row, where a candidate is drawn.
    let lh = window.update(cx, |view, _, _| view.render.line_height).unwrap();
    let at = point(
        px(GRID_PAD_X) + px(10.),
        px(GRID_PAD_Y) + lh * 2.0 + px(10.),
    );

    let press = |vcx: &mut gpui::VisualTestContext| {
        vcx.update(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at,
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }),
                cx,
            );
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
        });
        vcx.run_until_parked();
    };

    press(&mut vcx);
    window
        .update(cx, |view, _, _| {
            assert!(
                view.prompt.reverse_search.is_some(),
                "the press must not have closed the menu either"
            );
            assert!(
                !view.pointer.selecting,
                "the menu swallowed the press, so no selection started under it"
            );
        })
        .unwrap();

    window
        .update(cx, |view, _, cx| {
            view.prompt.reverse_search = None;
            cx.notify();
        })
        .unwrap();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();

    press(&mut vcx);
    window
        .update(cx, |view, _, _| {
            assert!(
                view.pointer.selecting,
                "with no menu over it the same press is the grid's to take"
            );
        })
        .unwrap();
}

/// Quick launch counts an agent each time one starts. Reattaching to a pane
/// whose agent was already running — every agent tab after an app restart
/// — is not a start, and counting it bumped every agent once per launch of
/// the app. Once that pane's agent exits, the next one is a start again.
#[gpui::test]
fn only_an_agent_that_starts_under_this_view_counts_as_detected(cx: &mut TestAppContext) {
    use anterm_core::cli_agent::CLIAgent;

    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _root_daemon) = harness(cx);
    let detected = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let pane = |reattached: bool, cx: &mut TestAppContext| {
        let (pane, daemon) = window
            .update(cx, |_, window, cx| {
                if reattached {
                    super::quiet_reattached_test_pane(2, window, cx)
                } else {
                    super::quiet_test_pane(3, window, cx)
                }
            })
            .unwrap();
        let seen = detected.clone();
        cx.update(|cx| {
            cx.subscribe(&pane, move |_, ev: &AgentDetected, _| {
                seen.borrow_mut().push(ev.0)
            })
            .detach()
        });
        (pane, daemon)
    };
    let report = |agent: Option<CLIAgent>,
                  at_prompt: bool,
                  pane: &Entity<TerminalView>,
                  daemon: &mut Stream,
                  cx: &mut TestAppContext| {
        DaemonMsg::Agent(agent).encode(daemon).unwrap();
        for _ in 0..200 {
            if pane.read_with(cx, |p, _| p.terminal.foreground_agent()) == agent {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        pane.update(cx, |p, cx| p.poll_agent_detection(at_prompt, cx));
        cx.run_until_parked();
    };

    let (restored, mut restored_daemon) = pane(true, cx);
    report(
        Some(CLIAgent::Claude),
        false,
        &restored,
        &mut restored_daemon,
        cx,
    );
    report(
        Some(CLIAgent::Claude),
        false,
        &restored,
        &mut restored_daemon,
        cx,
    );
    assert_eq!(
        *detected.borrow(),
        vec![],
        "already running when reattached"
    );
    report(None, true, &restored, &mut restored_daemon, cx);
    report(
        Some(CLIAgent::Codex),
        false,
        &restored,
        &mut restored_daemon,
        cx,
    );
    assert_eq!(
        *detected.borrow(),
        vec![CLIAgent::Codex],
        "started after the reattach"
    );

    let (fresh, mut fresh_daemon) = pane(false, cx);
    report(Some(CLIAgent::Claude), false, &fresh, &mut fresh_daemon, cx);
    report(Some(CLIAgent::Claude), false, &fresh, &mut fresh_daemon, cx);
    assert_eq!(
        *detected.borrow(),
        vec![CLIAgent::Codex, CLIAgent::Claude],
        "a spawned pane counts its first agent, once"
    );
}

/// #958: `prompt_cursor_style` shapes the shell's own caret at a prompt,
/// leaves the running program's cursor alone, and steps aside for a vi-mode
/// prompt, whose insert/normal shapes are the shell's to set.
#[gpui::test]
fn prompt_cursor_style_shapes_only_the_prompt(cx: &mut TestAppContext) {
    use anterm_ui_kit::config::{CursorStyle, PromptCursorStyle};

    let (window, mut daemon) = harness(cx);
    let shape = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let view = window.entity(cx).unwrap();
            TerminalElement::new(view).prompt_cursor_shape(cx)
        })
    };
    let wait_for = |cx: &mut TestAppContext, want: &dyn Fn(&TerminalView) -> bool| {
        for _ in 0..200 {
            cx.run_until_parked();
            if window.update(cx, |view, _, _| want(view)).unwrap() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the pane never reached the expected prompt state");
    };
    cx.update(|cx| cx.global_mut::<Config>().prompt_cursor_style = PromptCursorStyle::Bar);

    assert_eq!(
        shape(cx),
        None,
        "no prompt reported yet: a program owns the cursor"
    );

    DaemonMsg::Prompt {
        active: true,
        at_prompt: true,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for(cx, &|view| view.terminal.at_prompt());
    assert_eq!(shape(cx), Some(CursorStyle::Bar));

    cx.update(|cx| cx.global_mut::<Config>().prompt_cursor_style = PromptCursorStyle::Follow);
    assert_eq!(shape(cx), None, "follow leaves the prompt on cursor_style");
    cx.update(|cx| cx.global_mut::<Config>().prompt_cursor_style = PromptCursorStyle::Bar);

    DaemonMsg::Prompt {
        active: true,
        at_prompt: false,
        last_exit: None,
    }
    .encode(&mut daemon)
    .unwrap();
    wait_for(cx, &|view| !view.terminal.at_prompt());
    assert_eq!(shape(cx), None, "a running command gets cursor_style back");
}

/// Seeds a pane's history for the ghost tests: `(command, dirs, exit)`,
/// oldest first, with `cwd` as the directory the pane is in.
fn seed_ghost_history(
    view: &mut TerminalView,
    entries: &[(&str, &[&str], Option<i32>)],
    cwd: Option<&str>,
    cx: &mut Context<TerminalView>,
) {
    let mut snap = crate::prompt::history::ScopeSnapshot::new();
    snap.entries = entries.iter().map(|(cmd, _, _)| cmd.to_string()).collect();
    for (cmd, dirs, exit) in entries {
        snap.cwds.insert(
            cmd.to_string(),
            dirs.iter().map(|d| d.to_string()).collect(),
        );
        snap.meta.insert(
            cmd.to_string(),
            crate::prompt::history::EntryMeta {
                ts: None,
                exit: *exit,
                duration_ms: None,
            },
        );
    }
    cx.default_global::<HistoryStore>()
        .replace_scope_for_test(&view.prompt.history_scope, snap);
    view.prompt.ranked_cwd = cwd.map(std::path::PathBuf::from);
    view.invalidate_ghost_cache();
}

#[gpui::test]
fn ghost_suggests_the_command_just_run_over_a_frequent_one(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_ghost_history(
                view,
                &[
                    ("git checkout main", &["/work/proj"], Some(0)),
                    ("git commit -m x", &["/work/proj"], Some(0)),
                ],
                Some("/work/proj"),
                cx,
            );
            cx.default_global::<HistoryStore>()
                .snapshot_mut(&view.prompt.history_scope)
                .counts
                .insert("git checkout main".to_string(), 20);
            view.invalidate_ghost_cache();
            view.prompt.cmd.set("git c");
            assert_eq!(
                view.ghost_suggestion(cx).as_deref(),
                Some("git commit -m x"),
                "recency decides the ghost, however often the older one ran"
            );
        })
        .unwrap();
}

#[gpui::test]
fn ghost_prefers_this_directory_then_falls_back_to_anywhere(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_ghost_history(
                view,
                &[
                    ("cargo build", &["/work/proj"], None),
                    ("cargo test --all", &["/elsewhere"], None),
                    ("npm run dev", &["/elsewhere"], None),
                ],
                Some("/work/proj"),
                cx,
            );
            view.prompt.cmd.set("cargo ");
            assert_eq!(
                view.ghost_suggestion(cx).as_deref(),
                Some("cargo build"),
                "an older match run here beats a newer one run elsewhere"
            );
            view.prompt.cmd.set("npm ");
            assert_eq!(
                view.ghost_suggestion(cx).as_deref(),
                Some("npm run dev"),
                "with nothing run here, the newest match anywhere is offered"
            );
            view.prompt.cmd.set("python ");
            assert_eq!(view.ghost_suggestion(cx), None);
        })
        .unwrap();
}

#[gpui::test]
fn ghost_skips_commands_that_failed(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_ghost_history(
                view,
                &[
                    ("make test", &["/work/proj"], Some(0)),
                    ("make tset", &["/work/proj"], Some(2)),
                    ("make lint", &["/work/proj"], Some(1)),
                ],
                Some("/work/proj"),
                cx,
            );
            view.prompt.cmd.set("make t");
            assert_eq!(
                view.ghost_suggestion(cx).as_deref(),
                Some("make test"),
                "the newer typo exited 2, so the older success is offered"
            );
            view.prompt.cmd.set("make l");
            assert_eq!(
                view.ghost_suggestion(cx),
                None,
                "a command whose only run failed is never offered"
            );
        })
        .unwrap();
}

#[gpui::test]
fn ghost_names_what_up_arrow_recalls(cx: &mut TestAppContext) {
    let (window, _daemon) = harness(cx);
    window
        .update(cx, |view, _, cx| {
            seed_ghost_history(
                view,
                &[
                    ("git status", &[], None),
                    ("git log --oneline", &[], None),
                    ("ls", &[], None),
                    ("git push", &[], None),
                    ("echo hi", &[], None),
                ],
                None,
                cx,
            );
            cx.default_global::<HistoryStore>()
                .snapshot_mut(&view.prompt.history_scope)
                .counts
                .insert("git status".to_string(), 50);
            view.invalidate_ghost_cache();
            for typed in ["git", "git l", "e", "l"] {
                view.prompt.history_nav = None;
                view.prompt.cmd.set(typed);
                let ghost = view.ghost_suggestion(cx);
                view.handle_editor_key(&key("up"), cx);
                assert_eq!(
                    ghost.as_deref(),
                    Some(view.prompt.cmd.text().as_str()),
                    "ghost and ↑ disagree on {typed:?}"
                );
            }
        })
        .unwrap();
}

#[gpui::test]
fn submitting_a_command_again_moves_it_to_the_newest_entry(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["git status", "ls", "make"], cx);
            view.prompt.cmd.set("git status");
            view.submit_command(cx);
            assert_eq!(
                cx.default_global::<HistoryStore>()
                    .snapshot(&view.prompt.history_scope)
                    .entries,
                ["ls", "make", "git status"],
                "one entry per command, at its latest run, as a reload leaves it"
            );

            view.prompt.cmd.set("");
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "git status");
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "make", "↑ never lands on a command twice");
        })
        .unwrap();
}

#[gpui::test]
fn per_pane_history_walks_current_session_first_then_falls_back_to_host_without_duplicates(
    cx: &mut TestAppContext,
) {
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);
    cx.update(|cx| {
        let mut cfg = cx.global::<Config>().clone();
        cfg.per_pane_history = true;
        cx.set_global(cfg);
    });

    window
        .update(cx, |view, _, cx| {
            seed_history(view, &["old-host", "shared-cmd", "other-pane-cmd"], cx);
            // This pane runs two commands (one new, one already in host history).
            view.prompt.cmd.set("pane-only");
            view.submit_command(cx);
            view.prompt.cmd.set("shared-cmd");
            view.submit_command(cx);

            view.prompt.cmd.set("draft");
            view.prompt.cmd.set_cursor(0); // empty prefix walks all

            // Walks current pane's session commands first (newest -> oldest).
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "shared-cmd");
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "pane-only");

            // Smoothly falls back to host history, skipping already-shown session commands.
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "other-pane-cmd");
            view.handle_editor_key(&key("up"), cx);
            assert_eq!(view.prompt.cmd.text(), "old-host");

            // Reversing with Down walks cleanly back across the boundary to the stash.
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.prompt.cmd.text(), "other-pane-cmd");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.prompt.cmd.text(), "pane-only");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.prompt.cmd.text(), "shared-cmd");
            view.handle_editor_key(&key("down"), cx);
            assert_eq!(view.prompt.cmd.text(), "draft");
        })
        .unwrap();
}

#[gpui::test]
fn ctrl_r_tab_cycles_filter_scopes_and_ctrl_x_deletes_selected_entry(cx: &mut TestAppContext) {
    use crate::prompt::history::HistoryFilter;

    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    window
        .update(cx, |view, _, cx| {
            let mut snap = crate::prompt::history::ScopeSnapshot::new();
            snap.apply_record(
                "other:pane",
                "other-dir-cmd".into(),
                Some("/other".into()),
                100,
                Some(0),
                None,
            );
            snap.apply_record(
                "other:pane",
                "cwd-only-cmd".into(),
                Some("/work/repo".into()),
                200,
                Some(0),
                None,
            );
            snap.apply_record(
                &view.prompt.history_session_id,
                "session-cmd".into(),
                Some("/work/repo".into()),
                300,
                Some(0),
                None,
            );
            cx.default_global::<HistoryStore>()
                .replace_scope_for_test(&view.prompt.history_scope, snap);
            view.prompt.ranked_cwd = Some(std::path::PathBuf::from("/work/repo"));

            // Default filter is Host (all 3 commands visible).
            view.handle_editor_key(&key("ctrl-r"), cx);
            assert_eq!(
                view.prompt.reverse_search.as_ref().map(|r| r.filter()),
                Some(HistoryFilter::Host)
            );
            assert_eq!(
                view.prompt.reverse_search.as_ref().map(|r| r.matches().len()),
                Some(3)
            );

            // Shift+Tab -> Directory (2 commands in /work/repo).
            view.tab_pressed(false, cx);
            assert_eq!(
                view.prompt.reverse_search.as_ref().map(|r| r.filter()),
                Some(HistoryFilter::Directory)
            );
            assert_eq!(
                view.prompt.reverse_search.as_ref().map(|r| r.matches().len()),
                Some(2)
            );

            // Shift+Tab -> Session (1 command in this pane).
            view.tab_pressed(false, cx);
            assert_eq!(
                view.prompt.reverse_search.as_ref().map(|r| r.filter()),
                Some(HistoryFilter::Session)
            );
            assert_eq!(
                selected_reverse_search_line(view, cx).as_deref(),
                Some("session-cmd")
            );

            // Ctrl+X deletes the selected entry immediately.
            view.handle_editor_key(&key("ctrl-x"), cx);
            assert!(
                view.prompt.reverse_search
                    .as_ref()
                    .is_some_and(|r| r.matches().is_empty()),
                "deleted command disappears from the active search"
            );
            assert!(
                !cx.default_global::<HistoryStore>()
                    .snapshot(&view.prompt.history_scope)
                    .entries
                    .iter()
                    .any(|e| e == "session-cmd"),
                "deleted command is removed from the shared store"
            );
        })
        .unwrap();
}

#[gpui::test]
fn leading_space_commands_are_not_recorded_in_history(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    let secret = format!(" export API_KEY=secret_{}", std::process::id());
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set(&secret);
            view.submit_command(cx);
            assert!(
                view.prompt.pending_history.is_none(),
                "ignorespace command must not enter pending_history"
            );
            assert!(
                !cx.default_global::<HistoryStore>()
                    .snapshot(&view.prompt.history_scope)
                    .entries
                    .iter()
                    .any(|e| e.contains("API_KEY=secret_")),
                "ignorespace command must never appear in HistoryStore"
            );
        })
        .unwrap();
    assert_eq!(
        next_input_until_timeout(&mut daemon),
        Some(format!("{secret}\r").into_bytes()),
        "the command itself still executes on the PTY"
    );
}

#[gpui::test]
fn multiline_command_persists_through_sqlite_history(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let dir = anterm_ui_kit::config::config_dir_path().expect("pinned config dir");
    let (window, mut daemon) = harness(cx);
    prompt_ready(&window, cx, &mut daemon);

    let cmd = format!("for x in a b; do\n  echo multi_{}_$x\ndone", std::process::id());
    window
        .update(cx, |view, _, cx| {
            view.prompt.cmd.set(&cmd);
            view.submit_command(cx);
            cx.default_global::<HistoryStore>()
                .ensure_worker()
                .flush_sync();
        })
        .unwrap();

    let db = crate::prompt::history::HistoryDb::open_at(dir.join("history.db")).unwrap();
    let snap = db
        .load_scope_snapshot(
            &crate::prompt::history::Scope::Local,
            crate::prompt::history::MAX_HOT_ENTRIES,
        )
        .unwrap();
    assert!(
        snap.entries.iter().any(|e| e == &cmd),
        "multiline command must persist verbatim in SQLite: {:?}",
        snap.entries
    );
}

#[gpui::test]
fn row_cache_rebuilds_only_dirty_rows_on_single_line_update(cx: &mut TestAppContext) {
    let (_window, view, mut daemon) = rooted_harness(cx);

    let wait_rev = |cx: &mut TestAppContext, after: u64| {
        for _ in 0..400 {
            cx.run_until_parked();
            let rev = cx.update(|cx| view.read(cx).terminal.grid_revision());
            if rev > after {
                return rev;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("grid_revision never advanced past {after}");
    };

    // Initial full paint: populate multiple rows so RowCache is warm.
    let rev0 = cx.update(|cx| view.read(cx).terminal.grid_revision());
    let mut initial = String::new();
    for i in 0..10 {
        initial.push_str(&format!("line {i}: hello world\r\n"));
    }
    initial.push_str("spinner 0");
    DaemonMsg::Output(initial.into_bytes())
        .encode(&mut daemon)
        .unwrap();
    let rev1 = wait_rev(cx, rev0);
    cx.run_until_parked();

    let (rows, initial_rebuilt) = cx.update(|cx| {
        view.update(cx, |v, _| {
            let rows = v.terminal.size().rows;
            let initial_rebuilt = v.render.row_cache.stats_rebuilt_rows;
            v.render.row_cache.stats_rebuilt_rows = 0;
            v.render.row_cache.stats_reused_rows = 0;
            (rows, initial_rebuilt)
        })
    });
    assert!(rows >= 12, "expected multi-row viewport, got {rows}");
    assert!(
        initial_rebuilt >= rows,
        "initial paint should populate RowCache, got initial_rebuilt={initial_rebuilt}, rows={rows}"
    );

    // In-place update on a single row (no scrolling): only that row should rebuild.
    DaemonMsg::Output(b"\rspinner 1".to_vec())
        .encode(&mut daemon)
        .unwrap();
    let _rev2 = wait_rev(cx, rev1);
    cx.run_until_parked();

    let (rebuilt, reused) = cx.update(|cx| {
        let v = view.read(cx);
        (v.render.row_cache.stats_rebuilt_rows, v.render.row_cache.stats_reused_rows)
    });
    assert!(
        rebuilt >= 1 && rebuilt <= 2,
        "expected 1-2 rebuilt rows for single-line update, got rebuilt={rebuilt}, reused={reused}, rows={rows}"
    );
    assert!(
        reused >= rows - 2,
        "expected at least {} clean rows reused from RowCache, got reused={reused}, rebuilt={rebuilt}",
        rows - 2
    );
}

#[gpui::test]
fn canvas_notify_does_not_rerender_search_bar_or_composer(cx: &mut TestAppContext) {
    let (window, view, mut daemon) = rooted_harness(cx);
    // Register the probe global before the initial draw so resetting the
    // counters later does not trigger a gpui-fast global-presence invalidation.
    cx.update(overlay_render_probe::reset);

    // Draw once so the terminal grid has concrete dimensions, then paint a
    // Claude prompt so the composer stands in over it.
    cx.run_until_parked();
    let (cols, lines) = cx.update(|cx| {
        let s = view.read(cx).terminal.size();
        (s.cols as usize, s.rows as usize)
    });
    let rule = "─".repeat(cols);
    let prompt_screen = format!(
        "\x1b[?2004h\x1b[2J\x1b[H⏺ Ready.\x1b[{};1H{rule}\x1b[{};1H❯ \x1b[{};1H{rule}\x1b[{};1H  ? for shortcuts",
        lines - 3,
        lines - 2,
        lines - 1,
        lines,
    );
    DaemonMsg::Agent(Some(anterm_core::cli_agent::CLIAgent::Claude))
        .encode(&mut daemon)
        .unwrap();
    DaemonMsg::Output(prompt_screen.into_bytes())
        .encode(&mut daemon)
        .unwrap();
    for _ in 0..200 {
        cx.run_until_parked();
        let ready = cx.update(|cx| {
            let v = view.read(cx);
            v.agent() == Some(anterm_core::cli_agent::CLIAgent::Claude)
                && v.terminal
                    .term
                    .lock()
                    .mode()
                    .contains(TermMode::BRACKETED_PASTE)
        });
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| {
                v.open_search(window, cx);
                v.toggle_composer(window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();

    assert!(
        cx.update(|cx| view.read(cx).search_state.active.is_some()),
        "search bar must be open"
    );
    assert!(
        cx.update(|cx| view.read(cx).composer_shown()),
        "composer must be visible"
    );

    // Settle initial layout & cache population (the composer measures its
    // natural height on frame 0 and caches on frame 1), then reset the probe.
    cx.update(|cx| cx.refresh_windows());
    cx.run_until_parked();
    cx.update(|cx| view.update(cx, |v, cx| v.notify_canvas(cx)));
    cx.run_until_parked();
    cx.update(overlay_render_probe::reset);

    // Simulate 8 high-frequency PTY output ticks notifying only TerminalCanvas.
    for _ in 0..8 {
        cx.update(|cx| {
            view.update(cx, |v, cx| v.notify_canvas(cx));
        });
        cx.run_until_parked();
    }

    let (search_renders, composer_renders) = cx.update(|cx| {
        (
            overlay_render_probe::search_bar_renders(cx),
            overlay_render_probe::composer_renders(cx),
        )
    });
    assert_eq!(
        search_renders, 0,
        "TerminalSearchBar must not re-render on pure TerminalCanvas output ticks"
    );
    assert_eq!(
        composer_renders, 0,
        "AgentComposerView must not re-render on pure TerminalCanvas output ticks"
    );

    // Explicit TerminalView notify (e.g. toggling search case sensitivity) must
    // still invalidate the overlay entities.
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.search_state.case_sensitive = !v.search_state.case_sensitive;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(
        cx.update(|cx| overlay_render_probe::search_bar_renders(cx) >= 1),
        "TerminalSearchBar must re-render when TerminalView notifies"
    );
}

#[gpui::test]
fn estimate_initial_grid_matches_first_frame_prepaint(cx: &mut TestAppContext) {
    anterm_ui_kit::config::pin_test_config_dir();
    let (window, _daemon) = harness(cx);
    let mut vcx = gpui::VisualTestContext::from_window(window.into(), cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();

    let (estimated, actual_size, actual_cell) = window
        .update(cx, |view, window, cx| {
            let viewport = window.viewport_size();
            let estimated = TerminalView::estimate_initial_grid(
                viewport,
                view.render.font_size.as_f32(),
                window,
                cx,
            );
            let scale = window.scale_factor();
            let scale = if scale.is_finite() && scale > 0. {
                scale
            } else {
                1.
            };
            let cell_w = (view.render.cell_width.as_f32() * scale).round().max(1.) as u16;
            let cell_h = (view.render.line_height.as_f32() * scale).round().max(1.) as u16;
            (estimated, view.terminal.size(), (cell_w, cell_h))
        })
        .unwrap();

    assert_eq!(
        estimated.size, actual_size,
        "estimated initial grid must match Frame 1 TerminalElement::prepaint cols/rows"
    );
    assert_eq!(
        (estimated.cell_w, estimated.cell_h),
        actual_cell,
        "estimated device-pixel cell metrics must match Frame 1 set_grid_size"
    );
}
