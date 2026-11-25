use gpui::{App, Axis, Context, Entity, Window, prelude::*, px};

use crate::core::config::Config;
use crate::core::session::{Session, SessionAxis, SessionPane, SessionTab};
use crate::daemon::protocol::ShellSpec;
use crate::terminal::view::{ChildExited, TerminalView};
use crate::ui::remote::host_registry::HostId;
use crate::ui::workspace::pane::{Pane, PaneSlot};

use super::{AntermApp, Asleep, Tab};

/// Orders tab indices most-recently-used first from their `last_used` stamps.
/// A zero stamp means the tab was never activated, and those keep strip order
/// at the back. `active` leads regardless — its own stamp only lands on the
/// next frame.
/// The tab after (or before) `active` in `order`, wrapping round. `None`
/// when there is nowhere else to go; a tab missing from `order` starts from
/// its end, so the step still lands on a tab the user can see.
pub(super) fn step_in_order(order: &[usize], active: usize, forward: bool) -> Option<usize> {
    let n = order.len();
    if n < 2 {
        return None;
    }
    let pos = order.iter().position(|&i| i == active);
    let next = match (pos, forward) {
        (Some(p), true) => (p + 1) % n,
        (Some(p), false) => (p + n - 1) % n,
        (None, true) => 0,
        (None, false) => n - 1,
    };
    Some(order[next]).filter(|&i| i != active)
}

/// The `apply_tab_order` permutation that moves the active tab one slot
/// within `run` — a move, not a trade: the tab is lifted out and put back at
/// the neighbour's place, so through a wrap the run it crossed shifts one slot
/// (`[a, b, c]` with `c` moved right becomes `[c, a, b]`, not `[c, b, a]`).
/// `run` is the tabs drawn together (a sidebar section, or every tab on a top
/// bar) as ascending `self.tabs` indices out of `n`; only their slots are
/// reassigned, so no tab in another run moves, not even out of sight. `None`
/// when there is nothing to move: a run of fewer than two tabs, or one that
/// does not hold the active tab.
pub(super) fn one_slot_move(run: &[usize], n: usize, active: usize, right: bool) -> Option<Vec<usize>> {
    let len = run.len();
    if len < 2 {
        return None;
    }
    let pos = run.iter().position(|&i| i == active)?;
    let mut moved = run.to_vec();
    match (right, pos) {
        (true, p) if p + 1 == len => moved.rotate_right(1),
        (false, 0) => moved.rotate_left(1),
        (true, p) => moved.swap(p, p + 1),
        (false, p) => moved.swap(p, p - 1),
    }
    let mut permutation: Vec<usize> = (0..n).collect();
    for (&slot, &tab) in run.iter().zip(&moved) {
        permutation[slot] = tab;
    }
    Some(permutation)
}

pub(super) fn mru_order(stamps: &[u64], active: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..stamps.len()).collect();
    order.sort_by_key(|&i| (stamps[i] == 0, std::cmp::Reverse(stamps[i]), i));
    if let Some(pos) = order.iter().position(|&i| i == active) {
        let lead = order.remove(pos);
        order.insert(0, lead);
    }
    order
}

pub(super) fn tab_to_session(tab: &Tab, cx: &App) -> SessionTab {
    SessionTab {
        name: tab.name.clone(),
        pane: match tab.asleep_layout() {
            Some(layout) => layout.clone(),
            None => pane_to_session(&tab.pane, cx),
        },
        tags: tab.tags.borrow().clone(),
        tree_id: None,
        hibernated: false,
        asleep_view: None,
    }
}

pub(super) fn agent_resume_command(
    agent: &Option<crate::core::cli_agent::CLIAgent>,
    session_id: Option<&str>,
    launch_argv: Option<&[String]>,
    view: &TerminalView,
    cx: &App,
) -> Option<String> {
    if !cx.global::<Config>().restore_agent_sessions {
        return None;
    }
    let agent = agent.as_ref()?;
    let Some(session_id) = session_id else {
        log::info!(
            "{}'s pane had no captured session id; it comes back as a plain shell",
            agent.display_name()
        );
        return None;
    };
    agent.restore_command(
        session_id,
        launch_argv,
        pane_shell_program(view, cx).as_deref(),
    )
}

/// The shell `view` is running, for deciding what a line typed into it may
/// use. A pane spawned without an explicit shell got the configured one or,
/// failing that, the login shell — but only a local pane got this machine's;
/// a workspace pane's default lives on its host, so it stays unknown.
fn pane_shell_program(view: &TerminalView, cx: &App) -> Option<String> {
    if let Some(spec) = view.shell_spec() {
        return Some(spec.program);
    }
    if view.ssh_spec().is_some() || view.remote_context().is_some() {
        return None;
    }
    Some(match &cx.global::<Config>().shell {
        Some(shell) if !shell.program.trim().is_empty() => shell.program.clone(),
        _ => crate::core::shells::login_shell(),
    })
}

pub(super) fn pane_to_session(pane: &Pane, cx: &App) -> SessionPane {
    match pane {
        Pane::Leaf(PaneSlot::Connecting(pending)) => {
            let spawn = &pending.read(cx).spawn;
            SessionPane::Leaf {
                cwd: spawn.working_directory.clone(),
                pane_id: spawn.restore_pane,
                shell: spawn.shell.clone(),
                ssh_spec: None,
                agent: spawn.agent,
                agent_session_id: spawn.agent_session_id.clone(),
                agent_launch_argv: spawn.agent_launch_argv.clone(),
            }
        }
        Pane::Leaf(PaneSlot::Ready(view)) => {
            let view = view.read(cx);
            SessionPane::Leaf {
                // A native SSH leaf keeps the far shell's directory, so a
                // sleeping SSH tab wakes where it was: the redial that wakes it
                // takes this as its start directory on the same host, and the
                // tree record it seeds holds the far directory for such a pane
                // anyway. Any other remote pane's cwd is on a host no restore
                // dials, so it is still left out.
                cwd: match view.ssh_spec() {
                    Some(_) => view.cwd(),
                    None => view.spawnable_cwd(),
                },
                pane_id: Some(view.pane_id),
                // `None` for a pane this window attached to rather than
                // spawned: it never knew what was on the other end. The tree
                // does — the daemon records it — and that is what a restore
                // reads, so the gap here costs nothing it can see.
                shell: view.shell_spec(),
                ssh_spec: view.ssh_spec(),
                agent: view.agent(),
                agent_session_id: view.agent_session().and_then(|s| s.session_id),
                agent_launch_argv: view.agent_session().and_then(|s| s.launch_argv),
            }
        }
        Pane::Split {
            axis, a, b, ratio, ..
        } => SessionPane::Split {
            axis: match axis {
                Axis::Horizontal => SessionAxis::Horizontal,
                Axis::Vertical => SessionAxis::Vertical,
            },
            ratio: ratio.get(),
            a: Box::new(pane_to_session(a, cx)),
            b: Box::new(pane_to_session(b, cx)),
        },
        Pane::Empty => SessionPane::Leaf {
            cwd: None,
            pane_id: None,
            shell: None,
            ssh_spec: None,
            agent: None,
            agent_session_id: None,
            agent_launch_argv: None,
        },
    }
}

/// The daemon's account of which panes are alive, or `None` when it could not
/// be asked at all.
///
/// The distinction is the point: a pane absent from a *successful* listing is
/// genuinely gone and may be respawned, while a failed `List` says nothing
/// about any pane. Flattening the failure into an empty map made one transient
/// RPC error read as "every pane is dead", and the restore then spawned fresh
/// shells over all of them — the same destruction-by-inference this file's
/// restore path is built to avoid.
pub(crate) fn alive_panes() -> Option<std::collections::HashSet<u64>> {
    match crate::terminal::RemoteTerminal::try_list_panes_on(&crate::terminal::PaneRoute::Local) {
        Ok(list) => Some(
            list.into_iter()
                .filter(|p| p.alive)
                .map(|p| p.pane_id)
                .collect(),
        ),
        Err(e) => {
            log::warn!("could not list panes ({e}); leaving each attach to decide");
            None
        }
    }
}

pub(super) fn tabs_from_session(
    session: Option<Session>,
    font_size: f32,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> (Vec<Tab>, usize, usize) {
    let Some(session) = session.filter(|s| !s.tabs.is_empty()) else {
        return (Vec::new(), 0, 0);
    };
    let alive = alive_panes();
    let mut tabs: Vec<Tab> = Vec::with_capacity(session.tabs.len());
    let mut dropped = 0usize;
    let home = crate::ui::files::path_display::home_for_host(cx, HostId::LOCAL);
    for (index, st) in session.tabs.iter().enumerate() {
        if st.hibernated && index != session.active {
            tabs.push(asleep_tab(st, home.clone()));
            continue;
        }
        let Some(pane) = session_to_pane(&st.pane, alive.as_ref(), font_size, window, cx) else {
            log::error!("dropping a restored tab: no pane in it could be started");
            dropped += 1;
            continue;
        };
        tabs.push(Tab {
            pane,
            name: st.name.clone(),
            last_focused: None,
            zoomed: None,
            tags: std::cell::RefCell::new(st.tags.clone()),
            tree_id: std::cell::Cell::new(
                st.tree_id
                    .unwrap_or_else(anterm_core::machine::TabId::new),
            ),
            last_used: std::cell::Cell::new(0),
            focus_origin: Default::default(),
            asleep: None,
        });
    }
    let active = session.active.min(tabs.len().saturating_sub(1));
    (tabs, active, dropped)
}

/// A tab restored asleep: its place, its name and its tags, and nothing
/// running behind them.
fn asleep_tab(st: &SessionTab, home: Option<std::path::PathBuf>) -> Tab {
    let mut tab = Tab::new(Pane::Empty);
    tab.name = st.name.clone();
    *tab.tags.borrow_mut() = st.tags.clone();
    if let Some(id) = st.tree_id {
        tab.tree_id.set(id);
    }
    tab.asleep = Some(Asleep {
        layout: st.pane.clone(),
        view: st
            .asleep_view
            .clone()
            .unwrap_or_else(|| view_of_layout(tab.tree_id.get(), &st.pane)),
        home,
    });
    tab
}

/// What to call a sleeping tab when the tree had nothing better to say: the
/// first directory in its layout, and the agent in it if there was one.
fn view_of_layout(
    id: anterm_core::machine::TabId,
    layout: &SessionPane,
) -> anterm_core::tab_view::TabView {
    fn leaves<'a>(pane: &'a SessionPane, out: &mut Vec<&'a SessionPane>) {
        match pane {
            SessionPane::Leaf { .. } => out.push(pane),
            SessionPane::Split { a, b, .. } => {
                leaves(a, out);
                leaves(b, out);
            }
        }
    }
    let mut all = Vec::new();
    leaves(layout, &mut all);
    let cwd = all.iter().find_map(|leaf| match leaf {
        SessionPane::Leaf { cwd, .. } => cwd.as_ref().map(|p| p.display().to_string()),
        SessionPane::Split { .. } => None,
    });
    let agent = all.iter().find_map(|leaf| match leaf {
        SessionPane::Leaf { agent, .. } => *agent,
        SessionPane::Split { .. } => None,
    });
    anterm_core::tab_view::TabView {
        id,
        name: None,
        title: String::new(),
        osc_title: None,
        cwd,
        agent,
        status: None,
        live: false,
        panes: all.len(),
    }
}

/// A sleeping layout as a wake should read it: a native SSH pane dials its
/// host again rather than standing on its old id. That id belongs to a pane
/// whose connection is gone, and the restore reading it would put a local
/// shell in its place.
pub(super) fn redial_native_ssh(layout: &SessionPane) -> SessionPane {
    match layout {
        SessionPane::Leaf {
            ssh_spec: Some(_), ..
        } => {
            let mut leaf = layout.clone();
            if let SessionPane::Leaf { pane_id, .. } = &mut leaf {
                *pane_id = None;
            }
            leaf
        }
        SessionPane::Leaf { .. } => layout.clone(),
        SessionPane::Split { axis, ratio, a, b } => SessionPane::Split {
            axis: *axis,
            ratio: *ratio,
            a: Box::new(redial_native_ssh(a)),
            b: Box::new(redial_native_ssh(b)),
        },
    }
}

/// Whether a native SSH pane can be attached to by its old id: only when the
/// server lists it. A failed attach falls through to a fresh *local* shell,
/// and after a server restart that put the user's own machine behind a tab
/// that had been — and still looked like — a session on another one. Without
/// a listing the host is dialled again: a second connection is recoverable,
/// typing into the wrong machine is not.
pub(super) fn native_ssh_pane_alive(
    alive: Option<&std::collections::HashSet<u64>>,
    id: u64,
) -> bool {
    alive.is_some_and(|alive| alive.contains(&id))
}

pub(super) fn session_to_pane(
    sp: &SessionPane,
    alive: Option<&std::collections::HashSet<u64>>,
    font_size: f32,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> Option<Pane> {
    match sp {
        SessionPane::Leaf {
            cwd,
            pane_id,
            shell,
            ssh_spec,
            agent,
            agent_session_id,
            agent_launch_argv,
        } => {
            let restore =
                (*pane_id).filter(|id| ssh_spec.is_none() || native_ssh_pane_alive(alive, *id));
            if restore.is_none() {
                if let Some(spec) = ssh_spec.clone() {
                    let resolved =
                        crate::ui::remote::ssh_connect::resolve_persisted_ssh_spec(spec, cx);
                    match new_terminal_native(font_size, cwd.clone(), resolved, window, cx) {
                        Ok(view) => return Some(Pane::leaf(PaneSlot::Ready(view))),
                        Err(e) => log::error!("restoring native SSH pane failed: {e}"),
                    }
                }
            }
            let local_cwd = match ssh_spec {
                Some(_) => None,
                None => cwd.clone(),
            };
            let view = match new_terminal(
                font_size,
                local_cwd,
                restore,
                shell.clone(),
                window,
                cx,
            ) {
                Ok(view) => view,
                Err(e) => {
                    log::error!("restoring pane failed: {e}");
                    return None;
                }
            };
            match &view {
                PaneSlot::Ready(terminal) if !terminal.read(cx).restored() => {
                    if let Some(cmd) = agent_resume_command(
                        agent,
                        agent_session_id.as_deref(),
                        agent_launch_argv.as_deref(),
                        terminal.read(cx),
                        cx,
                    ) {
                        terminal.read(cx).run_command_line(&cmd);
                    }
                }
                PaneSlot::Ready(_) => {}
                PaneSlot::Connecting(pending) => {
                    pending.update(cx, |pending, _| {
                        pending.spawn.agent = *agent;
                        pending.spawn.agent_session_id = agent_session_id.clone();
                        pending.spawn.agent_launch_argv = agent_launch_argv.clone();
                    });
                }
            }
            Some(Pane::leaf(view))
        }
        SessionPane::Split { axis, ratio, a, b } => {
            let axis = match axis {
                SessionAxis::Horizontal => Axis::Horizontal,
                SessionAxis::Vertical => Axis::Vertical,
            };
            match (
                session_to_pane(a, alive, font_size, window, cx),
                session_to_pane(b, alive, font_size, window, cx),
            ) {
                (Some(a), Some(b)) => Some(Pane::split_node(axis, *ratio, a, b)),
                (Some(only), None) | (None, Some(only)) => Some(only),
                (None, None) => None,
            }
        }
    }
}

/// Estimate the initial terminal grid for a pane about to be spawned.
///
/// When `pane_bounds` is `None` (e.g. during window startup before `pane_area`
/// has been painted), derives the single-pane body area from the window's
/// viewport minus frame paddings, title bar, and any open left sidebar or right
/// panel so the PTY starts at the exact size Frame 1 will lay out.
pub(super) fn initial_terminal_grid(
    font_size: f32,
    pane_bounds: Option<gpui::Size<gpui::Pixels>>,
    window: &Window,
    cx: &App,
) -> crate::terminal::view::InitialGrid {
    let bounds = pane_bounds.unwrap_or_else(|| {
        let viewport = window.viewport_size();
        let pad = gpui_kit::component::window_paddings(window);
        let viewport_w = viewport.width.as_f32();
        let content_w = (viewport_w - pad.left.as_f32() - pad.right.as_f32()).max(0.);
        let content_h = (viewport.height.as_f32() - pad.top.as_f32() - pad.bottom.as_f32()).max(0.);

        let cfg = cx.global::<Config>();
        let sidebar_open = cfg.tab_bar_position == crate::core::config::TabBarPosition::Left
            && !cfg.sidebar_collapsed;
        let right_panel_open = cfg.right_panel_visible;

        let right_min = if right_panel_open {
            crate::ui::files::right_panel::RightPanelView::min_width(window, cx)
        } else {
            0.
        };
        let sidebar_min = if sidebar_open {
            crate::ui::workspace::tab_sidebar::MIN_SIDEBAR_WIDTH
        } else {
            0.
        };

        let sidebar_w = if sidebar_open {
            let max = super::side_panel_max(viewport_w, sidebar_min, right_min);
            cfg.sidebar_width.clamp(sidebar_min, max)
        } else {
            0.
        };
        let right_panel_w = if right_panel_open {
            let max = super::side_panel_max(viewport_w, right_min, sidebar_min);
            cfg.right_panel_width.clamp(right_min, max)
        } else {
            0.
        };

        gpui::size(
            px((content_w - sidebar_w - right_panel_w).max(0.)),
            px((content_h - super::TITLE_BAR_HEIGHT).max(0.)),
        )
    });
    TerminalView::estimate_initial_grid(bounds, font_size, window, cx)
}

pub(crate) fn new_terminal(
    font_size: f32,
    working_directory: Option<std::path::PathBuf>,
    restore_pane: Option<u64>,
    shell: Option<ShellSpec>,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> anyhow::Result<PaneSlot> {
    new_terminal_in_bounds(
        font_size,
        working_directory,
        restore_pane,
        shell,
        None,
        window,
        cx,
    )
}

pub(super) fn new_terminal_in_bounds(
    font_size: f32,
    working_directory: Option<std::path::PathBuf>,
    restore_pane: Option<u64>,
    shell: Option<ShellSpec>,
    pane_bounds: Option<gpui::Size<gpui::Pixels>>,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> anyhow::Result<PaneSlot> {
    let initial = initial_terminal_grid(font_size, pane_bounds, window, cx);
    let parts =
        TerminalView::spawn_shell_terminal_in(working_directory, restore_pane, shell, initial)?;
    Ok(PaneSlot::Ready(build_terminal_view(
        parts, font_size, window, cx,
    )))
}

#[allow(dead_code)]
pub(super) fn start_pane_spawn(
    pending: Entity<crate::ui::workspace::pending_pane::PendingPane>,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) {
    let spawn = pending.read(cx).spawn.clone();
    let slot_id = pending.entity_id();
    let font_size = spawn.font_size;
    let initial = initial_terminal_grid(font_size, None, window, cx);
    cx.spawn_in(window, async move |this, cx| {
        let parts = cx
            .background_executor()
            .spawn(async move {
                TerminalView::spawn_shell_terminal_in(
                    spawn.working_directory.clone(),
                    spawn.restore_pane,
                    spawn.shell.clone(),
                    initial,
                )
                .map_err(|e| format!("{e:#}"))
            })
            .await;
        let _ = this.update_in(cx, |app, window, cx| {
            app.land_pane(slot_id, &pending, parts, font_size, window, cx);
        });
    })
    .detach();
}

pub(super) fn build_terminal_view(
    parts: crate::terminal::view::ShellParts,
    font_size: f32,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> Entity<TerminalView> {
    let view = cx.new(|cx| {
        let mut view = TerminalView::from_shell_parts(parts, window, cx);
        view.render.font_size = px(font_size);
        view
    });
    cx.subscribe_in(&view, window, |app, view, _: &ChildExited, window, cx| {
        app.on_child_exited(view.clone(), window, cx);
    })
    .detach();
    cx.subscribe_in(
        &view,
        window,
        |app, _view, _: &crate::terminal::view::AgentSessionChanged, _window, cx| {
            app.save_session(cx);
        },
    )
    .detach();
    cx.subscribe_in(
        &view,
        window,
        |app, _view, ev: &crate::terminal::view::AgentDetected, _window, cx| {
            app.note_agent_detected(ev.0, cx);
        },
    )
    .detach();
    cx.subscribe_in(
        &view,
        window,
        |app, view, _: &crate::terminal::view::AuthPromptReady, window, cx| {
            let want = !app.ssh_prompt.read(cx).has_active_prompt();
            let batch = super::lifecycle::drain_ssh_prompt_batch(view, want, cx);
            app.ssh_prompt
                .update(cx, |modal, cx| modal.push_prompt_batch(batch, window, cx));
        },
    )
    .detach();
    watch_open_file_requests(&view, window, cx);
    let handle = view.read(cx).focus_handle.clone();
    watch_pane_focus(&handle, view.entity_id(), window, cx);
    view
}

pub(super) fn kill_pane_off_thread(route: crate::terminal::PaneRoute, pane_id: u64, cx: &mut App) {
    cx.background_executor()
        .spawn(async move { crate::terminal::RemoteTerminal::kill_pane_on(&route, pane_id) })
        .detach();
}

/// Routes the file links clicked in a pane to whatever the app opens files
/// with. Every pane needs this, however it was spawned — a link in an SSH pane
/// is the same click as a link in a local one.
fn watch_open_file_requests(
    view: &Entity<TerminalView>,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) {
    cx.subscribe_in(
        view,
        window,
        |app, _view, ev: &crate::terminal::view::OpenFileRequested, window, cx| {
            let intent = if ev.is_dir {
                crate::ui::workspace::intent::WorkspaceIntent::RevealDirectory(ev.path.clone())
            } else {
                crate::ui::workspace::intent::WorkspaceIntent::OpenFile {
                    path: ev.path.clone(),
                    line: ev.line,
                    column: ev.column,
                }
            };
            app.handle_intent(intent, window, cx);
        },
    )
    .detach();
}

/// Record `leaf` as the pane the tab holding it comes back to.
///
/// Which tab that is gets asked of the layout rather than assumed to be the
/// active one: a pane dragged into another tab takes focus with it, and it is
/// the tab holding it now whose memory the arrival should change. A leaf no
/// tab holds — one that has just closed, or arrived after its slot went away —
/// is recorded nowhere.
pub(super) fn remember_leaf_in(tabs: &mut [Tab], leaf: gpui::EntityId) {
    let held = tabs
        .iter_mut()
        .find(|tab| tab.pane.leaves().iter().any(|l| l.entity_id() == leaf));
    if let Some(tab) = held {
        tab.last_focused = Some(leaf);
    }
}

/// Put `new` where the slot `old` named stood, carrying that tab's focus
/// memory across with it.
///
/// The memory has to move because the id it holds does not survive the swap.
/// Focus arriving in a pane that is still coming up is recorded against the
/// *pending* slot — that is why connecting slots are watched at all — and that
/// slot's id dies the moment the pane lands. Left behind, the memory names an
/// entity no tab holds, `focus_target` falls through `leaf_matching_or_first`,
/// and the tab comes back to its first leaf: #843 again, one landing later.
///
/// Nothing else writes the answer down in that case. `land_pane` re-focuses
/// the pane it built only when the pending slot still held focus, and with
/// focus off the panes the switch-away sample has nothing to read either.
pub(super) fn replace_leaf_in(tabs: &mut [Tab], old: gpui::EntityId, new: PaneSlot) {
    for tab in tabs.iter_mut() {
        if tab.pane.replace_leaf(old, new.clone()) {
            if tab.last_focused == Some(old) {
                tab.last_focused = Some(new.entity_id());
            }
            break;
        }
    }
}

/// Repaint the chrome that marks the focused pane, and record the leaf as the
/// one its tab returns to (#843).
///
/// Every leaf is watched, connecting slots included: a pane can be focused
/// while it is still coming up, and if the tab is left in that moment the
/// answer has to already be written down.
pub(super) fn watch_pane_focus(
    handle: &gpui::FocusHandle,
    leaf: gpui::EntityId,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) {
    let app = cx.weak_entity();
    window
        .on_focus_in(handle, cx, move |_window, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| {
                    app.remember_focused_leaf(leaf);
                    cx.notify();
                });
            }
        })
        .detach();
}

/// A pane dialling `spec`. `remote_start_dir` is a directory on the far host
/// for its shell to start in — never a local path; see
/// [`super::inherited_start_dir`] for which openings carry one.
pub(crate) fn new_terminal_native(
    font_size: f32,
    remote_start_dir: Option<std::path::PathBuf>,
    spec: Box<crate::daemon::protocol::NativeSshSpec>,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> anyhow::Result<Entity<TerminalView>> {
    new_terminal_native_in_bounds(font_size, remote_start_dir, spec, None, window, cx)
}

pub(super) fn new_terminal_native_in_bounds(
    font_size: f32,
    remote_start_dir: Option<std::path::PathBuf>,
    spec: Box<crate::daemon::protocol::NativeSshSpec>,
    pane_bounds: Option<gpui::Size<gpui::Pixels>>,
    window: &mut Window,
    cx: &mut Context<AntermApp>,
) -> anyhow::Result<Entity<TerminalView>> {
    let initial = initial_terminal_grid(font_size, pane_bounds, window, cx);
    let parts = TerminalView::spawn_native_ssh_terminal(spec, remote_start_dir, initial)?;
    let view = cx.new(|cx| {
        let mut view = TerminalView::from_native_ssh_parts(parts, window, cx);
        view.render.font_size = px(font_size);
        view
    });
    cx.subscribe_in(&view, window, |app, view, _: &ChildExited, window, cx| {
        app.on_child_exited(view.clone(), window, cx);
    })
    .detach();
    cx.subscribe_in(
        &view,
        window,
        |app, view, _: &crate::terminal::view::AuthPromptReady, window, cx| {
            let want = !app.ssh_prompt.read(cx).has_active_prompt();
            let batch = super::lifecycle::drain_ssh_prompt_batch(view, want, cx);
            app.ssh_prompt
                .update(cx, |modal, cx| modal.push_prompt_batch(batch, window, cx));
        },
    )
    .detach();
    watch_open_file_requests(&view, window, cx);
    let handle = view.read(cx).focus_handle.clone();
    watch_pane_focus(&handle, view.entity_id(), window, cx);
    Ok(view)
}

/// Reads the Settings "Shell Arguments" field as a command line (#551).
///
/// The field is one line of text standing in for the `shell.args` array, so
/// this and [`join_shell_args`] have to be exact inverses of each other — the
/// field is refilled from the array every time Settings opens, and committed
/// back on every blur. Quoting rules are the smallest set that can spell any
/// argv: single quotes take everything literally, double quotes take everything
/// literally except `\"` and `\\`, and whitespace outside quotes separates
/// words.
///
/// A lone backslash outside quotes is *not* an escape, unlike a POSIX shell:
/// these arguments are handed to the process as argv without a shell in
/// between, and `--dir C:\Users\me` has to keep its backslashes on the platform
/// where that is how a path is spelled. `#` starts no comment here either, for
/// the same reason: nothing in an argv list is a comment, and swallowing the
/// rest of the line would be the silent rewrite this whole change exists to
/// stop.
///

pub(crate) use anterm_ui_kit::intent::join_shell_args;
pub(crate) use anterm_ui_kit::ssh_helpers::parse_ssh_connect_input;
#[cfg(test)]
pub(crate) use anterm_ui_kit::ssh_helpers::parse_ssh_option_words;
