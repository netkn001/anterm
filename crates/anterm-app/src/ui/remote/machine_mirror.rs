use gpui::{App, Global};
use anterm_core::daemon::control::{ControlRequest, ReplyOk};
use anterm_core::machine::{LayoutDelta, Machine, PaneRecord, Tab, TabId};
use anterm_core::tab_view::same_title_ignoring_status_mark;

pub use anterm_core::tab_view::{TabLabel, TabView};

/// A write a window made to its own mirror, kept for as long as a pull that
/// was already on its way when it happened has still to land.
type MirrorWrite = Box<dyn Fn(&mut Machine)>;

#[derive(Default)]
pub struct MachineMirrors {
    machine: Option<Machine>,
    pulling: bool,
    /// What this client wrote into the mirror while a pull was in flight.
    /// Replayed on top of the tree that lands, in the order they were made.
    since_pull: Option<Vec<MirrorWrite>>,
}

impl Global for MachineMirrors {}

impl MachineMirrors {
    pub fn machine(cx: &App) -> Option<&Machine> {
        cx.try_global::<Self>()?.machine.as_ref()
    }

    pub fn refresh(cx: &mut App) {
        let client = match crate::ui::remote::tree_sync::tree_control(cx) {
            crate::ui::remote::tree_sync::TreeLink::Ready(client) => client,
            crate::ui::remote::tree_sync::TreeLink::Unserved => {
                log::debug!("not pulling local daemon: server does not serve the machine tree");
                return;
            }
            crate::ui::remote::tree_sync::TreeLink::Down => return,
        };
        if !Self::start_pull(cx) {
            return;
        }
        cx.spawn(async move |cx| {
            let pulled = cx
                .background_executor()
                .spawn(async move {
                    match client.call(ControlRequest::MachineGet) {
                        Ok(ReplyOk::MachineTree(machine)) => Some(machine),
                        Ok(other) => {
                            log::warn!("MachineGet answered {other:?}");
                            None
                        }
                        Err(e) => {
                            log::debug!("could not pull the machine tree: {e}");
                            None
                        }
                    }
                })
                .await;
            cx.update(|cx| Self::finish_pull(cx, pulled.map(|machine| *machine)));
        })
        .detach();
    }

    fn start_pull(cx: &mut App) -> bool {
        let mirrors = cx.default_global::<Self>();
        if mirrors.pulling {
            return false;
        }
        mirrors.pulling = true;
        mirrors.since_pull.get_or_insert_with(Vec::new);
        true
    }

    fn finish_pull(cx: &mut App, pulled: Option<Machine>) {
        let mirrors = cx.default_global::<Self>();
        mirrors.pulling = false;
        let since = mirrors.since_pull.take().unwrap_or_default();
        let Some(mut machine) = pulled else {
            return;
        };
        for write in since {
            write(&mut machine);
        }
        mirrors.machine = Some(machine);
        cx.refresh_windows();
    }

    fn write(cx: &mut App, edit: impl Fn(&mut Machine) + 'static) {
        let mirrors = cx.default_global::<Self>();
        if let Some(machine) = mirrors.machine.as_mut() {
            edit(machine);
        }
        if let Some(since) = mirrors.since_pull.as_mut() {
            since.push(Box::new(edit));
        }
    }

    pub fn install(cx: &mut App, machine: Machine) {
        cx.default_global::<Self>().machine = Some(machine);
        cx.refresh_windows();
    }

    /// Whether `delta` only says again what a pane's tab already shows.
    pub fn delta_changes_nothing_shown(cx: &App, delta: &LayoutDelta) -> bool {
        let LayoutDelta::PaneFacts { pane } = delta else {
            return false;
        };
        Self::machine(cx)
            .and_then(|machine| machine.panes.iter().find(|p| p.id == pane.id))
            .is_some_and(|old| shows_the_same(old, pane))
    }

    pub fn apply_delta(cx: &mut App, delta: &LayoutDelta) {
        let applied = match cx.default_global::<Self>().machine.as_mut() {
            Some(machine) => apply(machine, delta),
            None => true,
        };
        if !applied {
            log::debug!("machine mirror fell behind; re-pulling");
            Self::refresh(cx);
        }
    }

    pub fn note_synced_tabs(cx: &mut App, tabs: Vec<Tab>, active: Option<TabId>) {
        Self::write(cx, move |machine| {
            machine.tabs = tabs.clone();
            machine.active_tab = active;
        });
    }

    pub fn note_seeded_panes(cx: &mut App, records: Vec<PaneRecord>) {
        Self::write(cx, move |machine| {
            for record in &records {
                if !machine.panes.iter().any(|p| p.id == record.id) {
                    machine.panes.push(record.clone());
                }
            }
        });
    }

    pub fn note_remembered_closes(cx: &mut App, closed: Vec<TabId>) {
        let now = crate::ui::workspace::home::now_secs();
        Self::write(cx, move |machine| {
            for id in &closed {
                let Some(at) = machine.tabs.iter().position(|t| t.id == *id) else {
                    continue;
                };
                let tab = machine.tabs.remove(at);
                machine.closed.push(anterm_core::machine::ClosedTab {
                    tab,
                    closed_at: now,
                });
            }
            let excess = machine
                .closed
                .len()
                .saturating_sub(anterm_core::machine::MAX_CLOSED_TABS);
            machine.closed.drain(..excess);
        });
    }

    pub fn note_reopened(cx: &mut App, tab: TabId) {
        Self::write(cx, move |machine| {
            machine.closed.retain(|c| c.tab.id != tab);
        });
        cx.refresh_windows();
    }
}

/// Does the local machine tree currently hold any tabs?
pub fn machine_holds_tabs(cx: &App) -> bool {
    MachineMirrors::machine(cx).is_some_and(|m| !m.tabs.is_empty())
}

fn shows_the_same(old: &PaneRecord, new: &PaneRecord) -> bool {
    let titles = match (&old.osc_title, &new.osc_title) {
        (Some(a), Some(b)) => same_title_ignoring_status_mark(a, b),
        (None, None) => true,
        _ => false,
    };
    titles
        && PaneRecord {
            osc_title: new.osc_title.clone(),
            ..old.clone()
        } == *new
}

fn apply(machine: &mut Machine, delta: &LayoutDelta) -> bool {
    match delta {
        LayoutDelta::PaneFacts { pane } => {
            match machine.panes.iter_mut().find(|p| p.id == pane.id) {
                Some(record) => *record = pane.clone(),
                None => machine.panes.push(pane.clone()),
            }
            true
        }
        LayoutDelta::ActiveTabChanged { tab } => {
            machine.active_tab = Some(*tab);
            true
        }
        LayoutDelta::TabCreated { at, tab } => {
            machine.tabs.retain(|t| t.id != tab.id);
            let at = (*at).min(machine.tabs.len());
            machine.tabs.insert(at, tab.clone());
            true
        }
        LayoutDelta::TabClosed { tab } => {
            let before = machine.tabs.len();
            machine.tabs.retain(|t| t.id != *tab);
            if machine.tabs.is_empty() {
                machine.active_tab = None;
            }
            machine.tabs.len() != before
        }
        LayoutDelta::TabRenamed { tab, name } => {
            let Some(t) = machine.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            t.name = name.clone();
            true
        }
        LayoutDelta::TabTagsChanged { tab, tags } => {
            let Some(t) = machine.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            t.tags = tags.clone();
            true
        }
        LayoutDelta::TagsChanged { tags } => {
            machine.tags = tags.clone();
            true
        }
        LayoutDelta::TabMoved { tab, to } => {
            let Some(from) = machine.tabs.iter().position(|t| t.id == *tab) else {
                return false;
            };
            let moved = machine.tabs.remove(from);
            machine.tabs.insert((*to).min(machine.tabs.len()), moved);
            true
        }
        LayoutDelta::TabRestructured { tab, pane } => {
            let Some(t) = machine.tabs.iter_mut().find(|t| t.id == tab.id) else {
                return false;
            };
            *t = tab.clone();
            if let Some(pane) = pane {
                match machine.panes.iter_mut().find(|p| p.id == pane.id) {
                    Some(record) => *record = pane.clone(),
                    None => machine.panes.push(pane.clone()),
                }
            }
            true
        }
        LayoutDelta::RatioChanged { tab, path, ratio } => {
            let Some(t) = machine.tabs.iter_mut().find(|t| t.id == *tab) else {
                return false;
            };
            match t.root.descend_mut(path) {
                Some(anterm_core::machine::PaneNode::Split { ratio: r, .. }) => {
                    *r = *ratio;
                    true
                }
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use anterm_core::machine::{Axis, PaneNode, PaneSeed, Tab, TabId};

    use super::*;

    fn leaf_tab(pane: u64) -> Tab {
        Tab::leaf(pane)
    }

    #[gpui::test]
    fn a_remembered_close_moves_onto_the_mirrors_list_and_a_reopen_takes_it_off(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let (kept, closed) = (leaf_tab(1), leaf_tab(2));
            MachineMirrors::install(cx, Machine::default());
            MachineMirrors::note_synced_tabs(cx, vec![kept.clone(), closed.clone()], None);

            for _ in 0..2 {
                MachineMirrors::note_remembered_closes(cx, vec![closed.id]);
            }

            let machine = |cx: &App| MachineMirrors::machine(cx).unwrap().clone();
            assert_eq!(machine(cx).tabs, vec![kept.clone()]);
            let listed: Vec<TabId> = machine(cx).closed.iter().map(|c| c.tab.id).collect();
            assert_eq!(listed, vec![closed.id]);

            MachineMirrors::note_reopened(cx, closed.id);
            assert!(machine(cx).closed.is_empty());
        });
    }

    #[gpui::test]
    fn a_seed_fills_only_the_records_the_mirror_lacks(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let facts = PaneRecord {
                cwd: Some("/work/deeper".into()),
                osc_title: Some("me@host:~/work/deeper".into()),
                live: true,
                ..PaneRecord::new(1)
            };
            MachineMirrors::install(
                cx,
                Machine {
                    panes: vec![facts.clone()],
                    ..Machine::default()
                },
            );
            let known = PaneSeed {
                cwd: Some("/work".into()),
                ..PaneSeed::bare(1)
            }
            .into_record(false);
            let fresh = PaneSeed {
                cwd: Some("/work/api".into()),
                ..PaneSeed::bare(2)
            }
            .into_record(true);
            MachineMirrors::note_seeded_panes(cx, vec![known, fresh]);
            let machine = MachineMirrors::machine(cx).expect("installed");
            assert_eq!(machine.panes.len(), 2);
            assert_eq!(
                machine.panes[0], facts,
                "what the machine said outranks the seed"
            );
            assert_eq!(machine.panes[1].cwd.as_deref(), Some("/work/api"));
            assert!(machine.panes[1].live);
        });
    }

    #[gpui::test]
    fn a_pull_already_on_its_way_lands_under_what_the_window_pushed(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(MachineMirrors::start_pull(cx));

            let pushed = leaf_tab(5);
            MachineMirrors::note_synced_tabs(cx, vec![pushed.clone()], None);
            let seeded = |pane, cwd: &str| {
                PaneSeed {
                    cwd: Some(cwd.into()),
                    ..PaneSeed::bare(pane)
                }
                .into_record(true)
            };
            MachineMirrors::note_seeded_panes(
                cx,
                vec![seeded(5, "/work/verify-main"), seeded(6, "/work")],
            );

            MachineMirrors::finish_pull(
                cx,
                Some(Machine {
                    tabs: vec![leaf_tab(4)],
                    panes: vec![PaneRecord {
                        cwd: Some("/work/deeper".into()),
                        osc_title: Some("me@host:~/work/deeper".into()),
                        live: true,
                        ..PaneRecord::new(6)
                    }],
                    ..Machine::default()
                }),
            );

            let machine = MachineMirrors::machine(cx).expect("the pull installed");
            assert_eq!(
                machine.tabs,
                vec![pushed],
                "the tabs the window pushed are not rolled back by a tree older than them"
            );
            let cwd_of = |pane: u64| {
                machine
                    .panes
                    .iter()
                    .find(|p| p.id == pane)
                    .and_then(|p| p.cwd.clone())
            };
            assert_eq!(
                cwd_of(5).as_deref(),
                Some("/work/verify-main"),
                "and the record for the pane it seeded is still the only one there is"
            );
            assert_eq!(
                cwd_of(6).as_deref(),
                Some("/work/deeper"),
                "while a pane the machine did speak about keeps what it said"
            );

            assert!(MachineMirrors::start_pull(cx));
            MachineMirrors::finish_pull(cx, Some(Machine::default()));
            let machine = MachineMirrors::machine(cx).expect("installed");
            assert!(
                machine.tabs.is_empty() && machine.panes.is_empty(),
                "a tree that has dropped them says so: {machine:?}"
            );
        });
    }

    #[test]
    fn structural_deltas_advance_the_mirrored_tree() {
        let mut machine = Machine::default();
        let tab = leaf_tab(1);
        let tab_id = tab.id;
        assert!(apply(&mut machine, &LayoutDelta::TabCreated { at: 0, tab }));
        let restructured = Tab {
            id: tab_id,
            name: None,
            tags: Vec::new(),
            root: PaneNode::Split {
                axis: Axis::Vertical,
                ratio: 0.5,
                a: Box::new(PaneNode::Leaf { pane: 1 }),
                b: Box::new(PaneNode::Leaf { pane: 2 }),
            },
            hibernated: false,
        };
        assert!(apply(
            &mut machine,
            &LayoutDelta::TabRestructured {
                tab: restructured,
                pane: Some(PaneRecord::new(2)),
            },
        ));
        assert_eq!(machine.tabs[0].root.pane_ids(), vec![1, 2]);
        assert_eq!(
            machine.panes.len(),
            1,
            "the rider pane record is upserted into the registry"
        );
    }

    #[test]
    fn a_tab_created_delta_that_straddled_a_pull_lands_once() {
        let mut machine = Machine::default();
        let delta = LayoutDelta::TabCreated {
            at: 0,
            tab: leaf_tab(1),
        };
        assert!(apply(&mut machine, &delta));
        assert!(apply(&mut machine, &delta));
        assert_eq!(
            machine.tabs.len(),
            1,
            "the second application is the pull/delta overlap, not a second tab"
        );
    }

    #[test]
    fn a_delta_about_a_tab_the_mirror_never_saw_asks_for_a_repull() {
        let mut machine = Machine::default();
        assert!(
            !apply(
                &mut machine,
                &LayoutDelta::TabRenamed {
                    tab: TabId::new(),
                    name: Some("x".into()),
                },
            ),
            "an unappliable delta must say so, so the caller re-pulls"
        );
    }

    #[gpui::test]
    fn a_delta_is_only_old_news_against_the_record_the_mirror_holds(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut known = PaneRecord::new(7);
            known.osc_title = Some("\u{25D0} fixing the tab".into());
            MachineMirrors::install(
                cx,
                Machine {
                    panes: vec![known.clone()],
                    ..Machine::default()
                },
            );
            let facts = |pane: PaneRecord| LayoutDelta::PaneFacts { pane };
            let asked = |cx: &App, delta: &LayoutDelta| {
                MachineMirrors::delta_changes_nothing_shown(cx, delta)
            };

            let mut frame = known.clone();
            frame.osc_title = Some("\u{25D1} fixing the tab".into());
            assert!(asked(cx, &facts(frame)), "a turned frame");

            let mut words = known.clone();
            words.osc_title = Some("\u{25D1} fixing the parser".into());
            assert!(!asked(cx, &facts(words)), "new words");

            let stranger = PaneRecord::new(8);
            assert!(
                !asked(cx, &facts(stranger)),
                "a pane the mirror has not met"
            );
            assert!(
                !asked(
                    cx,
                    &LayoutDelta::ActiveTabChanged { tab: TabId::new() }
                ),
                "a delta that is not about a pane's facts"
            );
        });
    }

    #[test]
    fn a_spinner_frame_is_not_news_but_any_other_change_is() {
        let mut old = PaneRecord::new(7);
        old.osc_title = Some("\u{25D0} fixing the tab".into());
        let mut next = old.clone();
        assert!(shows_the_same(&old, &next), "the same record again");
        next.osc_title = Some("\u{25D1} fixing the tab".into());
        assert!(shows_the_same(&old, &next), "only the mark moved");
        next.osc_title = Some("\u{25D1} fixing the parser".into());
        assert!(!shows_the_same(&old, &next), "the words changed");
        next.osc_title = None;
        assert!(!shows_the_same(&old, &next), "the title was cleared");

        let mut live = old.clone();
        live.live = true;
        assert!(!shows_the_same(&old, &live), "a pane that came alive");
        let mut moved = old.clone();
        moved.cwd = Some("/elsewhere".into());
        assert!(!shows_the_same(&old, &moved), "a pane that moved");
    }

    #[test]
    fn pane_facts_upsert_the_registry_even_for_a_pane_born_elsewhere() {
        let mut machine = Machine::default();
        let mut record = PaneRecord::new(7);
        record.cwd = Some("/work".into());
        assert!(apply(
            &mut machine,
            &LayoutDelta::PaneFacts {
                pane: record.clone(),
            },
        ));
        record.live = true;
        assert!(apply(
            &mut machine,
            &LayoutDelta::PaneFacts { pane: record },
        ));
        assert_eq!(machine.panes.len(), 1, "updated in place, not duplicated");
        assert!(machine.panes[0].live);
    }
}
