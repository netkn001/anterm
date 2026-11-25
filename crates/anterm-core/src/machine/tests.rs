use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::tag::{SidebarTags, TagColor, TagId};

use super::appearance::{read_appearance, write_appearance};
use super::migration::{
    adopt_by_copy, adopt_into, adopt_legacy_file, copy_new, is_the_machines_instance,
};
use super::store::{BACKUP_SPACING, backup_path, keep_a_generation, parse_machine};
use super::*;

fn store() -> (Arc<MachineStore>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    (MachineStore::open(dir.path().join(MACHINE_FILE)), dir)
}

#[test]
fn the_appearance_hint_round_trips_and_defaults_to_light() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("nested").join(APPEARANCE_FILE);

    assert_eq!(
        read_appearance(&path),
        Appearance { dark: false },
        "a hint nobody has written yet reads as the default preset"
    );

    write_appearance(&path, Appearance { dark: true }).unwrap();
    assert_eq!(read_appearance(&path), Appearance { dark: true });

    write_appearance(&path, Appearance { dark: false }).unwrap();
    assert_eq!(read_appearance(&path), Appearance { dark: false });

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(
        read_appearance(&path),
        Appearance { dark: false },
        "a corrupt hint must not decide the background either"
    );
}

fn seed(pane: u64, cwd: &str) -> PaneSeed {
    PaneSeed {
        pane,
        cwd: Some(cwd.to_string()),
        ssh_spec: None,
        agent: None,
        shell: None,
    }
}

fn store_with_tab() -> (Arc<MachineStore>, tempfile::TempDir, Tab) {
    let (store, dir) = store();
    let tab = store
        .tab_create(None, seed(1, "/work"), None, None)
        .unwrap();
    (store, dir, tab)
}

fn recorded(store: &Arc<MachineStore>) -> (Subscription, Arc<Mutex<Vec<LayoutDelta>>>) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&heard);
    let sub = store.subscribe(Arc::new(move |delta: &LayoutDelta| {
        sink.lock().unwrap().push(delta.clone());
    }));
    (sub, heard)
}

#[test]
fn a_client_minted_tab_id_is_kept_and_a_duplicate_is_refused() {
    let (store, _dir, _tab) = store_with_tab();
    let id = TabId::new();
    let tab = store
        .tab_create(None, seed(2, "/b"), Some(id), None)
        .unwrap();
    assert_eq!(tab.id, id);

    let refused = store
        .tab_create(None, seed(3, "/c"), Some(id), None)
        .expect_err("a taken tab id must refuse");
    assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    assert!(
        store.pane(3).is_none(),
        "the refused create adopted no pane either"
    );
}

#[test]
fn the_tree_round_trips_through_the_file() {
    let (store, dir) = store();
    store
        .tab_create(None, seed(1, "/work"), None, None)
        .unwrap();
    store
        .pane_split(
            1,
            Axis::Vertical,
            0.3,
            seed(2, "/work/api"),
            false,
            None,
        )
        .unwrap();

    let reopened = MachineStore::open(dir.path().join(MACHINE_FILE));
    let machine = reopened.machine();
    assert_eq!(machine.tabs.len(), 1);
    assert_eq!(machine.tabs[0].root.pane_ids(), vec![1, 2]);
    match &machine.tabs[0].root {
        PaneNode::Split { axis, ratio, .. } => {
            assert_eq!(*axis, Axis::Vertical);
            assert!((ratio - 0.3).abs() < 1e-6);
        }
        PaneNode::Leaf { .. } => panic!("the split has to survive"),
    }
    assert_eq!(
        machine.panes.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![1, 2],
        "the pane registry rides the same file"
    );
    assert_eq!(machine.panes[0].cwd.as_deref(), Some("/work"));
}

#[test]
fn a_reopened_store_marks_every_pane_awaiting_revival() {
    let (store, dir) = store();
    store
        .tab_create(None, seed(7, "/work"), None, None)
        .unwrap();
    assert!(
        store.pane(7).unwrap().live,
        "the pane its own client just seeded is live"
    );

    let restarted = MachineStore::open(dir.path().join(MACHINE_FILE));
    let record = restarted.pane(7).expect("the record survives the restart");
    assert!(!record.live, "a restarted daemon has no live panes");
    assert_eq!(
        record.cwd.as_deref(),
        Some("/work"),
        "the facts a successor spawns from survive"
    );
    assert_eq!(
        restarted.machine().tabs[0].root.pane_ids(),
        vec![7],
        "the leaf still names the dead pane: that is the revival slot"
    );
}

#[test]
fn a_seed_for_an_already_dead_pane_registers_as_awaiting_revival() {
    let (store, _dir) = store();
    store.set_liveness_probe(Arc::new(|id| id == 1));
    store
        .tab_create(None, PaneSeed::bare(1), None, None)
        .unwrap();
    store
        .pane_split(1, Axis::Vertical, 0.5, PaneSeed::bare(2), false, None)
        .unwrap();

    assert!(store.pane(1).unwrap().live, "the probe vouched for pane 1");
    assert!(
        !store.pane(2).unwrap().live,
        "pane 2 died before its adopting op; its record must be born revivable"
    );
}

#[test]
fn replacing_a_dead_pane_rebinds_the_leaf_and_spends_the_record() {
    let (store, dir) = store();
    store
        .tab_create(None, seed(7, "/work"), None, None)
        .unwrap();

    let restarted = MachineStore::open(dir.path().join(MACHINE_FILE));
    let (_sub, heard) = recorded(&restarted);
    restarted.pane_replace(7, seed(42, "/work"), None).unwrap();

    assert_eq!(restarted.machine().tabs[0].root.pane_ids(), vec![42]);
    assert!(restarted.pane(7).is_none(), "the old record is spent");
    assert!(restarted.pane(42).unwrap().live);
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 1);
    match &heard[0] {
        LayoutDelta::TabRestructured { tab, pane } => {
            assert_eq!(tab.root.pane_ids(), vec![42]);
            assert_eq!(pane.as_ref().map(|p| p.id), Some(42));
        }
        other => panic!("expected TabRestructured, got {other:?}"),
    }
}

#[test]
fn a_created_tab_lands_at_its_position_and_becomes_active() {
    let (store, _dir, first) = store_with_tab();
    let second = store
        .tab_create(None, seed(2, "/b"), None, None)
        .unwrap();
    let between = store
        .tab_create(Some(1), seed(3, "/c"), None, None)
        .unwrap();

    let machine = store.machine();
    let order: Vec<TabId> = machine.tabs.iter().map(|t| t.id).collect();
    assert_eq!(order, vec![first.id, between.id, second.id]);
    assert_eq!(machine.active_tab, Some(between.id));

    let clamped = store
        .tab_create(Some(99), seed(4, "/d"), None, None)
        .unwrap();
    assert_eq!(store.machine().tabs.last().unwrap().id, clamped.id);
}

#[test]
fn closing_a_tab_forgets_its_panes_and_heals_the_active_tab() {
    let (store, _dir, first) = store_with_tab();
    let second = store
        .tab_create(None, seed(2, "/b"), None, None)
        .unwrap();
    store.set_active_tab(second.id, None).unwrap();

    let (_sub, heard) = recorded(&store);
    let dropped = store.tab_close(second.id, None).unwrap();
    assert_eq!(dropped, vec![2]);
    let machine = store.machine();
    assert_eq!(machine.tabs.len(), 1);
    assert_eq!(
        machine.active_tab,
        Some(first.id),
        "the active tab may not dangle on a closed id"
    );
    assert!(
        matches!(
            heard.lock().unwrap().as_slice(),
            [
                LayoutDelta::TabClosed { tab },
                LayoutDelta::ActiveTabChanged { tab: active }
            ] if *tab == second.id && *active == first.id
        ),
        "heard {:?}",
        heard.lock().unwrap()
    );

    heard.lock().unwrap().clear();
    let dropped = store.tab_close(first.id, None).unwrap();
    assert_eq!(dropped, vec![1]);
    assert_eq!(
        store.machine().active_tab,
        None,
        "a machine with no tabs has no active one — the home-page state"
    );
    assert_eq!(
        heard.lock().unwrap().len(),
        1,
        "losing the last tab needs no ActiveTabChanged: no tabs, no active tab"
    );
}

#[test]
fn tabs_rename_move_and_tag_in_place() {
    let (store, _dir, first) = store_with_tab();
    let second = store
        .tab_create(None, seed(2, "/b"), None, None)
        .unwrap();

    let mut catalog = SidebarTags::default();
    let work = catalog.create("Work", TagColor::Blue).unwrap();
    let ops = catalog.create("Ops", TagColor::Green).unwrap();
    let unknown = TagId::new();
    store.set_tags(catalog, None).unwrap();

    store
        .tab_rename(first.id, Some("build".into()), None)
        .unwrap();
    let (_sub, heard) = recorded(&store);
    store
        .tab_set_tags(first.id, vec![ops, unknown, work, ops], None)
        .unwrap();
    // Setting the same effective tag list again is a no-op and raises no delta.
    store
        .tab_set_tags(first.id, vec![ops, work], None)
        .unwrap();
    store.tab_move(first.id, 1, None).unwrap();

    let machine = store.machine();
    assert_eq!(machine.tabs[0].id, second.id);
    assert_eq!(machine.tabs[1].name.as_deref(), Some("build"));
    assert_eq!(machine.tabs[1].tags, vec![ops, work]);
    let heard = heard.lock().unwrap();
    assert_eq!(
        *heard,
        vec![
            LayoutDelta::TabTagsChanged {
                tab: first.id,
                tags: vec![ops, work],
            },
            LayoutDelta::TabMoved {
                tab: first.id,
                to: 1,
            },
        ]
    );
}

/// Deleting a tag prunes it from every tab that carried it in the same
/// mutation, announcing `TabTagsChanged` for affected tabs before `TagsChanged`.
#[test]
fn dropping_a_tag_prunes_it_from_tabs() {
    let (store, _dir, first) = store_with_tab();
    let second = store
        .tab_create(None, seed(2, "/b"), None, None)
        .unwrap();
    let mut tags = SidebarTags::default();
    let keep = tags.create("keep", TagColor::Blue).unwrap();
    let drop = tags.create("drop", TagColor::Red).unwrap();
    store.set_tags(tags.clone(), None).unwrap();
    store
        .tab_set_tags(first.id, vec![drop, keep], None)
        .unwrap();
    store.tab_set_tags(second.id, vec![keep], None).unwrap();

    let (_sub, heard) = recorded(&store);
    let mut kept = tags;
    assert!(kept.remove(drop));
    store.set_tags(kept.clone(), None).unwrap();
    // Re-applying the identical catalog emits no deltas.
    store.set_tags(kept.clone(), None).unwrap();

    let machine = store.machine();
    assert_eq!(machine.tags, kept);
    assert_eq!(machine.tabs[0].tags, vec![keep], "dropped tag pruned");
    assert_eq!(machine.tabs[1].tags, vec![keep], "untouched");
    let heard = heard.lock().unwrap();
    assert_eq!(
        *heard,
        vec![
            LayoutDelta::TabTagsChanged {
                tab: first.id,
                tags: vec![keep],
            },
            LayoutDelta::TagsChanged { tags: kept },
        ]
    );
}

#[test]
fn splitting_and_closing_panes_reshapes_the_tree() {
    let (store, _dir, tab) = store_with_tab();
    store
        .pane_split(1, Axis::Horizontal, 0.5, seed(2, "/b"), false, None)
        .unwrap();
    store
        .pane_split(2, Axis::Vertical, 0.5, seed(3, "/c"), true, None)
        .unwrap();
    assert_eq!(
        store.machine().tabs[0].root.pane_ids(),
        vec![1, 3, 2],
        "`first` puts the new pane on the a side"
    );

    let dropped = store.pane_close(3, None).unwrap();
    assert_eq!(dropped, vec![3]);
    assert_eq!(store.machine().tabs[0].root.pane_ids(), vec![1, 2]);

    store.pane_close(2, None).unwrap();
    assert!(matches!(
        store.machine().tabs[0].root,
        PaneNode::Leaf { pane: 1 }
    ));

    let (_sub, heard) = recorded(&store);
    store.pane_close(1, None).unwrap();
    assert!(store.machine().tabs.is_empty());
    assert!(matches!(
        heard.lock().unwrap()[0],
        LayoutDelta::TabClosed { tab: id } if id == tab.id
    ));
}

#[test]
fn a_ratio_change_lands_on_the_split_its_path_names() {
    let (store, _dir, tab) = store_with_tab();
    store
        .pane_split(1, Axis::Horizontal, 0.5, seed(2, "/b"), false, None)
        .unwrap();
    store
        .pane_split(2, Axis::Vertical, 0.5, seed(3, "/c"), false, None)
        .unwrap();

    store
        .pane_set_ratio(tab.id, vec![Side::B], 0.7, None)
        .unwrap();
    match &store.machine().tabs[0].root {
        PaneNode::Split { a, b, ratio, .. } => {
            assert!((ratio - 0.5).abs() < 1e-6, "the root ratio is untouched");
            assert!(matches!(&**a, PaneNode::Leaf { pane: 1 }));
            match &**b {
                PaneNode::Split { ratio, .. } => assert!((ratio - 0.7).abs() < 1e-6),
                PaneNode::Leaf { .. } => panic!("the nested split is gone"),
            }
        }
        PaneNode::Leaf { .. } => panic!("the root split is gone"),
    }

    let err = store
        .pane_set_ratio(tab.id, vec![Side::A], 0.6, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

    store
        .pane_set_ratio(tab.id, vec![], 0.0001, None)
        .unwrap();
    match &store.machine().tabs[0].root {
        PaneNode::Split { ratio, .. } => assert!(*ratio >= 0.05),
        PaneNode::Leaf { .. } => unreachable!(),
    }
}

#[test]
fn moving_a_pane_between_tabs_dissolves_an_emptied_tab() {
    let (store, _dir, first) = store_with_tab();
    let second = store
        .tab_create(None, seed(2, "/b"), None, None)
        .unwrap();

    store.pane_move(2, 1, Axis::Vertical, false, None).unwrap();
    let machine = store.machine();
    assert_eq!(machine.tabs.len(), 1, "the emptied tab dissolved");
    assert_eq!(machine.tabs[0].id, first.id);
    assert_eq!(machine.tabs[0].root.pane_ids(), vec![1, 2]);
    assert!(
        !machine.tabs.iter().any(|t| t.id == second.id),
        "the source tab is gone"
    );
    assert!(store.pane(2).is_some(), "the pane moved; it did not die");

    let err = store
        .pane_move(2, 2, Axis::Vertical, false, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn a_refused_operation_changes_nothing_and_notifies_nobody() {
    let (store, _dir, tab) = store_with_tab();
    let before = store.machine();
    let (_sub, heard) = recorded(&store);

    assert!(store.set_active_tab(TabId::new(), None).is_err());
    assert!(store.tab_close(TabId::new(), None).is_err());
    assert!(
        store
            .pane_split(999, Axis::Vertical, 0.5, seed(9, "/x"), false, None)
            .is_err()
    );
    assert!(store.pane_close(999, None).is_err());
    assert!(
        store
            .pane_set_ratio(tab.id, vec![Side::A], 0.5, None)
            .is_err()
    );
    assert!(
        store
            .pane_set_ratio(tab.id, vec![], f32::NAN, None)
            .is_err()
    );
    assert!(store.pane_replace(999, seed(9, "/x"), None).is_err());

    assert_eq!(store.machine(), before);
    assert!(heard.lock().unwrap().is_empty());
    assert!(
        store.pane(9).is_none(),
        "a seed on a refused op must not leak into the registry"
    );
}

#[test]
fn a_delta_reaches_every_subscriber_but_its_author() {
    let (store, _dir, tab) = store_with_tab();
    let (author, heard_by_author) = recorded(&store);
    let (_other, heard_by_other) = recorded(&store);

    store
        .tab_rename(tab.id, Some("renamed".into()), Some(author.id()))
        .unwrap();
    assert!(heard_by_author.lock().unwrap().is_empty());
    assert_eq!(heard_by_other.lock().unwrap().len(), 1);

    store.tab_rename(tab.id, None, None).unwrap();
    assert_eq!(heard_by_author.lock().unwrap().len(), 1);
    assert_eq!(heard_by_other.lock().unwrap().len(), 2);
}

#[test]
fn dropping_a_subscription_stops_the_deltas() {
    let (store, _dir, tab) = store_with_tab();
    let (sub, heard) = recorded(&store);
    store.tab_rename(tab.id, Some("one".into()), None).unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1);
    drop(sub);
    store.tab_rename(tab.id, Some("two".into()), None).unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1);
}

#[test]
fn pane_facts_update_the_record_and_reach_every_client() {
    let (store, _dir, _tab) = store_with_tab();
    let (sub, heard) = recorded(&store);

    store.note_pane_facts(1, |p| {
        p.cwd = Some("/work/deeper".into());
    });
    let record = store.pane(1).unwrap();
    assert_eq!(record.cwd.as_deref(), Some("/work/deeper"));
    {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 1);
        assert!(matches!(&heard[0], LayoutDelta::PaneFacts { pane } if pane.id == 1));
    }

    store.note_pane_facts(1, |_| {});
    store.note_pane_facts(999, |p| p.cwd = Some("/ghost".into()));
    assert_eq!(heard.lock().unwrap().len(), 1);
    drop(sub);
}

#[test]
fn a_pane_already_in_the_tree_cannot_be_adopted_again() {
    let (store, _dir, _tab) = store_with_tab();
    store.note_pane_facts(1, |p| p.cwd = Some("/observed".into()));

    let err = store
        .tab_create(None, seed(1, "/stale"), None, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(
        store.machine().tabs.len(),
        1,
        "the refused tab must not half-exist"
    );
    assert_eq!(
        store.pane(1).unwrap().cwd.as_deref(),
        Some("/observed"),
        "and the stale seed must not clobber the daemon's own facts"
    );
}

#[test]
fn published_observations_land_in_the_installed_store() {
    let _slot = OBSERVE_SLOT.lock().unwrap_or_else(|e| e.into_inner());
    observe_pane(1, |p| p.cwd = Some("/nowhere".into()));

    let (store, _dir, _tab) = store_with_tab();
    publish_observations(&store);
    observe_pane(1, |p| p.cwd = Some("/observed/here".into()));
    assert_eq!(
        store.pane(1).unwrap().cwd.as_deref(),
        Some("/observed/here")
    );
    withdraw_observations();
}

#[test]
fn the_default_path_ends_at_the_documented_file() {
    match default_machine_path() {
        Ok(path) => assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(MACHINE_FILE)
        ),
        Err(e) => assert!(e.to_string().contains(DATA_DIR_ENV)),
    }
}

/// The invariant the whole file rests on: one config directory is one
/// instance, tree included.
#[test]
fn the_tree_lives_in_the_config_directory() {
    if std::env::var_os(DATA_DIR_ENV).is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("anterm-covtest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok();
    crate::config::set_config_dir(dir);
    let config_dir = crate::config::config_dir_path();
    assert!(config_dir.is_some(), "the pin puts one in force");
    assert_eq!(
        default_machine_path().unwrap().parent(),
        config_dir.as_deref(),
        "a config directory nobody else names must not share anybody's tree"
    );
}

#[test]
fn only_the_machines_own_instance_inherits_the_legacy_tree() {
    let machines = PathBuf::from("/home/u/.config/anterm");
    assert!(is_the_machines_instance(
        Some(&machines),
        Some(&machines.clone())
    ));
    assert!(
        !is_the_machines_instance(Some(Path::new("/tmp/scratch")), Some(&machines)),
        "an instance pointed somewhere of its own starts on an empty tree"
    );
    assert!(
        !is_the_machines_instance(None, Some(&machines)),
        "nowhere to put it is not a licence to move it"
    );
    assert!(!is_the_machines_instance(Some(&machines), None));
}

#[test]
fn an_instance_of_its_own_leaves_the_machines_tree_where_it_is() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let legacy = old.path().join(MACHINE_FILE);
    std::fs::write(&legacy, br#"{"tabs":[],"panes":[]}"#).unwrap();

    adopt_into(Some(new.path()), Some(old.path()), false);

    assert!(
        legacy.exists(),
        "a --config-dir instance that takes the tree hands the primary an \
         empty one, which is the failure this change exists to stop"
    );
    assert!(!new.path().join(MACHINE_FILE).exists());
}

#[test]
fn an_upgrade_carries_the_appearance_hint_along_with_the_tree() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    for file in [MACHINE_FILE, APPEARANCE_FILE] {
        std::fs::write(old.path().join(file), b"{}").unwrap();
    }

    adopt_into(Some(new.path()), Some(old.path()), true);

    for file in [MACHINE_FILE, APPEARANCE_FILE] {
        assert!(new.path().join(file).exists(), "{file} was left behind");
        assert!(!old.path().join(file).exists(), "{file} was not moved");
    }
}

#[test]
fn one_directory_under_two_names_is_not_a_migration() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join(MACHINE_FILE);
    std::fs::write(&file, b"live").unwrap();

    adopt_into(Some(dir.path()), Some(dir.path()), true);

    assert_eq!(std::fs::read(&file).unwrap(), b"live");
}

#[test]
fn a_copied_tree_never_lands_on_one_already_there() {
    let dir = tempfile::TempDir::new().unwrap();
    let (from, to) = (dir.path().join("from"), dir.path().join("to"));
    std::fs::write(&from, b"carried").unwrap();

    copy_new(&from, &to).expect("nothing is there yet");
    assert_eq!(std::fs::read(&to).unwrap(), b"carried");

    let second = copy_new(&from, &to).expect_err("the second must not replace the first");
    assert_eq!(second.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&to).unwrap(), b"carried");
}

#[test]
fn a_source_already_carried_over_is_not_an_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let (legacy, current) = (dir.path().join("legacy"), dir.path().join("current"));
    std::fs::write(&current, b"the winner's").unwrap();

    adopt_by_copy(&legacy, &current, &io::Error::from(io::ErrorKind::NotFound));

    assert_eq!(std::fs::read(&current).unwrap(), b"the winner's");
}

#[test]
fn an_upgrade_carries_the_legacy_tree_forward() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let (legacy, current) = (old.path().join(MACHINE_FILE), new.path().join(MACHINE_FILE));
    std::fs::write(&legacy, br#"{"tabs":[],"panes":[]}"#).unwrap();

    adopt_legacy_file(&legacy, &current);

    assert!(
        current.exists(),
        "moving the path without the tree would lose every tab at the \
         moment of upgrade, which is the failure this change exists to stop"
    );
    assert!(
        !legacy.exists(),
        "a move leaves nothing to be adopted twice"
    );
}

#[test]
fn a_legacy_file_never_overwrites_the_tree_in_use() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let (legacy, current) = (old.path().join(MACHINE_FILE), new.path().join(MACHINE_FILE));
    std::fs::write(&legacy, b"stale").unwrap();
    std::fs::write(&current, b"live").unwrap();

    adopt_legacy_file(&legacy, &current);

    assert_eq!(std::fs::read(&current).unwrap(), b"live");
    assert!(
        legacy.exists(),
        "the one it did not adopt is not the one it may delete"
    );
}

#[test]
fn nothing_to_adopt_creates_nothing() {
    let old = tempfile::TempDir::new().unwrap();
    let new = tempfile::TempDir::new().unwrap();
    let current = new.path().join(MACHINE_FILE);

    adopt_legacy_file(&old.path().join(MACHINE_FILE), &current);

    assert!(
        !current.exists(),
        "a fresh install has nothing to carry forward and must not be handed an empty tree"
    );
}

#[test]
fn an_observation_is_broadcast_at_once_and_written_a_little_later() {
    let (store, dir, tab) = store_with_tab();
    let path = dir.path().join(MACHINE_FILE);
    let (_sub, heard) = recorded(&store);

    store.note_pane_facts(1, |p| p.cwd = Some("/work/deeper".into()));
    assert_eq!(
        heard.lock().unwrap().len(),
        1,
        "the client hears the fact immediately; only the disk waits"
    );
    assert!(
        !std::fs::read_to_string(&path).unwrap().contains("deeper"),
        "an observation must not write the document synchronously"
    );

    store.flush();
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("deeper"),
        "…and the flush is what puts it on disk"
    );
    let before = std::fs::metadata(&path).unwrap().len();
    store.flush();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), before);

    store.note_pane_facts(1, |p| p.cwd = Some("/work/deepest".into()));
    store
        .tab_rename(tab.id, Some("web".into()), None)
        .unwrap();
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("deepest"),
        "a structural write carries whatever the facts left unwritten"
    );
}

#[test]
fn a_structural_edit_is_on_disk_before_anyone_hears_about_it() {
    let (store, dir) = store();
    let path = dir.path().join(MACHINE_FILE);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let path_in_callback = path.clone();
    let _sub = store.subscribe(Arc::new(move |_delta: &LayoutDelta| {
        sink.lock()
            .unwrap()
            .push(std::fs::read_to_string(&path_in_callback).unwrap_or_default());
    }));

    store
        .tab_create(None, seed(1, "/api"), None, None)
        .unwrap();
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen[0].contains("/api"),
        "the delta arrived before the file said so: {}",
        seen[0]
    );
}

#[test]
fn a_corrupt_file_is_quarantined_rather_than_overwritten() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, b"{ this is not json").unwrap();

    let store = MachineStore::open(&path);
    assert!(store.machine().tabs.is_empty());
    store
        .tab_create(None, PaneSeed::bare(1), None, None)
        .unwrap();
    let aside = std::fs::read_to_string(path.with_extension("json.corrupt")).unwrap();
    assert_eq!(aside, "{ this is not json");

    std::fs::write(&path, b"corrupt again").unwrap();
    let store = MachineStore::open(&path);
    store
        .tab_create(None, PaneSeed::bare(2), None, None)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(path.with_extension("json.corrupt")).unwrap(),
        "{ this is not json",
        "the first rescue copy is still the first one"
    );
    assert_eq!(
        std::fs::read_to_string(path.with_extension("json.corrupt.1")).unwrap(),
        "corrupt again"
    );
}

#[cfg(unix)]
#[test]
fn the_document_is_written_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let (store, dir) = store();
    store
        .tab_create(None, PaneSeed::bare(1), None, None)
        .unwrap();
    let mode = std::fs::metadata(dir.path().join(MACHINE_FILE))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_moved_aside_rather_than_overwritten() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, b"{\"tabs\":[]}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_to_string(&path).is_ok() {
        return;
    }

    let store = MachineStore::open(&path);
    store
        .tab_create(None, PaneSeed::bare(1), None, None)
        .unwrap();

    let aside = path.with_extension("json.corrupt");
    assert!(aside.exists(), "the unreadable original must be kept");
    std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read_to_string(&aside).unwrap(),
        "{\"tabs\":[]}",
        "moved aside byte-for-byte, ready for a hand repair"
    );
}

/// One tree with `tabs` tabs in it, written out the way `persist` writes.
fn document(tabs: u64) -> Vec<u8> {
    let machine = Machine {
        tabs: (0..tabs).map(Tab::leaf).collect(),
        ..Machine::default()
    };
    serde_json::to_vec_pretty(&machine).unwrap()
}

fn tabs_in(path: &Path) -> usize {
    let text = std::fs::read_to_string(path).expect("a generation must be there to read");
    parse_machine(&text).expect("and it must parse").tabs.len()
}

/// A machine tree is written whole on every mutation, so the write that
/// loses the layout also erases the only copy of it (#716). One generation
/// back is the least that makes a bad write survivable by hand.
#[test]
fn the_document_being_replaced_is_kept_beside_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    let store = MachineStore::open(&path);

    store
        .tab_create(None, seed(1, "/work"), None, None)
        .unwrap();
    store
        .tab_create(None, seed(2, "/work"), None, None)
        .unwrap();

    let kept = backup_path(&path, 0);
    assert!(
        kept.exists(),
        "the second write has a first write to keep: {}",
        kept.display()
    );
    assert_eq!(
        tabs_in(&kept),
        1,
        "and what it kept is the document that write replaced"
    );
}

/// The whole reason the generations are spaced. The failure this came from
/// arrived as a burst — nineteen `TabClose`s, nineteen persists, seconds
/// apart — and a ring that rotated on every write would have held three
/// copies of the damage by the time anyone looked at it.
#[test]
fn a_burst_of_writes_cannot_roll_the_history_away() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, document(19)).unwrap();
    keep_a_generation(&path, Duration::ZERO).unwrap();

    for remaining in (0..19).rev() {
        std::fs::write(&path, document(remaining)).unwrap();
        keep_a_generation(&path, BACKUP_SPACING).unwrap();
    }

    assert_eq!(tabs_in(&path), 0, "the burst emptied the live tree");
    assert_eq!(
        tabs_in(&backup_path(&path, 0)),
        19,
        "and the kept generation is from before it, not from one write ago"
    );
}

#[test]
fn the_ring_keeps_three_generations_and_no_more() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);

    for tabs in 1..=6 {
        std::fs::write(&path, document(tabs)).unwrap();
        keep_a_generation(&path, Duration::ZERO).unwrap();
    }

    assert_eq!(tabs_in(&backup_path(&path, 0)), 6);
    assert_eq!(tabs_in(&backup_path(&path, 1)), 5);
    assert_eq!(tabs_in(&backup_path(&path, 2)), 4);
    assert!(
        !backup_path(&path, 3).exists(),
        "the ring is bounded; a tree rewritten every few seconds must not \
         grow a file per write forever"
    );
}

#[test]
fn a_tree_that_does_not_parse_is_loaded_from_the_newest_backup() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(MACHINE_FILE);
    std::fs::write(&path, document(3)).unwrap();
    keep_a_generation(&path, Duration::ZERO).unwrap();
    std::fs::write(&path, b"{ not json").unwrap();

    let store = MachineStore::open(&path);

    assert_eq!(
        store.machine().tabs.len(),
        3,
        "the layout comes back from the kept generation"
    );
    assert!(
        path.with_extension("json.corrupt").exists(),
        "and the unparseable document is still kept for inspection"
    );
}

#[test]
fn a_sparse_document_decodes_with_defaults() {
    let machine: Machine = serde_json::from_str(r#"{"tabs":[{"root":{"Leaf":{"pane":3}}}]}"#)
        .expect("missing fields default rather than fail");
    assert_eq!(machine.tabs.len(), 1);
    assert_eq!(machine.tabs[0].root.pane_ids(), vec![3]);
    assert!(machine.panes.is_empty());
}

#[test]
fn legacy_workspaces_json_flattens_into_machine() {
    let tab_a = TabId::new();
    let tab_b = TabId::new();
    let tab_c = TabId::new();
    let closed_old = TabId::new();
    let closed_new = TabId::new();

    let legacy = serde_json::json!({
        "workspaces": [
            {
                "id": "6f1c0c0e-3b1a-4a8e-9d57-3c2b7c1f0a11",
                "name": "older",
                "last_active": 100,
                "tabs": [
                    {
                        "id": tab_a,
                        "group": "11111111-1111-1111-1111-111111111111",
                        "last_auto": { "ssh_host": "me@box" },
                        "root": { "Leaf": { "pane": 1 } }
                    }
                ],
                "active_tab": tab_a,
                "groups": {
                    "pinned": [{ "id": "11111111-1111-1111-1111-111111111111", "label": "work" }],
                    "auto_collapsed": [{ "ssh_host": "me@box" }],
                    "ungrouped_collapsed": false
                },
                "closed": [
                    { "tab": { "id": closed_new, "root": { "Leaf": { "pane": 11 } } }, "closed_at": 200 }
                ]
            },
            {
                "id": "9a1c0c0e-3b1a-4a8e-9d57-3c2b7c1f0a22",
                "name": "newer",
                "last_active": 500,
                "tabs": [
                    { "id": tab_b, "root": { "Leaf": { "pane": 2 } } },
                    { "id": tab_c, "root": { "Leaf": { "pane": 3 } } }
                ],
                "active_tab": tab_c,
                "groups": {
                    "pinned": [
                        { "id": "11111111-1111-1111-1111-111111111111", "label": "work" },
                        { "id": "22222222-2222-2222-2222-222222222222", "label": "ops" }
                    ],
                    "auto_collapsed": [{ "ssh_host": "me@box" }],
                    "ungrouped_collapsed": true
                },
                "closed": [
                    { "tab": { "id": closed_old, "root": { "Leaf": { "pane": 10 } } }, "closed_at": 150 }
                ]
            }
        ],
        "panes": [
            { "id": 1, "cwd": "/a", "live": true },
            { "id": 2, "cwd": "/b", "live": true }
        ]
    });

    let machine = parse_machine(&legacy.to_string()).expect("legacy tree migrates");
    let tab_ids: Vec<TabId> = machine.tabs.iter().map(|t| t.id).collect();
    assert_eq!(tab_ids, vec![tab_a, tab_b, tab_c]);
    assert_eq!(
        machine.active_tab,
        Some(tab_c),
        "picks the most recently active workspace's active_tab"
    );
    assert!(
        machine.tags.is_empty(),
        "legacy group definitions are dropped cleanly"
    );
    assert!(
        machine.tabs.iter().all(|t| t.tags.is_empty()),
        "legacy tab group fields are dropped cleanly"
    );
    assert_eq!(
        machine.closed.iter().map(|c| c.tab.id).collect::<Vec<_>>(),
        vec![closed_old, closed_new],
        "merges closed tabs oldest first"
    );
    assert!(
        machine.panes.iter().all(|p| !p.live),
        "panes still start awaiting revival"
    );
}

#[test]
fn legacy_single_machine_json_with_groups_decodes_cleanly() {
    let tab_id = TabId::new();
    let legacy = serde_json::json!({
        "tabs": [
            {
                "id": tab_id,
                "name": "dev",
                "group": "11111111-1111-1111-1111-111111111111",
                "last_auto": { "ssh_host": "me@box" },
                "root": { "Leaf": { "pane": 7 } }
            }
        ],
        "active_tab": tab_id,
        "groups": {
            "pinned": [{ "id": "11111111-1111-1111-1111-111111111111", "label": "work" }],
            "auto_collapsed": [{ "ssh_host": "me@box" }],
            "ungrouped_collapsed": true
        },
        "panes": [
            { "id": 7, "cwd": "/work", "live": true }
        ]
    });

    let machine = parse_machine(&legacy.to_string()).expect("legacy single-machine JSON parses");
    assert_eq!(machine.tabs.len(), 1);
    assert_eq!(machine.tabs[0].id, tab_id);
    assert_eq!(machine.tabs[0].name.as_deref(), Some("dev"));
    assert!(machine.tabs[0].tags.is_empty());
    assert!(machine.tags.is_empty());
    assert!(!machine.panes[0].live);
}

#[test]
fn a_sleeping_tab_keeps_its_panes_and_says_so_in_a_delta() {
    let (store, _dir, tab) = store_with_tab();
    store
        .pane_split(1, Axis::Horizontal, 0.5, seed(2, "/work"), false, None)
        .unwrap();
    let (_sub, heard) = recorded(&store);

    let panes = store.tab_set_hibernated(tab.id, true, None).unwrap();

    assert_eq!(panes, vec![1, 2]);
    let m = store.machine();
    let asleep = &m.tabs[0];
    assert!(asleep.hibernated);
    assert_eq!(asleep.root.pane_ids(), vec![1, 2]);
    assert!(m.panes.iter().any(|p| p.id == 1) && m.panes.iter().any(|p| p.id == 2));
    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), 1);
    match &heard[0] {
        LayoutDelta::TabRestructured { tab: t, pane: None } => {
            assert_eq!(t.id, tab.id);
            assert!(t.hibernated, "the delta carries the mark");
        }
        other => panic!("expected TabRestructured, heard {other:?}"),
    }
}

#[test]
fn setting_the_mark_a_tab_already_has_raises_nothing() {
    let (store, _dir, tab) = store_with_tab();
    let (_sub, heard) = recorded(&store);
    store.tab_set_hibernated(tab.id, false, None).unwrap();
    assert!(heard.lock().unwrap().is_empty());

    store.tab_set_hibernated(tab.id, true, None).unwrap();
    store.tab_set_hibernated(tab.id, true, None).unwrap();
    assert_eq!(heard.lock().unwrap().len(), 1);

    store.tab_set_hibernated(tab.id, false, None).unwrap();
    assert!(!store.machine().tabs[0].hibernated);
    assert_eq!(heard.lock().unwrap().len(), 2);
}

#[test]
fn hibernating_a_tab_that_is_not_there_is_refused() {
    let (store, _dir, _tab) = store_with_tab();
    let err = store
        .tab_set_hibernated(TabId::new(), true, None)
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn a_sleeping_tab_is_still_asleep_after_the_tree_is_read_back() {
    let (store, dir, tab) = store_with_tab();
    let other = store
        .tab_create(None, seed(5, "/else"), None, None)
        .unwrap();
    store.tab_set_hibernated(tab.id, true, None).unwrap();
    drop(store);

    let reopened = MachineStore::open(dir.path().join(MACHINE_FILE));
    let m = reopened.machine();
    let tabs = &m.tabs;
    assert!(tabs.iter().find(|t| t.id == tab.id).unwrap().hibernated);
    assert!(!tabs.iter().find(|t| t.id == other.id).unwrap().hibernated);
    let record = m.panes.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(record.cwd.as_deref(), Some("/work"));
    assert!(!record.live);
}

#[test]
fn the_mark_is_only_written_while_it_is_set() {
    let tab = Tab::leaf(4);
    let json = serde_json::to_string(&tab).unwrap();
    assert!(!json.contains("hibernated"), "{json}");
    let back: Tab = serde_json::from_str(&json).unwrap();
    assert!(!back.hibernated);

    let asleep = Tab {
        hibernated: true,
        ..Tab::leaf(4)
    };
    let back: Tab = serde_json::from_str(&serde_json::to_string(&asleep).unwrap()).unwrap();
    assert!(back.hibernated);
}

const NOON: u64 = 1_800_000_000;

#[test]
fn a_remembered_close_keeps_the_tab_and_its_pane_records() {
    let (store, _dir, tab) = store_with_tab();
    store
        .pane_split(1, Axis::Horizontal, 0.5, seed(2, "/else"), false, None)
        .unwrap();
    assert!(store.machine().panes.iter().all(|p| p.live));
    let (_sub, heard) = recorded(&store);

    let closed = store
        .tab_close_remembered(tab.id, &[1, 2], NOON, None)
        .unwrap();

    assert_eq!(closed.stopped, vec![1, 2]);
    assert!(closed.strays.is_empty() && closed.dropped.is_empty());
    let m = store.machine();
    assert!(m.tabs.is_empty());
    let entry = &m.closed[0];
    assert_eq!(entry.tab.id, tab.id);
    assert_eq!(entry.closed_at, NOON);
    assert_eq!(entry.tab.root.pane_ids(), vec![1, 2]);
    assert!(
        m.panes.iter().any(|p| p.id == 1) && m.panes.iter().any(|p| p.id == 2),
        "the records are what the reopen restores cwd and shell from"
    );
    assert!(
        m.panes.iter().all(|p| !p.live),
        "a stopped pane's record must not read as running to `anterm wait`"
    );
    let heard = heard.lock().unwrap();
    assert!(matches!(&heard[0], LayoutDelta::TabClosed { tab: t } if *t == tab.id));
}

#[test]
fn panes_the_tree_never_recorded_in_the_tab_are_strays() {
    let (store, _dir, tab) = store_with_tab();
    store
        .tab_create(None, seed(5, "/other"), None, None)
        .unwrap();

    let closed = store
        .tab_close_remembered(tab.id, &[1, 9, 5], NOON, None)
        .unwrap();

    assert_eq!(closed.stopped, vec![1]);
    assert_eq!(closed.strays, vec![9]);
}

#[test]
fn reopening_takes_the_newest_closed_tab_off_the_list() {
    let (store, _dir, first) = store_with_tab();
    let second = store
        .tab_create(None, seed(2, "/second"), None, None)
        .unwrap();
    store
        .tab_close_remembered(first.id, &[1], NOON, None)
        .unwrap();
    store
        .tab_close_remembered(second.id, &[2], NOON + 5, None)
        .unwrap();

    let (reopened, dropped) = store.tab_reopen(None, NOON + 10, None).unwrap();
    let reopened = reopened.expect("two tabs were closed");
    assert!(dropped.is_empty());
    assert_eq!(reopened.tab.id, second.id);
    assert_eq!(reopened.closed_at, NOON + 5);
    assert_eq!(reopened.panes.len(), 1);
    assert_eq!(reopened.panes[0].cwd.as_deref(), Some("/second"));

    let (older, _) = store.tab_reopen(None, NOON + 10, None).unwrap();
    assert_eq!(older.unwrap().tab.id, first.id);
    let (none, _) = store.tab_reopen(None, NOON + 10, None).unwrap();
    assert!(none.is_none());
    assert!(store.machine().closed.is_empty());
}

#[test]
fn a_reopened_tab_can_be_pushed_back_under_its_old_ids() {
    let (store, _dir, tab) = store_with_tab();
    store
        .tab_close_remembered(tab.id, &[1], NOON, None)
        .unwrap();
    store.tab_reopen(None, NOON, None).unwrap();

    let back = store
        .tab_create(None, PaneSeed::bare(1), Some(tab.id), None)
        .unwrap();

    assert_eq!(back.id, tab.id);
    let m = store.machine();
    let record = m.panes.iter().find(|p| p.id == 1).unwrap();
    assert_eq!(
        record.cwd.as_deref(),
        Some("/work"),
        "what the seed does not say, the kept record still does"
    );
    assert_eq!(m.panes.iter().filter(|p| p.id == 1).count(), 1);
    let twice = store.tab_create(None, PaneSeed::bare(1), None, None);
    assert_eq!(twice.unwrap_err().kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn another_close_does_not_collect_a_remembered_tabs_records() {
    let (store, _dir, tab) = store_with_tab();
    let other = store
        .tab_create(None, seed(2, "/other"), None, None)
        .unwrap();
    store
        .tab_close_remembered(tab.id, &[1], NOON, None)
        .unwrap();

    let orphans = store.tab_close(other.id, None).unwrap();

    assert_eq!(orphans, vec![2]);
    assert!(store.machine().panes.iter().any(|p| p.id == 1));
}

#[test]
fn closed_tabs_expire_after_a_day_with_their_records() {
    let (store, _dir, old) = store_with_tab();
    let fresh = store
        .tab_create(None, seed(2, "/fresh"), None, None)
        .unwrap();
    let day = CLOSED_TAB_TTL.as_secs();
    store
        .tab_close_remembered(old.id, &[1], NOON, None)
        .unwrap();
    store
        .tab_close_remembered(fresh.id, &[2], NOON + 60, None)
        .unwrap();

    assert!(store.expire_closed_tabs(NOON + day - 1).is_empty());
    assert_eq!(store.expire_closed_tabs(NOON + day), vec![1]);

    let m = store.machine();
    let left: Vec<TabId> = m.closed.iter().map(|c| c.tab.id).collect();
    assert_eq!(left, vec![fresh.id]);
    assert!(!m.panes.iter().any(|p| p.id == 1));
    assert!(m.panes.iter().any(|p| p.id == 2));
    let (reopened, _) = store.tab_reopen(None, NOON + 2 * day, None).unwrap();
    assert!(
        reopened.is_none(),
        "an entry past its day is never handed back, timer or no timer"
    );
}

#[test]
fn an_entry_stamped_in_the_future_is_kept() {
    let (store, _dir, tab) = store_with_tab();
    store
        .tab_close_remembered(tab.id, &[1], NOON, None)
        .unwrap();
    assert!(store.expire_closed_tabs(NOON - 3600).is_empty());
    assert_eq!(store.machine().closed.len(), 1);
}

#[test]
fn the_list_keeps_only_the_newest_twenty() {
    let (store, _dir, first) = store_with_tab();
    store
        .tab_close_remembered(first.id, &[1], NOON, None)
        .unwrap();
    for n in 0..MAX_CLOSED_TABS as u64 {
        let pane = 100 + n;
        let tab = store
            .tab_create(None, seed(pane, "/t"), None, None)
            .unwrap();
        let closed = store
            .tab_close_remembered(tab.id, &[pane], NOON + 1 + n, None)
            .unwrap();
        let expect: Vec<u64> = match n + 1 == MAX_CLOSED_TABS as u64 {
            true => vec![1],
            false => Vec::new(),
        };
        assert_eq!(closed.dropped, expect, "after close {n}");
    }
    let m = store.machine();
    assert_eq!(m.closed.len(), MAX_CLOSED_TABS);
    assert!(m.closed.iter().all(|c| c.tab.id != first.id));
    assert!(!m.panes.iter().any(|p| p.id == 1));
}

#[test]
fn closed_tabs_survive_the_tree_being_read_back() {
    let (store, dir, tab) = store_with_tab();
    store.tab_set_hibernated(tab.id, true, None).unwrap();
    store
        .tab_close_remembered(tab.id, &[1], NOON, None)
        .unwrap();
    drop(store);

    let reopened = MachineStore::open(dir.path().join(MACHINE_FILE));
    let m = reopened.machine();
    let entry = &m.closed[0];
    assert_eq!(entry.tab.id, tab.id);
    assert_eq!(entry.closed_at, NOON);
    assert!(!entry.tab.hibernated, "reopening is the wake");
    assert_eq!(
        m.panes.iter().find(|p| p.id == 1).unwrap().cwd.as_deref(),
        Some("/work")
    );
}

#[test]
fn a_tree_without_the_list_loads_and_an_empty_list_is_not_written() {
    let old = r#"{"tabs":[],"panes":[]}"#;
    let m = parse_machine(old).unwrap();
    assert!(m.closed.is_empty());

    let json = serde_json::to_string(&Machine::default()).unwrap();
    assert!(!json.contains("closed"), "{json}");

    let entry = ClosedTab {
        tab: Tab::leaf(3),
        closed_at: NOON,
    };
    let machine = Machine {
        closed: vec![entry.clone()],
        ..Machine::default()
    };
    let back: Machine = serde_json::from_str(&serde_json::to_string(&machine).unwrap()).unwrap();
    assert_eq!(back.closed, vec![entry]);
    let stampless: ClosedTab =
        serde_json::from_str(r#"{"tab":{"root":{"Leaf":{"pane":3}}}}"#).unwrap();
    assert_eq!(stampless.closed_at, 0);
}
