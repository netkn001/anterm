use std::collections::VecDeque;
use std::sync::Arc;

use gpui::{App, Global};
use anterm_core::daemon::control::{ControlClient, ControlRequest};
use anterm_core::machine::{
    AgentFacts, Axis as TreeAxis, Machine, PaneNode, PaneRecord, PaneSeed, Tab as TreeTab, TabId,
};

use crate::core::tag::{SidebarTags, TagId};
use crate::core::session::{Session, SessionPane, SessionTab};
use crate::ui::workspace::app::AntermApp;
use crate::ui::workspace::pane::{Pane, PaneSlot};

mod reconcile;
#[cfg(test)]
mod tests;

pub(crate) use reconcile::{
    forget, hydrate_window_from_tree, hydrate_window_then_open, mark_window_informed,
    note_instance, on_layout_delta, on_link_up, park_command_while_pulling,
    park_ssh_while_pulling, push_tags, resync_after_local_daemon_change,
    resync_local_windows_from_tree, sync_window,
};
#[cfg(test)]
pub(crate) use reconcile::{diff, prime_for_test, queued_for_test, resync_window_from_tree};

pub(crate) fn control(cx: &mut App) -> Option<Arc<ControlClient>> {
    crate::ui::remote::local_link::LocalLink::client(cx)
}

pub(crate) enum TreeLink {
    Ready(Arc<ControlClient>),
    Unserved,
    Down,
}

pub(crate) fn tree_control(cx: &mut App) -> TreeLink {
    classify_tree_link(control(cx))
}

fn classify_tree_link(client: Option<Arc<ControlClient>>) -> TreeLink {
    match client {
        Some(client)
            if client
                .hello()
                .has_feature(anterm_core::daemon::control::feature::MACHINE_TREE) =>
        {
            TreeLink::Ready(client)
        }
        Some(_) => TreeLink::Unserved,
        None => TreeLink::Down,
    }
}

/// Whether the local daemon can put a tab to sleep.
pub(crate) fn can_hibernate(cx: &App) -> bool {
    crate::ui::remote::local_link::LocalLink::supports(
        cx,
        anterm_core::daemon::control::feature::TAB_HIBERNATE,
    )
}

/// Whether the local daemon keeps recently closed tabs (#1021).
pub(crate) fn keeps_closed_tabs(cx: &App) -> bool {
    crate::ui::remote::local_link::LocalLink::supports(
        cx,
        anterm_core::daemon::control::feature::CLOSED_TABS,
    )
}

/// Whether closing `tab` can go to the machine's recently-closed list.
pub(crate) fn remembers_closed_tab(cx: &App, tab: TabId) -> bool {
    if !keeps_closed_tabs(cx) {
        return false;
    }
    let Some(state) = cx.try_global::<TreeSync>() else {
        return false;
    };
    state.informed
        && state.rehydrate.is_none()
        && matches!(&state.sync, SyncPhase::Primed(mirror) if mirror.tabs.iter().any(|t| t.id == tab))
}

/// A tab this window closed into the machine's recently-closed list.
#[derive(Clone, Debug)]
pub(crate) struct Retiring {
    pub tab: TabId,
    pub panes: Vec<u64>,
    pub route: crate::terminal::PaneRoute,
}

/// Records that the user closed `tab` in this window, which is what lets the
/// next sync close it on the machine.
pub(crate) fn intend_close(cx: &mut App, tab: TabId) {
    let closing = &mut cx.default_global::<TreeSync>().closing;
    if !closing.contains(&tab) {
        closing.push(tab);
    }
}

fn unaccounted(mirror: &WsMirror, desired: &[DesiredTab], closing: &mut Vec<TabId>) -> Vec<TabId> {
    closing.retain(|id| mirror.tabs.iter().any(|t| t.id == *id));
    mirror
        .tabs
        .iter()
        .map(|t| t.id)
        .filter(|id| !desired.iter().any(|d| d.id == *id) && !closing.contains(id))
        .collect()
}

pub(crate) fn retire_tab(cx: &mut App, retiring: Retiring) {
    cx.default_global::<TreeSync>().retiring.push(retiring);
}

pub(crate) fn take_unsent_retirement(cx: &mut App, tab: TabId) -> Option<Retiring> {
    let state = cx.default_global::<TreeSync>();
    let at = state.retiring.iter().position(|r| r.tab == tab)?;
    Some(state.retiring.remove(at))
}

pub(crate) fn tree_ops_settled(cx: &App) -> bool {
    cx.try_global::<TreeSync>()
        .is_none_or(|state| !state.inflight && state.queue.is_empty() && state.retiring.is_empty())
}

pub(crate) fn closed_tabs_link(cx: &mut App) -> Option<Arc<ControlClient>> {
    if !keeps_closed_tabs(cx) {
        return None;
    }
    match tree_control(cx) {
        TreeLink::Ready(client) => Some(client),
        TreeLink::Unserved | TreeLink::Down => None,
    }
}

pub(crate) fn session_tab_from_reopened(
    reopened: &anterm_core::machine::ReopenedTab,
) -> SessionTab {
    closed_session_tab(&reopened.tab, &reopened.panes)
}

fn closed_session_tab(tab: &TreeTab, panes: &[PaneRecord]) -> SessionTab {
    SessionTab {
        name: tab.name.clone(),
        pane: session_pane_from_node(&tab.root, panes),
        tags: tab.tags.clone(),
        tree_id: Some(tab.id),
        hibernated: false,
        asleep_view: None,
    }
}

pub(crate) fn newest_remembered_close(cx: &App) -> Option<SessionTab> {
    if !keeps_closed_tabs(cx) {
        return None;
    }
    let machine = crate::ui::remote::machine_mirror::MachineMirrors::machine(cx)?;
    let now = crate::ui::workspace::home::now_secs();
    let ttl = anterm_core::machine::CLOSED_TAB_TTL.as_secs();
    let entry = machine
        .closed
        .iter()
        .rev()
        .find(|c| now.saturating_sub(c.closed_at) < ttl)?;
    Some(closed_session_tab(&entry.tab, &machine.panes))
}

fn remember_closes(ops: &mut [ControlRequest], retiring: &mut Vec<Retiring>) -> Vec<Retiring> {
    let mut sent = Vec::new();
    for op in ops.iter_mut() {
        let ControlRequest::TabClose { tab } = *op else {
            continue;
        };
        let Some(at) = retiring.iter().position(|r| r.tab == tab) else {
            continue;
        };
        let retired = retiring.remove(at);
        *op = ControlRequest::TabCloseRemembered {
            tab,
            panes: retired.panes.clone(),
        };
        sent.push(retired);
    }
    sent
}

fn end_unsent_retirements(cx: &mut App, retired: &mut Vec<Retiring>, dropped: &[ControlRequest]) {
    for op in dropped {
        let ControlRequest::TabCloseRemembered { tab, .. } = op else {
            continue;
        };
        let Some(at) = retired.iter().position(|r| r.tab == *tab) else {
            continue;
        };
        let Retiring { panes, route, .. } = retired.remove(at);
        log::info!("the machine never took closed tab {tab}; ending its panes from here");
        cx.background_executor()
            .spawn(async move {
                for pane in panes {
                    crate::terminal::RemoteTerminal::kill_pane_on(&route, pane);
                }
            })
            .detach();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DesiredTab {
    pub id: TabId,
    pub name: Option<String>,
    pub tags: Vec<TagId>,
    pub root: DesiredNode,
    pub hibernated: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum DesiredNode {
    Leaf {
        pane: u64,
        seed: PaneSeed,
    },
    Split {
        axis: TreeAxis,
        ratio: f32,
        a: Box<DesiredNode>,
        b: Box<DesiredNode>,
    },
}

impl DesiredNode {
    fn first_leaf(&self) -> (&u64, &PaneSeed) {
        match self {
            DesiredNode::Leaf { pane, seed } => (pane, seed),
            DesiredNode::Split { a, .. } => a.first_leaf(),
        }
    }

    fn to_pane_node(&self) -> PaneNode {
        match self {
            DesiredNode::Leaf { pane, .. } => PaneNode::Leaf { pane: *pane },
            DesiredNode::Split { axis, ratio, a, b } => PaneNode::Split {
                axis: *axis,
                ratio: *ratio,
                a: Box::new(a.to_pane_node()),
                b: Box::new(b.to_pane_node()),
            },
        }
    }

    fn seed_of(&self, pane: u64) -> Option<&PaneSeed> {
        match self {
            DesiredNode::Leaf { pane: p, seed } => (*p == pane).then_some(seed),
            DesiredNode::Split { a, b, .. } => a.seed_of(pane).or_else(|| b.seed_of(pane)),
        }
    }
}

pub(crate) fn desired_tabs(
    app: &AntermApp,
    cx: &App,
) -> (Vec<DesiredTab>, Option<TabId>, Vec<TabId>) {
    let mut out = Vec::new();
    let mut active = None;
    let mut held = Vec::new();
    for (index, tab) in app.tabs.iter().enumerate() {
        let root = match tab.asleep_layout() {
            Some(layout) => desired_node_from_session(layout),
            None => desired_node(&tab.pane, cx),
        };
        let Some(root) = root else {
            held.push(tab.tree_id.get());
            continue;
        };
        let id = tab.tree_id.get();
        if index == app.active {
            active = Some(id);
        }
        out.push(DesiredTab {
            id,
            name: tab.name.clone(),
            tags: tab.tags.borrow().clone(),
            root,
            hibernated: tab.asleep_layout().is_some(),
        });
    }
    (out, active, held)
}

fn desired_node_from_session(layout: &SessionPane) -> Option<DesiredNode> {
    match layout {
        SessionPane::Leaf {
            cwd,
            pane_id,
            shell,
            ssh_spec,
            agent,
            agent_session_id,
            agent_launch_argv,
        } => {
            let pane = (*pane_id)?;
            Some(DesiredNode::Leaf {
                pane,
                seed: PaneSeed {
                    pane,
                    cwd: cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    ssh_spec: ssh_spec.clone(),
                    agent: agent.map(|agent| AgentFacts {
                        agent,
                        session_id: agent_session_id.clone(),
                        launch_argv: agent_launch_argv.clone(),
                        status: None,
                    }),
                    shell: shell.clone(),
                },
            })
        }
        SessionPane::Split { axis, ratio, a, b } => {
            let left = desired_node_from_session(a);
            let right = desired_node_from_session(b);
            match (left, right) {
                (Some(a), Some(b)) => Some(DesiredNode::Split {
                    axis: match axis {
                        crate::core::session::SessionAxis::Horizontal => TreeAxis::Horizontal,
                        crate::core::session::SessionAxis::Vertical => TreeAxis::Vertical,
                    },
                    ratio: *ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, other) => one.or(other),
            }
        }
    }
}

fn desired_node(pane: &Pane, cx: &App) -> Option<DesiredNode> {
    match pane {
        Pane::Leaf(PaneSlot::Ready(view)) => {
            let view = view.read(cx);
            let ssh_spec = view.ssh_spec();
            let agent = view.agent().map(|agent| {
                let session = view.agent_session();
                AgentFacts {
                    agent,
                    session_id: session.as_ref().and_then(|s| s.session_id.clone()),
                    launch_argv: session.as_ref().and_then(|s| s.launch_argv.clone()),
                    status: None,
                }
            });
            Some(DesiredNode::Leaf {
                pane: view.pane_id,
                seed: PaneSeed {
                    pane: view.pane_id,
                    cwd: view
                        .spawnable_cwd()
                        .map(|p| p.to_string_lossy().into_owned()),
                    ssh_spec,
                    agent,
                    shell: view.shell_spec(),
                },
            })
        }
        Pane::Leaf(PaneSlot::Connecting(pending)) => {
            let spawn = &pending.read(cx).spawn;
            let pane = spawn.restore_pane?;
            let agent = spawn.agent.map(|agent| AgentFacts {
                agent,
                session_id: spawn.agent_session_id.clone(),
                launch_argv: spawn.agent_launch_argv.clone(),
                status: None,
            });
            Some(DesiredNode::Leaf {
                pane,
                seed: PaneSeed {
                    pane,
                    cwd: spawn
                        .working_directory
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    ssh_spec: None,
                    agent,
                    shell: spawn.shell.clone(),
                },
            })
        }
        Pane::Split {
            axis, a, b, ratio, ..
        } => {
            let left = desired_node(a, cx);
            let right = desired_node(b, cx);
            match (left, right) {
                (Some(a), Some(b)) => Some(DesiredNode::Split {
                    axis: match axis {
                        gpui::Axis::Horizontal => TreeAxis::Horizontal,
                        gpui::Axis::Vertical => TreeAxis::Vertical,
                    },
                    ratio: ratio.get(),
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (one, other) => one.or(other),
            }
        }
        Pane::Empty => None,
    }
}

fn seeded_records(desired: &[DesiredTab], live: impl Fn(u64) -> bool) -> Vec<PaneRecord> {
    fn walk(node: &DesiredNode, live: &impl Fn(u64) -> bool, out: &mut Vec<PaneRecord>) {
        match node {
            DesiredNode::Leaf { pane, seed } => out.push(seed.clone().into_record(live(*pane))),
            DesiredNode::Split { a, b, .. } => {
                walk(a, live, out);
                walk(b, live, out);
            }
        }
    }
    let mut out = Vec::new();
    for tab in desired {
        walk(&tab.root, &live, &mut out);
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WsMirror {
    pub tabs: Vec<TreeTab>,
    pub active: Option<TabId>,
    pub tags: SidebarTags,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SyncScope {
    Full,
    Additive,
}

enum SyncPhase {
    Unprimed { dirty: bool, priming: bool },
    Primed(WsMirror),
}

pub(crate) struct TreeSync {
    sync: SyncPhase,
    queue: VecDeque<ControlRequest>,
    inflight: bool,
    informed: bool,
    epoch: u64,
    rehydrate: Option<Adopt>,
    owed_over: Vec<TabId>,
    not_rebuilt: Vec<TabId>,
    rehydrate_attempts: u32,
    then_open: Vec<ParkedOpen>,
    unsent_tags: Option<SidebarTags>,
    retiring: Vec<Retiring>,
    closing: Vec<TabId>,
    retired: Vec<Retiring>,
    said_why_empty: bool,
}

impl Default for TreeSync {
    fn default() -> Self {
        TreeSync {
            sync: SyncPhase::Unprimed {
                dirty: false,
                priming: false,
            },
            queue: VecDeque::new(),
            inflight: false,
            informed: false,
            epoch: 0,
            rehydrate: None,
            owed_over: Vec::new(),
            not_rebuilt: Vec::new(),
            rehydrate_attempts: 0,
            then_open: Vec::new(),
            unsent_tags: None,
            retiring: Vec::new(),
            retired: Vec::new(),
            closing: Vec::new(),
            said_why_empty: false,
        }
    }
}

impl Global for TreeSync {}

pub(crate) fn session_from_tree(machine: &Machine) -> Session {
    let views = anterm_core::tab_view::tab_views_of(&machine.tabs, &machine.panes);
    let tabs: Vec<SessionTab> = machine
        .tabs
        .iter()
        .zip(views)
        .map(|(tab, view)| SessionTab {
            name: tab.name.clone(),
            tree_id: Some(tab.id),
            tags: tab.tags.clone(),
            pane: session_pane_from_node(&tab.root, &machine.panes),
            hibernated: tab.hibernated,
            asleep_view: tab.hibernated.then_some(view),
        })
        .collect();
    let active = machine
        .active_tab
        .and_then(|id| machine.tabs.iter().position(|t| t.id == id))
        .unwrap_or(0);
    Session { active, tabs }
}

fn session_pane_from_node(node: &PaneNode, panes: &[PaneRecord]) -> SessionPane {
    match node {
        PaneNode::Leaf { pane } => {
            let record = panes.iter().find(|p| p.id == *pane);
            let (cwd, ssh_spec, agent, shell) = match record {
                Some(r) => (
                    r.cwd.clone().map(std::path::PathBuf::from),
                    r.ssh_spec.clone(),
                    r.agent.clone(),
                    r.shell.clone(),
                ),
                None => (None, None, None, None),
            };
            SessionPane::Leaf {
                cwd,
                pane_id: Some(*pane),
                shell,
                ssh_spec,
                agent: agent.as_ref().map(|a| a.agent),
                agent_session_id: agent.as_ref().and_then(|a| a.session_id.clone()),
                agent_launch_argv: agent.as_ref().and_then(|a| a.launch_argv.clone()),
            }
        }
        PaneNode::Split { axis, ratio, a, b } => SessionPane::Split {
            axis: match axis {
                TreeAxis::Horizontal => crate::core::session::SessionAxis::Horizontal,
                TreeAxis::Vertical => crate::core::session::SessionAxis::Vertical,
            },
            ratio: *ratio,
            a: Box::new(session_pane_from_node(a, panes)),
            b: Box::new(session_pane_from_node(b, panes)),
        },
    }
}

/// A tab parked until the window's layout lands.
enum ParkedOpen {
    Folder(std::path::PathBuf),
    Command {
        cwd: std::path::PathBuf,
        command: String,
    },
    Ssh(anterm_core::ssh_profile::QuickConnect),
}

#[derive(Clone, Copy, PartialEq)]
enum Adopt {
    IfEmpty,
    Replace,
}
