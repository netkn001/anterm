use std::collections::HashMap;
use std::io;

use gpui::App;
use gpui_kit::component::WindowExt as _;
use anterm_core::daemon::control::{ControlClient, ControlRequest, ReplyOk};
use anterm_core::machine::{
    Axis as TreeAxis, LayoutDelta, Machine, PaneNode, Side, Tab as TreeTab, TabId,
};

use crate::core::tag::SidebarTags;
use crate::core::session::Session;
use crate::ui::workspace::app::AntermApp;
use crate::ui::workspace::pane::{Pane, PaneSlot};
use anterm_i18n::{L10nKey, t};

use super::{
    Adopt, DesiredNode, DesiredTab, ParkedOpen, SyncPhase, SyncScope, TreeLink, TreeSync,
    WsMirror, desired_tabs, end_unsent_retirements, remember_closes, seeded_records,
    session_from_tree, tree_control, unaccounted,
};

pub(crate) fn diff(
    mirror: &mut WsMirror,
    desired: &[DesiredTab],
    desired_active: Option<TabId>,
    scope: SyncScope,
    held: &[TabId],
) -> Vec<ControlRequest> {
    let mut ops = Vec::new();

    if scope == SyncScope::Full {
        migrate_panes(mirror, desired, &mut ops);
        let mut index = 0;
        while index < mirror.tabs.len() {
            let id = mirror.tabs[index].id;
            if desired.iter().any(|t| t.id == id) || held.contains(&id) {
                index += 1;
                continue;
            }
            let closed = mirror.tabs.remove(index);
            ops.push(ControlRequest::TabClose { tab: closed.id });
            heal_active(mirror, index);
        }
    }

    for want in desired {
        if let Some(at) = mirror.tabs.iter().position(|t| t.id == want.id) {
            reconcile_tab(mirror, at, want, &mut ops);
        }
    }
    for (index, want) in desired.iter().enumerate() {
        if mirror.tabs.iter().any(|t| t.id == want.id) {
            continue;
        }
        let at = match scope {
            SyncScope::Full => index,
            SyncScope::Additive => mirror.tabs.len(),
        };
        create_tab(mirror, at, want, &mut ops);
    }

    if scope == SyncScope::Additive || !held.is_empty() {
        return ops;
    }

    for (index, want) in desired.iter().enumerate() {
        let at = mirror
            .tabs
            .iter()
            .position(|t| t.id == want.id)
            .expect("every desired tab exists after the passes above");
        if at != index {
            let tab = mirror.tabs.remove(at);
            mirror.tabs.insert(index, tab);
            ops.push(ControlRequest::TabMove {
                tab: want.id,
                to: index as u64,
            });
        }
    }

    if let Some(active) = desired_active
        && mirror.active != Some(active)
        && mirror.tabs.iter().any(|t| t.id == active)
    {
        mirror.active = Some(active);
        ops.push(ControlRequest::SetActiveTab { tab: active });
    }

    ops
}

fn migrate_panes(mirror: &mut WsMirror, desired: &[DesiredTab], ops: &mut Vec<ControlRequest>) {
    while let Some((pane, to, axis, first)) = next_migration(mirror, desired) {
        let holders = |m: &WsMirror, p: u64| m.tabs.iter().position(|t| t.root.contains(p));
        let (Some(from), Some(dest)) = (holders(mirror, pane), holders(mirror, to)) else {
            return;
        };
        if !mirror.tabs[dest]
            .root
            .split_leaf(to, pane, axis, 0.5, first)
        {
            return;
        }
        if mirror.tabs[from].root.remove_leaf(pane).is_none() {
            mirror.tabs.remove(from);
            heal_active(mirror, from);
        }
        ops.push(ControlRequest::PaneMove {
            pane,
            to,
            axis,
            first,
        });
    }
}

fn next_migration(mirror: &WsMirror, desired: &[DesiredTab]) -> Option<(u64, u64, TreeAxis, bool)> {
    for want in desired {
        let Some(at) = mirror.tabs.iter().position(|t| t.id == want.id) else {
            continue;
        };
        let root = want.root.to_pane_node();
        let arriving = |node: &PaneNode| {
            let ids = node.pane_ids();
            !ids.is_empty()
                && ids.iter().all(|p| {
                    mirror
                        .tabs
                        .iter()
                        .position(|t| t.root.contains(*p))
                        .is_some_and(|holder| holder != at)
                })
        };
        let settled = |node: &PaneNode| match node {
            PaneNode::Leaf { pane } => mirror.tabs[at].root.contains(*pane).then_some(*pane),
            _ => None,
        };
        if let Some(step) = arrival_site(&root, &arriving, &settled) {
            return Some(step);
        }
    }
    None
}

fn arrival_site(
    node: &PaneNode,
    arriving: &impl Fn(&PaneNode) -> bool,
    settled: &impl Fn(&PaneNode) -> Option<u64>,
) -> Option<(u64, u64, TreeAxis, bool)> {
    let PaneNode::Split { axis, a, b, .. } = node else {
        return None;
    };
    let first = |side: &PaneNode| side.pane_ids().first().copied();
    if let (Some(to), true) = (settled(a), arriving(b)) {
        return Some((first(b)?, to, *axis, false));
    }
    if let (true, Some(to)) = (arriving(a), settled(b)) {
        return Some((first(a)?, to, *axis, true));
    }
    arrival_site(a, arriving, settled).or_else(|| arrival_site(b, arriving, settled))
}

fn heal_active(mirror: &mut WsMirror, removed: usize) {
    let named = mirror
        .active
        .is_some_and(|active| mirror.tabs.iter().any(|t| t.id == active));
    if named {
        return;
    }
    if mirror.tabs.is_empty() {
        mirror.active = None;
        return;
    }
    mirror.active = Some(mirror.tabs[removed.min(mirror.tabs.len() - 1)].id);
}

fn create_tab(
    mirror: &mut WsMirror,
    index: usize,
    want: &DesiredTab,
    ops: &mut Vec<ControlRequest>,
) {
    let (first, seed) = want.root.first_leaf();
    ops.push(ControlRequest::TabCreate {
        at: Some(index as u64),
        pane: seed.clone(),
        tab: Some(want.id),
    });
    let mut root = PaneNode::Leaf { pane: *first };
    materialize_splits(&want.root, &mut root, ops);
    if want.name.is_some() {
        ops.push(ControlRequest::TabRename {
            tab: want.id,
            name: want.name.clone(),
        });
    }
    if !want.tags.is_empty() {
        ops.push(ControlRequest::TabSetTags {
            tab: want.id,
            tags: want.tags.clone(),
        });
    }
    if want.hibernated {
        ops.push(ControlRequest::TabSetHibernated {
            tab: want.id,
            hibernated: true,
        });
    }
    mirror.tabs.insert(
        index.min(mirror.tabs.len()),
        TreeTab {
            id: want.id,
            name: want.name.clone(),
            tags: want.tags.clone(),
            root,
            hibernated: want.hibernated,
        },
    );
    mirror.active = Some(want.id);
}

fn materialize_splits(want: &DesiredNode, root: &mut PaneNode, ops: &mut Vec<ControlRequest>) {
    let DesiredNode::Split { axis, ratio, a, b } = want else {
        return;
    };
    let (anchor, _) = a.first_leaf();
    let (new, seed) = b.first_leaf();
    ops.push(ControlRequest::PaneSplit {
        pane: *anchor,
        axis: *axis,
        ratio: *ratio,
        new: seed.clone(),
        first: false,
    });
    root.split_leaf(*anchor, *new, *axis, *ratio, false);
    materialize_splits(a, root, ops);
    materialize_splits(b, root, ops);
}

fn reconcile_tab(
    mirror: &mut WsMirror,
    at: usize,
    want: &DesiredTab,
    ops: &mut Vec<ControlRequest>,
) {
    {
        let tab = &mut mirror.tabs[at];
        if tab.name != want.name {
            tab.name = want.name.clone();
            ops.push(ControlRequest::TabRename {
                tab: want.id,
                name: want.name.clone(),
            });
        }
        if tab.tags != want.tags {
            tab.tags = want.tags.clone();
            ops.push(ControlRequest::TabSetTags {
                tab: want.id,
                tags: want.tags.clone(),
            });
        }
        if tab.hibernated != want.hibernated {
            tab.hibernated = want.hibernated;
            ops.push(ControlRequest::TabSetHibernated {
                tab: want.id,
                hibernated: want.hibernated,
            });
        }
    }

    let desired_root = want.root.to_pane_node();
    if mirror.tabs[at].root == desired_root {
        return;
    }
    if same_shape_and_panes(&mirror.tabs[at].root, &desired_root) {
        fix_ratios(want.id, &mut mirror.tabs[at].root, &desired_root, ops);
        return;
    }

    let have = mirror.tabs[at].root.pane_ids();
    let wanted = desired_root.pane_ids();
    let added: Vec<u64> = wanted
        .iter()
        .copied()
        .filter(|p| !have.contains(p))
        .collect();
    let removed: Vec<u64> = have
        .iter()
        .copied()
        .filter(|p| !wanted.contains(p))
        .collect();

    let done = match (added.as_slice(), removed.as_slice()) {
        ([new], []) => try_single_split(mirror, at, want, &desired_root, *new, ops),
        ([], []) => try_single_move(mirror, at, &desired_root, ops),
        ([], gone) if !gone.is_empty() => {
            for pane in gone {
                mirror.tabs[at].root.remove_leaf(*pane);
                ops.push(ControlRequest::PaneClose { pane: *pane });
            }
            same_shape_and_panes(&mirror.tabs[at].root, &desired_root)
        }
        ([new], [old]) => {
            let elsewhere = mirror
                .tabs
                .iter()
                .enumerate()
                .any(|(i, t)| i != at && t.root.contains(*new));
            let mut predicted = mirror.tabs[at].root.clone();
            predicted.replace_leaf(*old, *new);
            if !elsewhere && same_shape_and_panes(&predicted, &desired_root) {
                let seed = want
                    .root
                    .seed_of(*new)
                    .expect("the added pane is a desired leaf")
                    .clone();
                mirror.tabs[at].root = predicted;
                ops.push(ControlRequest::PaneReplace {
                    old: *old,
                    new: seed,
                });
                true
            } else {
                false
            }
        }
        _ => false,
    };

    if done {
        fix_ratios(want.id, &mut mirror.tabs[at].root, &desired_root, ops);
        return;
    }

    let closed = mirror.tabs.remove(at);
    ops.push(ControlRequest::TabClose { tab: closed.id });
    heal_active(mirror, at);
    create_tab(mirror, at, want, ops);
}

fn try_single_split(
    mirror: &mut WsMirror,
    at: usize,
    want: &DesiredTab,
    desired_root: &PaneNode,
    new: u64,
    ops: &mut Vec<ControlRequest>,
) -> bool {
    let Some((sibling, axis, ratio, first)) = split_site(desired_root, new) else {
        return false;
    };
    let mut predicted = mirror.tabs[at].root.clone();
    if !predicted.split_leaf(sibling, new, axis, ratio, first) {
        return false;
    }
    if !same_shape_and_panes(&predicted, desired_root) {
        return false;
    }
    let seed = want
        .root
        .seed_of(new)
        .expect("the added pane is a desired leaf")
        .clone();
    mirror.tabs[at].root = predicted;
    ops.push(ControlRequest::PaneSplit {
        pane: sibling,
        axis,
        ratio,
        new: seed,
        first,
    });
    true
}

fn try_single_move(
    mirror: &mut WsMirror,
    at: usize,
    desired_root: &PaneNode,
    ops: &mut Vec<ControlRequest>,
) -> bool {
    for pane in mirror.tabs[at].root.pane_ids() {
        let Some((to, axis, _, first)) = split_site(desired_root, pane) else {
            continue;
        };
        let mut predicted = mirror.tabs[at].root.clone();
        if predicted.remove_leaf(pane) != Some(true) {
            continue;
        }
        if !predicted.split_leaf(to, pane, axis, 0.5, first)
            || !same_shape_and_panes(&predicted, desired_root)
        {
            continue;
        }
        mirror.tabs[at].root = predicted;
        ops.push(ControlRequest::PaneMove {
            pane,
            to,
            axis,
            first,
        });
        return true;
    }
    false
}

fn split_site(node: &PaneNode, new: u64) -> Option<(u64, TreeAxis, f32, bool)> {
    let PaneNode::Split { axis, ratio, a, b } = node else {
        return None;
    };
    match (&**a, &**b) {
        (PaneNode::Leaf { pane }, sibling) if *pane == new => {
            if let PaneNode::Leaf { pane: s } = sibling {
                return Some((*s, *axis, *ratio, true));
            }
            return None;
        }
        (sibling, PaneNode::Leaf { pane }) if *pane == new => {
            if let PaneNode::Leaf { pane: s } = sibling {
                return Some((*s, *axis, *ratio, false));
            }
            return None;
        }
        _ => {}
    }
    if a.contains(new) {
        split_site(a, new)
    } else if b.contains(new) {
        split_site(b, new)
    } else {
        None
    }
}

fn same_shape_and_panes(a: &PaneNode, b: &PaneNode) -> bool {
    match (a, b) {
        (PaneNode::Leaf { pane: pa }, PaneNode::Leaf { pane: pb }) => pa == pb,
        (
            PaneNode::Split {
                axis: ax,
                a: aa,
                b: ab,
                ..
            },
            PaneNode::Split {
                axis: bx,
                a: ba,
                b: bb,
                ..
            },
        ) => ax == bx && same_shape_and_panes(aa, ba) && same_shape_and_panes(ab, bb),
        _ => false,
    }
}

fn fix_ratios(
    tab: TabId,
    mirror: &mut PaneNode,
    desired: &PaneNode,
    ops: &mut Vec<ControlRequest>,
) {
    fn walk(
        tab: TabId,
        mirror: &mut PaneNode,
        desired: &PaneNode,
        path: &mut Vec<Side>,
        ops: &mut Vec<ControlRequest>,
    ) {
        let (
            PaneNode::Split {
                ratio: mr,
                a: ma,
                b: mb,
                ..
            },
            PaneNode::Split {
                ratio: dr,
                a: da,
                b: db,
                ..
            },
        ) = (mirror, desired)
        else {
            return;
        };
        if (*mr - *dr).abs() > 1e-4 {
            *mr = *dr;
            ops.push(ControlRequest::PaneSetRatio {
                tab,
                path: path.clone(),
                ratio: *dr,
            });
        }
        path.push(Side::A);
        walk(tab, ma, da, path, ops);
        path.pop();
        path.push(Side::B);
        walk(tab, mb, db, path, ops);
        path.pop();
    }
    let mut path = Vec::new();
    walk(tab, mirror, desired, &mut path, ops);
}

pub(crate) fn sync_window(app: &AntermApp, cx: &mut App) {
    let showing: Vec<TabId> = app.tabs.iter().map(|t| t.tree_id.get()).collect();
    if let Some(adopt) = take_rehydrate(cx, &showing) {
        hydrate_with(cx, adopt, showing);
        return;
    }
    adopt_tab_ids(app, cx);
    let (desired, desired_active, mut held) = desired_tabs(app, cx);

    let state = cx.default_global::<TreeSync>();
    match &mut state.sync {
        SyncPhase::Unprimed { dirty, priming } => {
            *dirty = true;
            if !*priming {
                *priming = true;
                start_prime(cx);
            }
        }
        SyncPhase::Primed(mirror) => {
            let scope = if state.informed {
                SyncScope::Full
            } else {
                SyncScope::Additive
            };
            state
                .not_rebuilt
                .retain(|id| mirror.tabs.iter().any(|t| t.id == *id));
            held.extend(state.not_rebuilt.iter().copied());
            let mut missing = unaccounted(mirror, &desired, &mut state.closing);
            missing.retain(|id| !held.contains(id));
            if scope == SyncScope::Full && !missing.is_empty() {
                log::warn!(
                    "the window is not showing {} tab(s) the user did not close here; leaving \
                     them on the machine",
                    missing.len()
                );
            }
            held.extend(missing);
            let mut ops = diff(mirror, &desired, desired_active, scope, &held);
            let sent = remember_closes(&mut ops, &mut state.retiring);
            let remembered: Vec<TabId> = sent.iter().map(|r| r.tab).collect();
            state.retired.extend(sent);
            if !ops.is_empty() {
                let (tabs, active) = (mirror.tabs.clone(), mirror.active);
                state.queue.extend(ops);
                crate::ui::remote::machine_mirror::MachineMirrors::note_remembered_closes(
                    cx, remembered,
                );
                crate::ui::remote::machine_mirror::MachineMirrors::note_synced_tabs(
                    cx, tabs, active,
                );
                let open: Vec<u64> = app
                    .tabs
                    .iter()
                    .flat_map(|t| t.pane.terminals())
                    .map(|v| v.read(cx).pane_id)
                    .collect();
                crate::ui::remote::machine_mirror::MachineMirrors::note_seeded_panes(
                    cx,
                    seeded_records(&desired, |pane| open.contains(&pane)),
                );
                pump(cx);
            }
        }
    }
}

pub(crate) fn on_link_up(cx: &mut App) {
    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return;
    }
    if let Some((_, app)) = crate::ui::workspace::windows::WindowRegistry::primary(cx)
        && let Some(app) = app.upgrade()
    {
        app.update(cx, |app, cx| sync_window(app, cx));
    }
}

pub(super) fn take_rehydrate(cx: &mut App, showing: &[TabId]) -> Option<Adopt> {
    let state = cx.default_global::<TreeSync>();
    let adopt = state.rehydrate.take()?;
    let overtaken = showing.iter().any(|id| !state.owed_over.contains(id));
    if overtaken && adopt == Adopt::Replace {
        state.rehydrate_attempts = 0;
        return None;
    }
    Some(adopt)
}

pub(crate) fn mark_window_informed(cx: &mut App) {
    cx.default_global::<TreeSync>().informed = true;
}

fn adopt_tab_ids(app: &AntermApp, cx: &App) {
    let Some(TreeSync {
        sync: SyncPhase::Primed(mirror),
        ..
    }) = cx.try_global::<TreeSync>()
    else {
        return;
    };
    let known: Vec<TabId> = app.tabs.iter().map(|t| t.tree_id.get()).collect();
    for tab in &app.tabs {
        let id = tab.tree_id.get();
        if mirror.tabs.iter().any(|m| m.id == id) {
            continue;
        }
        let panes: Vec<u64> = tab
            .pane
            .terminals()
            .iter()
            .map(|v| v.read(cx).pane_id)
            .collect();
        if panes.is_empty() {
            continue;
        }
        let Some(matched) = mirror
            .tabs
            .iter()
            .find(|m| !known.contains(&m.id) && panes.iter().any(|p| m.root.contains(*p)))
        else {
            continue;
        };
        tab.tree_id.set(matched.id);
    }
}

pub(crate) fn forget(cx: &mut App) {
    if cx.try_global::<TreeSync>().is_some() {
        clear_queue(cx);
        *cx.default_global::<TreeSync>() = TreeSync::default();
    }
}

fn clear_queue(cx: &mut App) {
    let dropped: Vec<ControlRequest> = cx.default_global::<TreeSync>().queue.drain(..).collect();
    let mut retired = std::mem::take(&mut cx.default_global::<TreeSync>().retired);
    end_unsent_retirements(cx, &mut retired, &dropped);
    cx.default_global::<TreeSync>().retired.extend(retired);
}

#[cfg(test)]
pub(crate) fn prime_for_test(cx: &mut App, tabs: Vec<TreeTab>) {
    let state = cx.default_global::<TreeSync>();
    state.sync = SyncPhase::Primed(WsMirror {
        tabs,
        ..Default::default()
    });
    state.informed = true;
    state.inflight = true;
}

#[cfg(test)]
pub(crate) fn queued_for_test(cx: &mut App) -> Vec<ControlRequest> {
    cx.default_global::<TreeSync>()
        .queue
        .iter()
        .cloned()
        .collect()
}

fn start_prime(cx: &mut App) {
    let client = match tree_control(cx) {
        TreeLink::Ready(client) => client,
        unavailable => {
            if matches!(unavailable, TreeLink::Unserved) {
                log::warn!(
                    "the local server does not serve the machine tree; layout will not be synced"
                );
            }
            if let SyncPhase::Unprimed { priming, .. } = &mut cx.default_global::<TreeSync>().sync {
                *priming = false;
            }
            return;
        }
    };
    let epoch = cx.default_global::<TreeSync>().epoch;
    cx.spawn(async move |cx| {
        let outcome = cx
            .background_executor()
            .spawn(async move { pull_mirror(&client) })
            .await;
        cx.update(|cx| finish_prime(cx, epoch, outcome));
    })
    .detach();
}

fn pull_mirror(client: &ControlClient) -> io::Result<WsMirror> {
    let machine = machine_get(client)?;
    Ok(WsMirror {
        tabs: machine.tabs,
        active: machine.active_tab,
        tags: machine.tags,
    })
}

pub(super) fn finish_prime(cx: &mut App, epoch: u64, outcome: io::Result<WsMirror>) {
    let state = cx.default_global::<TreeSync>();
    if state.epoch != epoch || !matches!(state.sync, SyncPhase::Unprimed { priming: true, .. }) {
        log::debug!("dropping a superseded tree pull");
        return;
    }
    let was_dirty = matches!(state.sync, SyncPhase::Unprimed { dirty: true, .. });
    let landed = match outcome {
        Ok(mirror) => {
            state.informed |= mirror.tabs.is_empty();
            state.rehydrate_attempts = 0;
            let landed = (mirror.tabs.clone(), mirror.active);
            state.sync = SyncPhase::Primed(mirror);
            landed
        }
        Err(e) => {
            log::warn!("could not pull the machine tree: {e}");
            state.sync = SyncPhase::Unprimed {
                dirty: was_dirty,
                priming: false,
            };
            return;
        }
    };
    adopt_tags(cx);
    crate::ui::remote::machine_mirror::MachineMirrors::note_synced_tabs(cx, landed.0, landed.1);
    if !was_dirty {
        return;
    }
    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return;
    }
    let Some((_, app)) = crate::ui::workspace::windows::WindowRegistry::primary(cx) else {
        return;
    };
    let Some(app) = app.upgrade() else {
        return;
    };
    app.update(cx, |app, cx| sync_window(app, cx));
}

pub(crate) fn push_tags(cx: &mut App, tags: SidebarTags) {
    let state = cx.default_global::<TreeSync>();
    match &mut state.sync {
        SyncPhase::Primed(mirror) => {
            if mirror.tags == tags {
                return;
            }
            mirror.tags = tags.clone();
            state.queue.push_back(ControlRequest::SetTags { tags });
            pump(cx);
        }
        SyncPhase::Unprimed { .. } => state.unsent_tags = Some(tags),
    }
}

fn adopt_tags(cx: &mut App) {
    if let Some(unsent) = cx.default_global::<TreeSync>().unsent_tags.take() {
        push_tags(cx, unsent);
        return;
    }
    cx.defer(move |cx| {
        let tags = match &cx.default_global::<TreeSync>().sync {
            SyncPhase::Primed(mirror) => mirror.tags.clone(),
            _ => return,
        };
        if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
            return;
        }
        for (_, app) in crate::ui::workspace::windows::WindowRegistry::open_windows(cx) {
            if let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| app.adopt_sidebar_tags(tags.clone(), cx));
            }
        }
    });
}

fn pump(cx: &mut App) {
    let client = tree_control(cx);
    let state = cx.default_global::<TreeSync>();
    if state.inflight || state.queue.is_empty() {
        return;
    }
    let client = match client {
        TreeLink::Ready(client) => client,
        TreeLink::Unserved => {
            desync(cx, "the server does not serve the machine tree");
            return;
        }
        TreeLink::Down => {
            desync(cx, "the control link is down");
            return;
        }
    };
    let batch: Vec<ControlRequest> = state.queue.drain(..).collect();
    let remembered: Vec<TabId> = batch
        .iter()
        .filter_map(|op| match op {
            ControlRequest::TabCloseRemembered { tab, .. } => Some(*tab),
            _ => None,
        })
        .collect();
    state.inflight = true;
    cx.spawn(async move |cx| {
        let result = cx
            .background_executor()
            .spawn(async move {
                let mut ops = batch.into_iter();
                while let Some(op) = ops.next() {
                    if let Err(e) = client.call(op.clone()) {
                        let unsent: Vec<ControlRequest> =
                            std::iter::once(op.clone()).chain(ops).collect();
                        return Err((op, e, unsent));
                    }
                }
                Ok(())
            })
            .await;
        cx.update(|cx| {
            let mut retired = {
                let state = cx.default_global::<TreeSync>();
                state.inflight = false;
                std::mem::take(&mut state.retired)
            };
            if let Err((_, _, unsent)) = &result {
                end_unsent_retirements(cx, &mut retired, unsent);
            }
            retired.retain(|r| !remembered.contains(&r.tab));
            cx.default_global::<TreeSync>().retired.extend(retired);
            match result {
                Ok(()) => pump(cx),
                Err((op, e, _)) => {
                    log::warn!("tree operation {op:?} failed: {e}; re-pulling the tree");
                    desync(cx, "an operation was refused");
                }
            }
        });
    })
    .detach();
}

fn desync(cx: &mut App, why: &str) {
    log::info!("resynchronizing window with local daemon ({why})");
    clear_queue(cx);
    let state = cx.default_global::<TreeSync>();
    state.inflight = false;
    state.sync = SyncPhase::Unprimed {
        dirty: true,
        priming: true,
    };
    state.epoch += 1;
    start_prime(cx);
}

const HYDRATE_LINK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
const HYDRATE_LINK_POLL: std::time::Duration = std::time::Duration::from_millis(200);

pub(crate) fn hydrate_window_from_tree(cx: &mut App) {
    hydrate(cx, Adopt::IfEmpty);
}

pub(crate) fn hydrate_window_then_open(cx: &mut App, path: std::path::PathBuf) {
    cx.default_global::<TreeSync>()
        .then_open
        .push(ParkedOpen::Folder(path));
    hydrate(cx, Adopt::IfEmpty);
}

pub(crate) fn park_command_while_pulling(
    cx: &mut App,
    cwd: &std::path::Path,
    command: &str,
) -> bool {
    if !layout_is_pending(cx) {
        return false;
    }
    let parked = &mut cx.default_global::<TreeSync>().then_open;
    match parked
        .iter_mut()
        .find(|open| matches!(open, ParkedOpen::Folder(folder) if folder == cwd))
    {
        Some(open) => {
            *open = ParkedOpen::Command {
                cwd: cwd.to_path_buf(),
                command: command.to_owned(),
            }
        }
        None => parked.push(ParkedOpen::Command {
            cwd: cwd.to_path_buf(),
            command: command.to_owned(),
        }),
    }
    true
}

pub(crate) fn park_ssh_while_pulling(
    cx: &mut App,
    ssh: &anterm_core::ssh_profile::QuickConnect,
) -> bool {
    if !layout_is_pending(cx) {
        return false;
    }
    cx.default_global::<TreeSync>()
        .then_open
        .push(ParkedOpen::Ssh(ssh.clone()));
    true
}

fn layout_is_pending(cx: &mut App) -> bool {
    let state = cx.default_global::<TreeSync>();
    matches!(state.sync, SyncPhase::Unprimed { priming: true, .. })
        || state.rehydrate.is_some()
        || !state.then_open.is_empty()
}

pub(super) fn open_parked_path(cx: &mut App) {
    let parked = std::mem::take(&mut cx.default_global::<TreeSync>().then_open);
    if parked.is_empty() {
        return;
    }
    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return;
    }
    let Some((handle, app)) = crate::ui::workspace::windows::WindowRegistry::primary(cx) else {
        return;
    };
    let Some(app) = app.upgrade() else {
        return;
    };
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| {
            for open in parked {
                match open {
                    ParkedOpen::Folder(cwd) => app.new_tab_at(cwd, window, cx),
                    ParkedOpen::Command { cwd, command } => {
                        app.new_tab_running(cwd, command, window, cx)
                    }
                    ParkedOpen::Ssh(ssh) => app.quick_connect(ssh, window, cx),
                }
            }
        });
    });
}

fn tabs_on_screen(cx: &mut App) -> Vec<TabId> {
    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return Vec::new();
    }
    crate::ui::workspace::windows::WindowRegistry::primary(cx)
        .and_then(|(_, app)| app.upgrade())
        .map(|app| app.read(cx).tabs.iter().map(|t| t.tree_id.get()).collect())
        .unwrap_or_default()
}

fn hydrate(cx: &mut App, adopt: Adopt) {
    let showing = tabs_on_screen(cx);
    hydrate_with(cx, adopt, showing);
}

pub(super) fn hydrate_with(cx: &mut App, adopt: Adopt, showing: Vec<TabId>) {
    clear_queue(cx);
    let (epoch, failures) = {
        let state = cx.default_global::<TreeSync>();
        state.sync = SyncPhase::Unprimed {
            dirty: false,
            priming: true,
        };
        state.epoch += 1;
        state.rehydrate = None;
        state.owed_over = showing;
        if adopt == Adopt::Replace {
            state.informed = false;
        }
        (state.epoch, state.rehydrate_attempts)
    };
    let level = hydration_log_level(failures, log::Level::Warn);
    cx.spawn(async move |cx| {
        let deadline = std::time::Instant::now() + HYDRATE_LINK_DEADLINE;
        let client = loop {
            match cx.update(tree_control) {
                TreeLink::Ready(client) => break Some(client),
                TreeLink::Unserved => {
                    log::log!(
                        level,
                        "the local server does not serve the machine tree; opening empty"
                    );
                    break None;
                }
                TreeLink::Down if std::time::Instant::now() > deadline => {
                    log::log!(level, "no link to the local daemon; opening empty");
                    break None;
                }
                TreeLink::Down => cx.background_executor().timer(HYDRATE_LINK_POLL).await,
            }
        };
        let Some(client) = client else {
            cx.update(|cx| {
                if owe_rehydration(cx, epoch, adopt) && adopt == Adopt::IfEmpty {
                    say_why_the_window_is_empty(cx);
                }
            });
            return;
        };
        let outcome = cx
            .background_executor()
            .spawn(async move { pull_machine(&client) })
            .await;
        cx.update(|cx| finish_hydration(cx, epoch, adopt, outcome));
    })
    .detach();
}

fn say_why_the_window_is_empty(cx: &mut App) {
    let state = cx.default_global::<TreeSync>();
    if state.said_why_empty {
        return;
    }
    state.said_why_empty = true;
    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return;
    }
    let Some((handle, _)) = crate::ui::workspace::windows::WindowRegistry::primary(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.push_notification(t(L10nKey::TreeWindowOpenedEmpty), cx);
    });
}

pub(super) fn owe_rehydration(cx: &mut App, epoch: u64, adopt: Adopt) -> bool {
    let state = cx.default_global::<TreeSync>();
    if state.epoch != epoch {
        return false;
    }
    if let SyncPhase::Unprimed { priming, .. } = &mut state.sync {
        *priming = false;
    }
    state.rehydrate = Some(adopt);
    state.rehydrate_attempts = state.rehydrate_attempts.saturating_add(1);
    let attempts = state.rehydrate_attempts;
    log::log!(
        hydration_log_level(attempts, log::Level::Info),
        "will pull layout again once local daemon answers (attempt {attempts})"
    );
    arm_rehydrate_retry(cx, epoch, attempts);
    true
}

pub(super) fn still_owed(cx: &App, epoch: u64) -> bool {
    cx.try_global::<TreeSync>()
        .is_some_and(|s| s.rehydrate.is_some() && s.epoch == epoch)
}

pub(super) const REHYDRATE_SETTLED: u32 = 5;
const REHYDRATE_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) fn rehydrate_backoff(attempts: u32) -> std::time::Duration {
    std::time::Duration::from_secs(2u64.saturating_pow(attempts.min(REHYDRATE_SETTLED)))
        .min(REHYDRATE_BACKOFF_CAP)
}

pub(super) fn hydration_log_level(attempts: u32, fresh: log::Level) -> log::Level {
    if attempts >= REHYDRATE_SETTLED {
        log::Level::Debug
    } else {
        fresh
    }
}

fn arm_rehydrate_retry(cx: &mut App, epoch: u64, attempts: u32) {
    let delay = rehydrate_backoff(attempts);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        let _ = cx.update(|cx| {
            if !still_owed(cx, epoch) {
                return;
            }
            if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
                return;
            }
            let Some((_, app)) = crate::ui::workspace::windows::WindowRegistry::primary(cx) else {
                return;
            };
            let Some(app) = app.upgrade() else {
                return;
            };
            app.update(cx, |app, cx| sync_window(app, cx));
        });
    })
    .detach();
}

fn pull_machine(client: &ControlClient) -> io::Result<(Machine, WsMirror, Session)> {
    let machine = machine_get(client)?;
    let mirror = WsMirror {
        tabs: machine.tabs.clone(),
        active: machine.active_tab,
        tags: machine.tags.clone(),
    };
    let session = session_from_tree(&machine);
    Ok((machine, mirror, session))
}

fn machine_get(client: &ControlClient) -> io::Result<Machine> {
    match client.call(ControlRequest::MachineGet)? {
        ReplyOk::MachineTree(m) => Ok(*m),
        other => Err(io::Error::other(format!("MachineGet answered {other:?}"))),
    }
}

fn finish_hydration(
    cx: &mut App,
    epoch: u64,
    adopt: Adopt,
    outcome: io::Result<(Machine, WsMirror, Session)>,
) {
    if settle_hydration(cx, epoch, adopt, outcome) {
        open_parked_path(cx);
    }
}

pub(super) fn settle_hydration(
    cx: &mut App,
    epoch: u64,
    adopt: Adopt,
    outcome: io::Result<(Machine, WsMirror, Session)>,
) -> bool {
    let current = cx.default_global::<TreeSync>().epoch;
    if current != epoch {
        log::debug!("dropping a superseded hydration");
        return false;
    }
    let (machine, mirror, session) = match outcome {
        Ok(pulled) => pulled,
        Err(e) => {
            let failures = cx.default_global::<TreeSync>().rehydrate_attempts;
            log::log!(
                hydration_log_level(failures, log::Level::Warn),
                "could not hydrate window from local daemon: {e}"
            );
            let _ = owe_rehydration(cx, epoch, adopt);
            return false;
        }
    };
    crate::ui::remote::machine_mirror::MachineMirrors::install(cx, machine);
    let machine_was_empty = mirror.tabs.is_empty();
    let was_dirty = {
        let state = cx.default_global::<TreeSync>();
        let dirty = matches!(state.sync, SyncPhase::Unprimed { dirty: true, .. });
        state.informed |= machine_was_empty;
        state.sync = SyncPhase::Primed(mirror);
        state.rehydrate_attempts = 0;
        state.said_why_empty = false;
        dirty
    };
    adopt_tags(cx);
    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return false;
    }
    let Some((handle, app_weak)) = crate::ui::workspace::windows::WindowRegistry::primary(cx)
    else {
        return false;
    };
    let Some(app) = app_weak.upgrade() else {
        return false;
    };
    if adopt == Adopt::IfEmpty && !app.read(cx).tabs.is_empty() {
        if was_dirty || machine_was_empty {
            app.update(cx, |app, cx| sync_window(app, cx));
        }
        return true;
    }
    if session.tabs.is_empty() && adopt == Adopt::IfEmpty {
        if was_dirty {
            app.update(cx, |app, cx| sync_window(app, cx));
        }
        return true;
    }
    let wanted = session.tabs.len();
    let wanted_ids: Vec<TabId> = session.tabs.iter().filter_map(|t| t.tree_id).collect();
    debug_assert_eq!(
        wanted_ids.len(),
        wanted,
        "a session rebuilt from the tree names every tab it holds"
    );
    log::info!("rebuilding {wanted} tab(s) from the machine's tree");
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| app.adopt_session(session, window, cx));
    });
    let showing = tabs_on_screen(cx);
    settle_rebuild(cx, wanted, &wanted_ids, &showing);
    true
}

pub(super) fn settle_rebuild(
    cx: &mut App,
    wanted: usize,
    wanted_ids: &[TabId],
    showing: &[TabId],
) {
    let not_rebuilt: Vec<TabId> = wanted_ids
        .iter()
        .copied()
        .filter(|id| !showing.contains(id))
        .collect();
    if !not_rebuilt.is_empty() && !showing.is_empty() {
        log::warn!(
            "{} of {wanted} tab(s) could not be rebuilt; they stay on the machine, held out of \
             this window's sync",
            not_rebuilt.len()
        );
    }
    log::info!(
        "rebuild complete; requested={wanted} restored={} failed={}",
        wanted.saturating_sub(not_rebuilt.len()),
        not_rebuilt.len()
    );
    let rebuilt = !showing.is_empty();
    cx.default_global::<TreeSync>().not_rebuilt = not_rebuilt;
    if rebuilt || wanted == 0 {
        mark_window_informed(cx);
    } else {
        log::warn!(
            "none of {wanted} tab(s) could be rebuilt; leaving the window uninformed so the \
             layout is not mistaken for an empty session"
        );
    }
}

pub(crate) fn on_layout_delta(cx: &mut App, delta: LayoutDelta) {
    let nothing_shown =
        crate::ui::remote::machine_mirror::MachineMirrors::delta_changes_nothing_shown(cx, &delta);
    crate::ui::remote::machine_mirror::MachineMirrors::apply_delta(cx, &delta);

    let mirror_ok = match &mut cx.default_global::<TreeSync>().sync {
        SyncPhase::Primed(mirror) => apply_to_mirror(mirror, &delta),
        _ => return,
    };

    if !cx.has_global::<crate::ui::workspace::windows::WindowRegistry>() {
        return;
    }
    let Some((handle, app_weak)) = crate::ui::workspace::windows::WindowRegistry::primary(cx)
    else {
        return;
    };
    let Some(app) = app_weak.upgrade() else {
        return;
    };
    let window_ok = nothing_shown
        || handle
            .update(cx, |_, window, cx| {
                app.update(cx, |app, cx| app.apply_layout_delta(&delta, window, cx))
            })
            .unwrap_or(true);
    if !mirror_ok || !window_ok {
        log::info!("delta {delta:?} did not apply cleanly; re-pulling the tree");
        resync_window_from_tree(cx);
        return;
    }
    app.update(cx, |app, cx| sync_window(app, cx));
}

pub(super) fn apply_to_mirror(mirror: &mut WsMirror, delta: &LayoutDelta) -> bool {
    match delta {
        LayoutDelta::PaneFacts { .. } => true,
        LayoutDelta::ActiveTabChanged { tab } => {
            mirror.active = Some(*tab);
            true
        }
        LayoutDelta::TabCreated { at, tab } => {
            mirror.tabs.retain(|t| t.id != tab.id);
            let at = (*at).min(mirror.tabs.len());
            mirror.tabs.insert(at, tab.clone());
            true
        }
        LayoutDelta::TabClosed { tab } => {
            let before = mirror.tabs.len();
            mirror.tabs.retain(|t| t.id != *tab);
            if mirror.tabs.is_empty() {
                mirror.active = None;
            }
            mirror.tabs.len() != before
        }
        LayoutDelta::TabRenamed { tab, name } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            t.name = name.clone();
            true
        }
        LayoutDelta::TabTagsChanged { tab, tags } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            t.tags = tags.clone();
            true
        }
        LayoutDelta::TagsChanged { tags } => {
            mirror.tags = tags.clone();
            true
        }
        LayoutDelta::TabMoved { tab, to } => {
            let Some(from) = mirror.tabs.iter().position(|t| t.id == *tab) else {
                return false;
            };
            let moved = mirror.tabs.remove(from);
            mirror.tabs.insert((*to).min(mirror.tabs.len()), moved);
            true
        }
        LayoutDelta::TabRestructured { tab, .. } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == tab.id) else {
                return false;
            };
            *t = tab.clone();
            true
        }
        LayoutDelta::RatioChanged { tab, path, ratio } => {
            let Some(t) = mirror.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            match t.root.descend_mut(path) {
                Some(PaneNode::Split { ratio: r, .. }) => {
                    *r = *ratio;
                    true
                }
                _ => false,
            }
        }
    }
}

pub(crate) fn resync_window_from_tree(cx: &mut App) {
    hydrate(cx, Adopt::Replace);
}

pub(crate) fn note_instance(seen: &mut String, instance: &str) -> bool {
    if instance.is_empty() {
        return false;
    }
    let before = std::mem::replace(seen, instance.to_string());
    if !before.is_empty() && before != instance {
        log::info!(
            "the anterm-server on this machine is a new process ({before} → {instance}); \
             its panes are gone"
        );
        return true;
    }
    false
}

pub(crate) fn resync_after_local_daemon_change(cx: &mut App) {
    crate::ui::remote::local_link::LocalLink::invalidate(cx);
    resync_local_windows_from_tree(cx);
}

pub(crate) fn resync_local_windows_from_tree(cx: &mut App) {
    resync_window_from_tree(cx);
}

impl AntermApp {
    pub(crate) fn apply_layout_delta(
        &mut self,
        delta: &LayoutDelta,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let index_of = |tabs: &[crate::ui::workspace::app::Tab], id: TabId| {
            tabs.iter().position(|t| t.tree_id.get() == id)
        };
        let applied = match delta {
            LayoutDelta::PaneFacts { .. } => true,
            LayoutDelta::ActiveTabChanged { tab } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    self.activate_from_delta(index, window, cx);
                }
                true
            }
            LayoutDelta::TabCreated { tab, .. } if tab.hibernated => false,
            LayoutDelta::TabCreated { at, tab } => {
                self.insert_tab_from_tree((*at).min(self.tabs.len()), tab, window, cx)
            }
            LayoutDelta::TabClosed { tab } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    let active_id = self.tabs.get(self.active).map(|t| t.tree_id.get());
                    self.tabs.remove(index);
                    self.active = active_id
                        .and_then(|id| index_of(&self.tabs, id))
                        .unwrap_or_else(|| index.min(self.tabs.len().saturating_sub(1)));
                    self.maximized = None;
                    self.wake_active_if_asleep(window, cx);
                    self.focus_active(window, cx);
                }
                true
            }
            LayoutDelta::TabRenamed { tab, name } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    self.tabs[index].name = name.clone();
                }
                true
            }
            LayoutDelta::TabTagsChanged { tab, tags } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    *self.tabs[index].tags.borrow_mut() = tags.clone();
                }
                true
            }
            LayoutDelta::TagsChanged { tags } => {
                self.adopt_sidebar_tags(tags.clone(), cx);
                true
            }
            LayoutDelta::TabMoved { tab, to } => {
                if let Some(from) = index_of(&self.tabs, *tab) {
                    let active_id = self.tabs.get(self.active).map(|t| t.tree_id.get());
                    let moved = self.tabs.remove(from);
                    self.tabs.insert((*to).min(self.tabs.len()), moved);
                    if let Some(id) = active_id
                        && let Some(index) = index_of(&self.tabs, id)
                    {
                        self.active = index;
                    }
                }
                true
            }
            LayoutDelta::TabRestructured { tab, .. } => match index_of(&self.tabs, tab.id) {
                Some(index) if tab.hibernated || self.tabs[index].is_asleep() => false,
                Some(index) => self.rebuild_tab_from_tree(index, tab, window, cx),
                None => false,
            },
            LayoutDelta::RatioChanged { tab, path, ratio } => {
                if let Some(index) = index_of(&self.tabs, *tab) {
                    set_gui_ratio(&mut self.tabs[index].pane, path, *ratio)
                } else {
                    true
                }
            }
        };
        cx.notify();
        applied
    }

    fn activate_from_delta(
        &mut self,
        index: usize,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.active == index {
            return;
        }
        self.maximized = None;
        self.active = index;
        self.wake_active_if_asleep(window, cx);
        self.focus_active(window, cx);
    }

    fn insert_tab_from_tree(
        &mut self,
        at: usize,
        tab: &TreeTab,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        if self.tabs.iter().any(|t| t.tree_id.get() == tab.id) {
            return true;
        }
        let mut existing = HashMap::new();
        let Some(pane) = self.build_pane_from_tree(&tab.root, &mut existing, window, cx) else {
            return false;
        };
        let gui = crate::ui::workspace::app::Tab::from_tree(tab, pane);
        self.tabs.insert(at, gui);
        if self.active >= at && self.tabs.len() > 1 {
            self.active += 1;
        }
        true
    }

    fn rebuild_tab_from_tree(
        &mut self,
        index: usize,
        tab: &TreeTab,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let mut existing: HashMap<u64, PaneSlot> = HashMap::new();
        for slot in self.tabs[index].pane.leaves() {
            let id = match &slot {
                PaneSlot::Ready(view) => Some(view.read(cx).pane_id),
                PaneSlot::Connecting(pending) => pending.read(cx).spawn.restore_pane,
            };
            if let Some(id) = id {
                existing.insert(id, slot);
            }
        }
        let Some(pane) = self.build_pane_from_tree(&tab.root, &mut existing, window, cx) else {
            return false;
        };
        let gui = &mut self.tabs[index];
        gui.pane = pane;
        gui.name = tab.name.clone();
        *gui.tags.borrow_mut() = tab.tags.clone();
        self.maximized = None;
        true
    }

    fn build_pane_from_tree(
        &self,
        node: &PaneNode,
        existing: &mut HashMap<u64, PaneSlot>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Option<Pane> {
        match node {
            PaneNode::Leaf { pane } => {
                if let Some(slot) = existing.remove(pane) {
                    return Some(Pane::Leaf(slot));
                }
                match crate::ui::workspace::app::new_terminal(
                    self.font_size,
                    None,
                    Some(*pane),
                    None,
                    window,
                    cx,
                ) {
                    Ok(slot) => Some(Pane::Leaf(slot)),
                    Err(e) => {
                        log::warn!("could not attach pane {pane} from a delta: {e}");
                        None
                    }
                }
            }
            PaneNode::Split { axis, ratio, a, b } => {
                let left = self.build_pane_from_tree(a, existing, window, cx);
                let right = self.build_pane_from_tree(b, existing, window, cx);
                match (left, right) {
                    (Some(a), Some(b)) => Some(Pane::split_node(
                        match axis {
                            TreeAxis::Horizontal => gpui::Axis::Horizontal,
                            TreeAxis::Vertical => gpui::Axis::Vertical,
                        },
                        *ratio,
                        a,
                        b,
                    )),
                    (one, other) => one.or(other),
                }
            }
        }
    }
}

pub(super) fn set_gui_ratio(pane: &mut Pane, path: &[Side], ratio: f32) -> bool {
    match path.split_first() {
        None => match pane {
            Pane::Split { ratio: cell, .. } => {
                cell.set(ratio.clamp(0.05, 0.95));
                true
            }
            _ => false,
        },
        Some((side, rest)) => match pane {
            Pane::Split { a, b, .. } => match side {
                Side::A => set_gui_ratio(a, rest, ratio),
                Side::B => set_gui_ratio(b, rest, ratio),
            },
            _ => false,
        },
    }
}
