use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::tag::{SidebarTags, TagId};

use super::migration::data_dir;
use super::model::{
    Axis, CLOSED_TAB_TTL, ClosedTab, LayoutDelta, MAX_CLOSED_TABS, MAX_PANES, Machine, PaneNode,
    PaneRecord, PaneSeed, Remembered, ReopenedTab, Side, Tab, TabId,
};

pub const MACHINE_FILE: &str = "machine.json";

#[cfg(not(test))]
pub const FACT_FLUSH_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(test)]
pub const FACT_FLUSH_INTERVAL: Duration = Duration::from_secs(600);

/// How many earlier generations of the machine tree are kept beside it.
///
/// Bounded on purpose: the tree is rewritten whole on every mutation, so an
/// unbounded history would be a new file every few seconds forever. Three is
/// what the spacing below has to work with.
const BACKUP_GENERATIONS: usize = 3;

/// How far apart those generations are taken.
///
/// The point of the spacing is that "the previous write" is worthless as a
/// backup here. The tree is written whole on every mutation, and the failure
/// worth recovering from arrives as a burst — #716 emptied a workspace with
/// nineteen `TabClose`s in a row, each of them a persist. A backup taken on
/// every write would have rolled the damage through all three generations
/// before anyone looked. Spaced out, the oldest generation is a quarter of an
/// hour of history, which is longer than any burst.
///
/// Note that "previous good" cannot mean "the last document that parsed":
/// an emptied tree parses perfectly and is exactly what you want to recover
/// *from*. Age is the only signal available here, so age is what is used.
pub(super) const BACKUP_SPACING: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriberId(pub u64);

pub type Notify = Arc<dyn Fn(&LayoutDelta) + Send + Sync>;

pub struct Subscription {
    store: Arc<MachineStore>,
    id: SubscriberId,
}

impl Subscription {
    pub fn id(&self) -> SubscriberId {
        self.id
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.store.unsubscribe(self.id);
    }
}

pub type LivenessProbe = Arc<dyn Fn(u64) -> bool + Send + Sync>;

pub struct MachineStore {
    path: PathBuf,
    state: Mutex<Machine>,
    liveness: Mutex<Option<LivenessProbe>>,
    notify_order: Mutex<()>,
    subscribers: Mutex<Vec<(SubscriberId, Notify)>>,
    next_subscriber: AtomicU64,
    unwritten: AtomicBool,
    flushing: AtomicBool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Persist {
    Now,
    Soon,
}

pub(super) fn refuse(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg.into())
}

fn not_found(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, msg.into())
}

impl MachineStore {
    pub fn open(path: impl Into<PathBuf>) -> Arc<MachineStore> {
        let path = path.into();
        let machine = load_machine(&path);
        Arc::new(MachineStore {
            path,
            state: Mutex::new(machine),
            liveness: Mutex::new(None),
            notify_order: Mutex::new(()),
            subscribers: Mutex::new(Vec::new()),
            next_subscriber: AtomicU64::new(1),
            unwritten: AtomicBool::new(false),
            flushing: AtomicBool::new(false),
        })
    }

    pub fn set_liveness_probe(&self, probe: LivenessProbe) {
        *self.liveness.lock().unwrap_or_else(|e| e.into_inner()) = Some(probe);
    }

    fn seed_is_live(&self, pane: u64) -> bool {
        let probe = self
            .liveness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match probe {
            Some(probe) => probe(pane),
            None => true,
        }
    }

    pub fn shared() -> io::Result<Arc<MachineStore>> {
        Ok(MachineStore::open(default_machine_path()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn machine(&self) -> Machine {
        self.locked().clone()
    }

    pub fn pane(&self, id: u64) -> Option<PaneRecord> {
        self.locked().panes.iter().find(|p| p.id == id).cloned()
    }

    pub fn set_active_tab(&self, tab: TabId, origin: Option<SubscriberId>) -> io::Result<()> {
        self.mutate(origin, |m| {
            if !m.tabs.iter().any(|t| t.id == tab) {
                return Err(not_found(format!("no tab {tab} on this machine")));
            }
            m.active_tab = Some(tab);
            Ok(((), vec![LayoutDelta::ActiveTabChanged { tab }]))
        })
    }

    pub fn tab_create(
        &self,
        at: Option<usize>,
        pane: PaneSeed,
        id: Option<TabId>,
        origin: Option<SubscriberId>,
    ) -> io::Result<Tab> {
        let live = self.seed_is_live(pane.pane);
        self.mutate(origin, |m| {
            if let Some(id) = id
                && m.tabs.iter().any(|t| t.id == id)
            {
                return Err(refuse(format!("tab {id} already exists")));
            }
            register_pane(m, pane.clone(), live)?;
            let mut tab = Tab::leaf(pane.pane);
            if let Some(id) = id {
                tab.id = id;
            }
            let at = at.unwrap_or(m.tabs.len()).min(m.tabs.len());
            m.tabs.insert(at, tab.clone());
            m.active_tab = Some(tab.id);
            let active = tab.id;
            Ok((
                tab.clone(),
                vec![
                    LayoutDelta::TabCreated { at, tab },
                    LayoutDelta::ActiveTabChanged { tab: active },
                ],
            ))
        })
    }

    pub fn tab_close(&self, tab: TabId, origin: Option<SubscriberId>) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let index = m
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .ok_or_else(|| not_found(format!("no tab {tab} on this machine")))?;
            m.tabs.remove(index);
            let mut deltas = vec![LayoutDelta::TabClosed { tab }];
            if let Some(active) = heal_active_tab(m, index) {
                deltas.push(LayoutDelta::ActiveTabChanged { tab: active });
            }
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            Ok((orphans, deltas))
        })
    }

    /// [`Self::tab_close`] for a tab the user closed and may want back: the tab
    /// leaves the machine exactly as a close takes it, and lands at the end of
    /// [`closed`](Machine::closed) instead of being forgotten, with its panes'
    /// records kept for the reopen to read.
    ///
    /// `held` is every pane the closing client had in the tab. Ones the tree
    /// never recorded there come back as strays, for the caller to end
    /// outright. The list is trimmed to [`MAX_CLOSED_TABS`] and
    /// [`CLOSED_TAB_TTL`] in the same mutation, measured from `now` (seconds
    /// since the epoch) so the rule can be tested without a clock.
    pub fn tab_close_remembered(
        &self,
        tab: TabId,
        held: &[u64],
        now: u64,
        origin: Option<SubscriberId>,
    ) -> io::Result<Remembered> {
        self.mutate(origin, |m| {
            let index = m
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .ok_or_else(|| not_found(format!("no tab {tab} on this machine")))?;
            let mut closed = m.tabs.remove(index);
            // A tab closed asleep comes back awake: reopening is itself the
            // wake, through the same restore.
            closed.hibernated = false;
            let stopped = closed.root.pane_ids();
            m.closed.push(ClosedTab {
                tab: closed,
                closed_at: now,
            });
            let mut deltas = vec![LayoutDelta::TabClosed { tab }];
            if let Some(active) = heal_active_tab(m, index) {
                deltas.push(LayoutDelta::ActiveTabChanged { tab: active });
            }
            // The caller stops these panes as soon as this returns. The records
            // stay for the reopen, but must not go on saying the panes run:
            // `anterm wait` reads `live` to tell a shell from an exited pane, and
            // would otherwise wait on a closed one forever.
            for record in m.panes.iter_mut().filter(|p| stopped.contains(&p.id)) {
                record.live = false;
            }
            let outgrown = prune_closed(m, now);
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            let strays = held
                .iter()
                .copied()
                .filter(|p| !stopped.contains(p) && !references(m, *p))
                .collect();
            let dropped = outgrown
                .into_iter()
                .filter(|p| orphans.contains(p))
                .collect();
            Ok((
                Remembered {
                    stopped,
                    strays,
                    dropped,
                },
                deltas,
            ))
        })
    }

    /// Take a closed tab back off the machine's list: the one named, or the
    /// most recently closed. `None` when there is nothing (left) to reopen.
    ///
    /// The tab does not go back into the tree here. The client that asked
    /// rebuilds it — every pane a fresh shell opening on its predecessor's
    /// screen — and pushes it as the new tab it then is, under the same id.
    ///
    /// The records stay in the pane list until that push lands, and past it:
    /// a record is what keeps the sweeps off a pane's stored screen, and the
    /// screen is read when the successor spawns, which for a remote pane can
    /// be a round trip after this answers. The next close collects them.
    ///
    /// Also trims the list, so an entry that has aged out is never handed back.
    /// The second half of the answer is the panes that trim dropped.
    pub fn tab_reopen(
        &self,
        tab: Option<TabId>,
        now: u64,
        origin: Option<SubscriberId>,
    ) -> io::Result<(Option<ReopenedTab>, Vec<u64>)> {
        self.mutate(origin, |m| {
            let outgrown = prune_closed(m, now);
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            let dropped: Vec<u64> = outgrown
                .into_iter()
                .filter(|p| orphans.contains(p))
                .collect();
            let at = match tab {
                Some(id) => m.closed.iter().rposition(|c| c.tab.id == id),
                None => m.closed.len().checked_sub(1),
            };
            let Some(at) = at else {
                return Ok(((None, dropped), Vec::new()));
            };
            let entry = m.closed.remove(at);
            let ids = entry.tab.root.pane_ids();
            let panes = m
                .panes
                .iter()
                .filter(|p| ids.contains(&p.id))
                .cloned()
                .collect();
            let reopened = ReopenedTab {
                tab: entry.tab,
                closed_at: entry.closed_at,
                panes,
            };
            Ok(((Some(reopened), dropped), Vec::new()))
        })
    }

    /// Drop the closed tabs that have aged past [`CLOSED_TAB_TTL`], with their
    /// records. Answers the panes dropped, whose stored screens the caller
    /// forgets. Run by the daemon on a timer, so an entry expires on time
    /// whether or not anyone closes or reopens a tab.
    pub fn expire_closed_tabs(&self, now: u64) -> Vec<u64> {
        let expired = self.mutate(None, |m| {
            let outgrown = prune_closed(m, now);
            if outgrown.is_empty() {
                return Ok((Vec::new(), Vec::new()));
            }
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            let dropped = outgrown
                .into_iter()
                .filter(|p| orphans.contains(p))
                .collect();
            Ok((dropped, Vec::new()))
        });
        expired.unwrap_or_else(|e| {
            log::warn!("could not expire closed tabs: {e}");
            Vec::new()
        })
    }

    pub fn tab_rename(
        &self,
        tab: TabId,
        name: Option<String>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let t = find_tab(m, tab)?;
            t.name = name.clone();
            Ok(((), vec![LayoutDelta::TabRenamed { tab, name }]))
        })
    }

    pub fn tab_move(
        &self,
        tab: TabId,
        to: usize,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            let from = m
                .tabs
                .iter()
                .position(|t| t.id == tab)
                .ok_or_else(|| not_found(format!("no tab {tab} on this machine")))?;
            let moved = m.tabs.remove(from);
            let to = to.min(m.tabs.len());
            m.tabs.insert(to, moved);
            Ok(((), vec![LayoutDelta::TabMoved { tab, to }]))
        })
    }

    /// Put a tab to sleep, or mark it awake again. Answers the panes the tab
    /// holds, which for a tab going to sleep are the ones whoever asked is now
    /// expected to stop — see [`ControlRequest::TabSetHibernated`].
    ///
    /// Only the mark changes here. The panes stay in the tab and in the pane
    /// list, so the facts a wake needs survive the processes: waking hands the
    /// same ids to the ordinary restore, which spawns successors and swaps
    /// them in with `pane_replace`. Setting the mark a tab already has is not
    /// a change and raises no delta.
    ///
    /// [`ControlRequest::TabSetHibernated`]: crate::daemon::control::ControlRequest::TabSetHibernated
    pub fn tab_set_hibernated(
        &self,
        tab: TabId,
        hibernated: bool,
        origin: Option<SubscriberId>,
    ) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let t = find_tab(m, tab)?;
            let panes = t.root.pane_ids();
            if t.hibernated == hibernated {
                return Ok((panes, Vec::new()));
            }
            t.hibernated = hibernated;
            let delta = LayoutDelta::TabRestructured {
                tab: t.clone(),
                pane: None,
            };
            Ok((panes, vec![delta]))
        })
    }

    pub fn tab_set_tags(
        &self,
        tab: TabId,
        mut tags: Vec<TagId>,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        self.mutate(origin, |m| {
            m.tags.retain_valid(&mut tags);
            let t = find_tab(m, tab)?;
            if t.tags == tags {
                return Ok(((), Vec::new()));
            }
            t.tags = tags.clone();
            Ok(((), vec![LayoutDelta::TabTagsChanged { tab, tags }]))
        })
    }

    /// Replaces the machine's sidebar tag catalog whole.
    ///
    /// Any tag ids dropped from the catalog are pruned from every tab in the
    /// same mutation, so no tab keeps pointing at a tag that no longer exists.
    /// Each affected tab is announced with [`LayoutDelta::TabTagsChanged`],
    /// followed by [`LayoutDelta::TagsChanged`] when the catalog itself changed.
    pub fn set_tags(&self, tags: SidebarTags, origin: Option<SubscriberId>) -> io::Result<()> {
        self.mutate(origin, |m| {
            let mut deltas = Vec::new();
            for t in &mut m.tabs {
                if tags.retain_valid(&mut t.tags) {
                    deltas.push(LayoutDelta::TabTagsChanged {
                        tab: t.id,
                        tags: t.tags.clone(),
                    });
                }
            }
            for c in &mut m.closed {
                tags.retain_valid(&mut c.tab.tags);
            }
            if m.tags != tags {
                m.tags = tags.clone();
                deltas.push(LayoutDelta::TagsChanged { tags });
            }
            Ok(((), deltas))
        })
    }

    pub fn pane_split(
        &self,
        pane: u64,
        axis: Axis,
        ratio: f32,
        new: PaneSeed,
        first: bool,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        let ratio = clamp_ratio(ratio)?;
        let live = self.seed_is_live(new.pane);
        self.mutate(origin, |m| {
            register_pane(m, new.clone(), live)?;
            let record = m
                .panes
                .iter()
                .find(|p| p.id == new.pane)
                .cloned()
                .expect("registered above");
            let tab = m
                .tabs
                .iter_mut()
                .find(|t| t.root.contains(pane))
                .ok_or_else(|| not_found(format!("no pane {pane} to split on this machine")))?;
            tab.root.split_leaf(pane, new.pane, axis, ratio, first);
            let delta = LayoutDelta::TabRestructured {
                tab: tab.clone(),
                pane: Some(record),
            };
            Ok(((), vec![delta]))
        })
    }

    pub fn pane_close(&self, pane: u64, origin: Option<SubscriberId>) -> io::Result<Vec<u64>> {
        self.mutate(origin, |m| {
            let index = m
                .tabs
                .iter()
                .position(|t| t.root.contains(pane))
                .ok_or_else(|| not_found(format!("no pane {pane} on this machine")))?;
            let mut deltas = Vec::new();
            match m.tabs[index].root.remove_leaf(pane) {
                None => {
                    let closed = m.tabs.remove(index);
                    deltas.push(LayoutDelta::TabClosed { tab: closed.id });
                    if let Some(active) = heal_active_tab(m, index) {
                        deltas.push(LayoutDelta::ActiveTabChanged { tab: active });
                    }
                }
                Some(true) => deltas.push(LayoutDelta::TabRestructured {
                    tab: m.tabs[index].clone(),
                    pane: None,
                }),
                Some(false) => unreachable!("the tab was chosen because it contains the pane"),
            };
            let orphans = collect_orphan_panes(m);
            m.panes.retain(|p| !orphans.contains(&p.id));
            Ok((orphans, deltas))
        })
    }

    pub fn pane_set_ratio(
        &self,
        tab: TabId,
        path: Vec<Side>,
        ratio: f32,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        let ratio = clamp_ratio(ratio)?;
        self.mutate(origin, |m| {
            let t = find_tab(m, tab)?;
            match t.root.descend_mut(&path) {
                Some(PaneNode::Split { ratio: r, .. }) => *r = ratio,
                _ => {
                    return Err(refuse(format!(
                        "tab {tab} has no split at that path any more"
                    )));
                }
            }
            Ok(((), vec![LayoutDelta::RatioChanged { tab, path, ratio }]))
        })
    }

    pub fn pane_move(
        &self,
        pane: u64,
        to: u64,
        axis: Axis,
        first: bool,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        if pane == to {
            return Err(refuse("a pane cannot be moved next to itself"));
        }
        self.mutate(origin, |m| {
            let from = m
                .tabs
                .iter()
                .position(|t| t.root.contains(pane))
                .ok_or_else(|| not_found(format!("no pane {pane} on this machine")))?;
            let dest = m
                .tabs
                .iter()
                .position(|t| t.root.contains(to))
                .ok_or_else(|| not_found(format!("no pane {to} on this machine")))?;

            let mut deltas: Vec<LayoutDelta> = Vec::new();
            match m.tabs[from].root.remove_leaf(pane) {
                None => {
                    if from == dest {
                        return Err(refuse("a pane cannot be moved next to itself".to_string()));
                    }
                    let closed = m.tabs.remove(from);
                    deltas.push(LayoutDelta::TabClosed { tab: closed.id });
                    if let Some(active) = heal_active_tab(m, from) {
                        deltas.push(LayoutDelta::ActiveTabChanged { tab: active });
                    }
                }
                Some(true) => {
                    deltas.push(LayoutDelta::TabRestructured {
                        tab: m.tabs[from].clone(),
                        pane: None,
                    });
                }
                Some(false) => unreachable!("the tab was chosen because it contains the pane"),
            }
            let dest_tab = m
                .tabs
                .iter_mut()
                .find(|t| t.root.contains(to))
                .expect("the destination tab still exists; only the source tab can close");
            dest_tab.root.split_leaf(to, pane, axis, 0.5, first);
            deltas.push(LayoutDelta::TabRestructured {
                tab: dest_tab.clone(),
                pane: None,
            });
            Ok(((), deltas))
        })
    }

    pub fn pane_replace(
        &self,
        old: u64,
        new: PaneSeed,
        origin: Option<SubscriberId>,
    ) -> io::Result<()> {
        let live = self.seed_is_live(new.pane);
        self.mutate(origin, |m| {
            register_pane(m, new.clone(), live)?;
            let record = m
                .panes
                .iter()
                .find(|p| p.id == new.pane)
                .cloned()
                .expect("registered above");
            let tab = m
                .tabs
                .iter_mut()
                .find(|t| t.root.contains(old))
                .ok_or_else(|| not_found(format!("no pane {old} on this machine")))?;
            tab.root.replace_leaf(old, new.pane);
            let delta = LayoutDelta::TabRestructured {
                tab: tab.clone(),
                pane: Some(record),
            };
            m.panes.retain(|p| p.id != old);
            Ok(((), vec![delta]))
        })
    }

    pub fn note_pane_facts(self: &Arc<Self>, pane: u64, update: impl FnOnce(&mut PaneRecord)) {
        self.ensure_flusher();
        let result: io::Result<()> = self.mutate_with(None, Persist::Soon, |m| {
            let Some(record) = m.panes.iter_mut().find(|p| p.id == pane) else {
                return Ok(((), Vec::new()));
            };
            let before = record.clone();
            update(record);
            record.id = before.id;
            if *record == before {
                return Ok(((), Vec::new()));
            }
            let record = record.clone();
            let deltas = if m.tabs.iter().any(|t| t.root.contains(pane)) {
                vec![LayoutDelta::PaneFacts { pane: record }]
            } else {
                Vec::new()
            };
            Ok(((), deltas))
        });
        if let Err(e) = result {
            log::warn!("could not record facts about pane {pane}: {e}");
        }
    }

    pub fn subscribe(self: &Arc<Self>, f: Notify) -> Subscription {
        let id = SubscriberId(self.next_subscriber.fetch_add(1, Ordering::Relaxed));
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, f));
        Subscription {
            store: Arc::clone(self),
            id,
        }
    }

    fn unsubscribe(&self, id: SubscriberId) {
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(sid, _)| *sid != id);
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Machine> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn mutate<T>(
        &self,
        origin: Option<SubscriberId>,
        op: impl FnOnce(&mut Machine) -> io::Result<(T, Vec<LayoutDelta>)>,
    ) -> io::Result<T> {
        self.mutate_with(origin, Persist::Now, op)
    }

    fn mutate_with<T>(
        &self,
        origin: Option<SubscriberId>,
        persist: Persist,
        op: impl FnOnce(&mut Machine) -> io::Result<(T, Vec<LayoutDelta>)>,
    ) -> io::Result<T> {
        let _order = self.notify_order.lock().unwrap_or_else(|e| e.into_inner());
        let deltas;
        let value;
        {
            let mut m = self.locked();
            let before = m.clone();
            match op(&mut m).and_then(|out| {
                if *m != before {
                    match persist {
                        Persist::Now => self.persist(&m)?,
                        Persist::Soon => self.unwritten.store(true, Ordering::Release),
                    }
                }
                Ok(out)
            }) {
                Ok((v, d)) => {
                    value = v;
                    deltas = d;
                }
                Err(e) => {
                    *m = before;
                    return Err(e);
                }
            }
        }
        if !deltas.is_empty() {
            self.notify_all(&deltas, origin);
        }
        Ok(value)
    }

    pub fn flush(&self) {
        if !self.unwritten.load(Ordering::Acquire) {
            return;
        }
        let _order = self.notify_order.lock().unwrap_or_else(|e| e.into_inner());
        let m = self.locked();
        self.unwritten.store(false, Ordering::Release);
        if let Err(e) = self.persist(&m) {
            log::warn!("could not write {}: {e}", self.path.display());
            self.unwritten.store(true, Ordering::Release);
        }
    }

    fn ensure_flusher(self: &Arc<Self>) {
        if self.flushing.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("anterm-machine-flush".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(FACT_FLUSH_INTERVAL);
                    let Some(store) = weak.upgrade() else { return };
                    store.flush();
                }
            });
        if let Err(e) = spawned {
            log::warn!("could not start the machine-tree flusher ({e}); writing facts inline");
            self.flushing.store(false, Ordering::Release);
            self.flush();
        }
    }

    fn persist(&self, m: &Machine) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(m).map_err(io::Error::other)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Before the overwrite, never after: the backup is the document about
        // to be replaced. A failure here is not a reason to stop writing the
        // new one — a machine with no backup still has to keep working.
        if let Err(e) = keep_a_generation(&self.path, BACKUP_SPACING) {
            log::warn!(
                "could not keep an earlier generation of {}: {e}",
                self.path.display()
            );
        }
        crate::config::write_atomic_private(&self.path, &bytes)
    }

    fn notify_all(&self, deltas: &[LayoutDelta], origin: Option<SubscriberId>) {
        let subscribers: Vec<(SubscriberId, Notify)> = self
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for delta in deltas {
            for (sid, f) in &subscribers {
                if Some(*sid) != origin {
                    f(delta);
                }
            }
        }
    }
}

fn find_tab(m: &mut Machine, tab: TabId) -> io::Result<&mut Tab> {
    m.tabs
        .iter_mut()
        .find(|t| t.id == tab)
        .ok_or_else(|| not_found(format!("no tab {tab} on this machine")))
}

fn heal_active_tab(m: &mut Machine, removed: usize) -> Option<TabId> {
    let named = m
        .active_tab
        .is_some_and(|active| m.tabs.iter().any(|t| t.id == active));
    if named || m.tabs.is_empty() {
        m.active_tab = m.active_tab.filter(|_| named);
        return None;
    }
    let active = m.tabs[removed.min(m.tabs.len() - 1)].id;
    m.active_tab = Some(active);
    Some(active)
}

/// A pane registers once: a second tab, a second split, a second seed for the
/// same id is refused. The exception is a record nothing names any more — left
/// by a reopened tab ([`MachineStore::tab_reopen`]) — which the pane it was
/// kept for claims back, taking over what the tree still knew about it.
fn register_pane(m: &mut Machine, seed: PaneSeed, live: bool) -> io::Result<()> {
    if references(m, seed.pane) {
        return Err(refuse(format!(
            "pane {} is already part of this machine's tree",
            seed.pane
        )));
    }
    if let Some(record) = m.panes.iter_mut().find(|p| p.id == seed.pane) {
        let kept = std::mem::replace(record, seed.into_record(live));
        record.title = kept.title;
        record.osc_title = kept.osc_title;
        if record.cwd.is_none() {
            record.cwd = kept.cwd;
        }
        if record.agent.is_none() {
            record.agent = kept.agent;
        }
        if record.shell.is_none() {
            record.shell = kept.shell;
        }
        return Ok(());
    }
    if m.panes.len() >= MAX_PANES {
        return Err(refuse(format!(
            "this machine's tree already references {MAX_PANES} panes"
        )));
    }
    m.panes.push(seed.into_record(live));
    Ok(())
}

/// Whether a tab names `pane` — an open one, or one kept to reopen. A closed
/// tab's panes are still the tree's: their records and stored screens are what
/// the reopen is made of.
fn references(m: &Machine, pane: u64) -> bool {
    m.tabs.iter().any(|t| t.root.contains(pane))
        || m.closed.iter().any(|c| c.tab.root.contains(pane))
}

fn collect_orphan_panes(m: &Machine) -> Vec<u64> {
    m.panes
        .iter()
        .map(|p| p.id)
        .filter(|id| !references(m, *id))
        .collect()
}

/// Trim the machine's recently-closed list to what it may keep at `now`:
/// nothing older than [`CLOSED_TAB_TTL`], and the newest [`MAX_CLOSED_TABS`]
/// of the rest. Answers the panes of the entries dropped.
///
/// An entry stamped in the future — a clock set back since it closed — counts
/// as just closed rather than as expired: losing it to a clock change would be
/// a surprise, keeping it a little longer is not.
fn prune_closed(m: &mut Machine, now: u64) -> Vec<u64> {
    let ttl = CLOSED_TAB_TTL.as_secs();
    let mut dropped = Vec::new();
    m.closed.retain(|c| {
        let keep = now.saturating_sub(c.closed_at) < ttl;
        if !keep {
            dropped.extend(c.tab.root.pane_ids());
        }
        keep
    });
    let excess = m.closed.len().saturating_sub(MAX_CLOSED_TABS);
    for c in m.closed.drain(..excess) {
        dropped.extend(c.tab.root.pane_ids());
    }
    dropped
}

fn clamp_ratio(ratio: f32) -> io::Result<f32> {
    if !ratio.is_finite() {
        return Err(refuse("a split ratio must be a finite number"));
    }
    Ok(ratio.clamp(0.05, 0.95))
}

fn load_machine(path: &Path) -> Machine {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Machine::default(),
        Err(e) => {
            log::warn!("could not read {}; quarantining it: {e}", path.display());
            crate::config::quarantine_by_rename(path);
            return load_a_backup(path).unwrap_or_default();
        }
    };
    match parse_machine(&text) {
        Ok(machine) => machine,
        Err(e) => {
            log::warn!("{} does not parse ({e}); quarantining it", path.display());
            crate::config::quarantine(path);
            load_a_backup(path).unwrap_or_default()
        }
    }
}

#[derive(serde::Deserialize)]
struct LegacyWorkspace {
    #[serde(default)]
    last_active: u64,
    #[serde(default)]
    tabs: Vec<Tab>,
    #[serde(default)]
    active_tab: Option<TabId>,
    #[serde(default)]
    closed: Vec<ClosedTab>,
}

#[derive(serde::Deserialize)]
struct StoredMachine {
    #[serde(default)]
    tabs: Vec<Tab>,
    #[serde(default)]
    active_tab: Option<TabId>,
    #[serde(default)]
    tags: SidebarTags,
    #[serde(default)]
    closed: Vec<ClosedTab>,
    #[serde(default)]
    panes: Vec<PaneRecord>,
    #[serde(default)]
    workspaces: Vec<LegacyWorkspace>,
}

pub(super) fn parse_machine(text: &str) -> serde_json::Result<Machine> {
    let stored = serde_json::from_str::<StoredMachine>(crate::config::strip_bom(text))?;
    let mut machine = if stored.tabs.is_empty() && !stored.workspaces.is_empty() {
        let mut tabs = Vec::new();
        let mut closed = Vec::new();
        let mut best_active: Option<(u64, TabId)> = None;
        for ws in stored.workspaces {
            if let Some(at) = ws.active_tab
                && ws.tabs.iter().any(|t| t.id == at)
                && best_active.is_none_or(|(ts, _)| ws.last_active >= ts)
            {
                best_active = Some((ws.last_active, at));
            }
            tabs.extend(ws.tabs);
            closed.extend(ws.closed);
        }
        closed.sort_by_key(|c| c.closed_at);
        let active_tab = best_active
            .map(|(_, id)| id)
            .or_else(|| tabs.first().map(|t| t.id));
        Machine {
            tabs,
            active_tab,
            tags: stored.tags,
            closed,
            panes: stored.panes,
        }
    } else {
        Machine {
            tabs: stored.tabs,
            active_tab: stored.active_tab,
            tags: stored.tags,
            closed: stored.closed,
            panes: stored.panes,
        }
    };
    for pane in &mut machine.panes {
        pane.live = false;
    }
    Ok(machine)
}

/// The newest kept generation that still parses.
///
/// Only reached when the live document is gone or unreadable — a tree that
/// parses is always preferred to a backup, however empty it turns out to be.
/// This is the automatic half of [`keep_a_generation`]; the manual half is a
/// human copying a `.bak` over `machine.json`, which is why the files are
/// plain JSON under obvious names.
fn load_a_backup(path: &Path) -> Option<Machine> {
    (0..BACKUP_GENERATIONS).find_map(|generation| {
        let kept = backup_path(path, generation);
        let machine = parse_machine(&std::fs::read_to_string(&kept).ok()?).ok()?;
        log::warn!("recovered the machine tree from {}", kept.display());
        Some(machine)
    })
}

/// Where generation `n` of `path` is kept — `machine.json.bak` for the newest,
/// `machine.json.bak.1` and `.bak.2` behind it.
pub(super) fn backup_path(path: &Path, generation: usize) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(MACHINE_FILE);
    match generation {
        0 => path.with_file_name(format!("{name}.bak")),
        n => path.with_file_name(format!("{name}.bak.{n}")),
    }
}

/// Rotates the document currently at `path` into the backup ring, if the ring's
/// newest entry is at least `spacing` old.
///
/// The live file is never renamed, only read: at every point in this function
/// `path` still holds a complete document, so a crash part-way through costs at
/// most one *backup* generation and never the tree itself. The new generation
/// lands through `write_atomic_private`, which writes a sibling temporary and
/// renames it into place — so a reader never sees a half-written `.bak` either,
/// and the 0600 the live tree is written under is the mode the copies get.
///
/// Returns `Ok(())` when there was nothing to do: no tree yet, or the newest
/// generation is younger than `spacing`.
pub(super) fn keep_a_generation(path: &Path, spacing: Duration) -> io::Result<()> {
    // Asked before the tree is read: this runs on every persist and answers
    // "nothing to do" on almost all of them, so reading the whole document
    // first would be a full read per write for one copy every five minutes.
    let newest = backup_path(path, 0);
    let too_soon = std::fs::metadata(&newest)
        .and_then(|meta| meta.modified())
        .map(|at| at.elapsed().unwrap_or_default() < spacing)
        .unwrap_or(false);
    if too_soon {
        return Ok(());
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        // Nothing has been written yet, so there is no previous generation.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    // Oldest first, so nothing is overwritten before it has been moved down.
    // A generation that is not there yet simply has nothing to move.
    for generation in (1..BACKUP_GENERATIONS).rev() {
        let from = backup_path(path, generation - 1);
        match std::fs::rename(&from, backup_path(path, generation)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    crate::config::write_atomic_private(&newest, &bytes)
}

static OBSERVED: Mutex<Option<Arc<MachineStore>>> = Mutex::new(None);

pub fn publish_observations(store: &Arc<MachineStore>) {
    *OBSERVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(store));
}

pub fn observe_pane(pane: u64, f: impl FnOnce(&mut PaneRecord)) {
    let store = OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(store) = store {
        store.note_pane_facts(pane, f);
    }
}

pub fn observed_store() -> Option<Arc<MachineStore>> {
    OBSERVED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
pub(crate) fn withdraw_observations() {
    *OBSERVED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
pub(crate) static OBSERVE_SLOT: Mutex<()> = Mutex::new(());

pub fn default_machine_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join(MACHINE_FILE))
}
