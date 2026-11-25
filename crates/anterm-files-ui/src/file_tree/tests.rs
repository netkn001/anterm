use super::*;
use super::model::{SearchState, dir_note, optimistic_write, rollback_write, search_rows};

fn entry(name: &str, is_dir: bool) -> TreeEntry {
    TreeEntry {
        name: name.to_string(),
        path: PathBuf::from(format!("/x/{name}")),
        is_dir,
        ignored: false,
    }
}

#[test]
fn an_expanded_directory_says_why_it_has_no_children() {
    // Still in flight.
    assert_eq!(dir_note(false, None), TreeNote::Loading);
    // A listing that came back with nothing in it.
    assert_eq!(dir_note(false, Some(0)), TreeNote::Empty);
    // Entries landed, but the hidden filter took every one of them.
    assert_eq!(dir_note(false, Some(3)), TreeNote::HiddenOnly);
    // A directory the OS refused. `read_dir` used to be
    // `unwrap_or_default`ed, so this was byte-identical to Empty.
    assert_eq!(dir_note(true, Some(0)), TreeNote::Unreadable);
    assert_eq!(
        dir_note(true, None),
        TreeNote::Unreadable,
        "a known failure outranks a retry still in flight"
    );
}

#[test]
fn a_failed_search_says_so_instead_of_drawing_no_rows() {
    let mut search = SearchState::default();
    let walk = search.retarget("foo", false).expect("a new query walks");
    search.accept(walk, false, Vec::new());
    assert_eq!(
        search_rows(&search)
            .iter()
            .filter_map(|r| r.note)
            .collect::<Vec<_>>(),
        vec![TreeNote::SearchFailed],
        "without a note the column falls through to \"Nothing matches\""
    );

    // A search that ran and found nothing still draws nothing.
    let walk = search
        .retarget("bar", false)
        .expect("a changed query walks");
    search.accept(walk, true, Vec::new());
    assert!(search_rows(&search).is_empty());
}

#[test]
fn a_listing_superseded_in_flight_is_still_shown() {
    let mut loads: InFlight<DirKey> = InFlight::default();
    let mut children: ByHost<PathBuf, Vec<TreeEntry>> = ByHost::default();
    let id = HostId::LOCAL;
    let dir = PathBuf::from("/home/me");
    let key: DirKey = (id, dir.clone());

    let mut stale: HashSet<DirKey> = HashSet::new();

    assert!(loads.begin(key.clone()), "the listing goes out");
    loads.invalidate(&key);

    let again = land_listing(
        &mut loads,
        &mut children,
        &mut stale,
        &key,
        id,
        dir.clone(),
        vec![entry("src", true)],
    );
    assert!(again, "superseded, so the caller goes round again");
    assert!(
        children.get(id, &dir).is_some(),
        "the snapshot is on screen rather than thrown away"
    );

    assert!(loads.begin(key.clone()));
    let again = land_listing(
        &mut loads,
        &mut children,
        &mut stale,
        &key,
        id,
        dir.clone(),
        vec![entry("src", true)],
    );
    assert!(!again, "nothing superseded it, so one listing is enough");
}

#[test]
fn an_outdated_listing_stays_on_screen_until_its_replacement_lands() {
    let mut loads: InFlight<DirKey> = InFlight::default();
    let mut children: ByHost<PathBuf, Vec<TreeEntry>> = ByHost::default();
    let mut stale: HashSet<DirKey> = HashSet::new();
    let id = HostId::LOCAL;
    let dir = PathBuf::from("/home/me");
    let key: DirKey = (id, dir.clone());

    loads.begin(key.clone());
    land_listing(
        &mut loads,
        &mut children,
        &mut stale,
        &key,
        id,
        dir.clone(),
        vec![entry("src", true)],
    );

    stale.insert(key.clone());
    assert_eq!(
        children.get(id, &dir).map(Vec::len),
        Some(1),
        "the rows are still there to paint"
    );

    let current = children.get(id, &dir).is_some() && !stale.contains(&key);
    assert!(!current, "stale means re-ask");

    loads.begin(key.clone());
    land_listing(
        &mut loads,
        &mut children,
        &mut stale,
        &key,
        id,
        dir.clone(),
        vec![entry("src", true), entry("README", false)],
    );
    assert!(!stale.contains(&key), "the replacement clears the mark");
    assert_eq!(children.get(id, &dir).map(Vec::len), Some(2));
}

#[test]
fn a_directorys_own_event_does_not_relist_it() {
    let batch: HashSet<PathBuf> = [
        PathBuf::from("/home/me/.claude.json"),
        PathBuf::from("/home/me"),
    ]
    .into_iter()
    .collect();

    let dirs = dirs_to_relist(&batch, false);
    assert!(
        !dirs.contains(Path::new("/home/me")),
        "the home listing is not re-fetched for a dot-file write"
    );
    assert!(dirs.contains(Path::new("/home")), "its parent is");

    let batch: HashSet<PathBuf> = [
        PathBuf::from("/home/me/notes.md"),
        PathBuf::from("/home/me"),
    ]
    .into_iter()
    .collect();
    assert!(dirs_to_relist(&batch, false).contains(Path::new("/home/me")));

    let batch: HashSet<PathBuf> = [PathBuf::from("/home/me/.claude.json")]
        .into_iter()
        .collect();
    assert!(dirs_to_relist(&batch, true).contains(Path::new("/home/me")));
    assert!(dirs_to_relist(&batch, false).is_empty());
}

#[test]
fn an_unshown_dot_file_does_not_trigger_a_relist() {
    let hidden = Path::new("/home/me/.claude.json");
    let visible = Path::new("/home/me/src");
    assert!(!event_can_change_a_row(hidden, false));
    assert!(event_can_change_a_row(hidden, true));
    assert!(event_can_change_a_row(visible, false));
    assert!(!event_can_change_a_row(
        Path::new("/home/me/.config"),
        false
    ));
    assert!(event_can_change_a_row(Path::new("/home/me/.config"), true));
}

#[test]
fn sort_puts_dirs_first_then_case_insensitive_names() {
    let mut v = vec![
        entry("zeta.rs", false),
        entry("Alpha", true),
        entry("beta", true),
        entry("Apple.rs", false),
    ];
    sort_entries(&mut v);
    let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["Alpha", "beta", "Apple.rs", "zeta.rs"]);
}

fn tree_entry(path: &str, is_dir: bool, ignored: bool) -> TreeEntry {
    let path = PathBuf::from(path);
    TreeEntry {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path,
        is_dir,
        ignored,
    }
}

fn one_repo(paths: &[(&str, DecoStatus)]) -> Decorations {
    let mut index = StatusIndex::default();
    for (path, status) in paths {
        index.insert(path, *status);
    }
    vec![(PathBuf::from("/repo"), Arc::new(index))]
}

#[test]
fn a_row_is_keyed_by_where_it_sits_below_the_repository_root() {
    let root = Path::new("/repo");
    assert!(
        repo_relative(root, Path::new("/repo")).is_none(),
        "the root row has no key of its own"
    );
    assert_eq!(
        repo_relative(root, Path::new("/repo/README.md")).as_deref(),
        Some("README.md")
    );
    assert_eq!(
        repo_relative(root, Path::new("/repo/src/ui/file_tree.rs")).as_deref(),
        Some("src/ui/file_tree.rs")
    );
    assert!(
        repo_relative(root, Path::new("/elsewhere/a.rs")).is_none(),
        "a row outside the repository is not decorated"
    );
    assert!(
        repo_relative(root, Path::new("/repository/a.rs")).is_none(),
        "a shared text prefix is not a shared root"
    );
}

#[test]
fn a_windows_path_is_keyed_with_forward_slashes() {
    assert_eq!(
        with_forward_slashes(r"src\ui\file_tree.rs", '\\'),
        "src/ui/file_tree.rs"
    );
    assert_eq!(with_forward_slashes("README.md", '\\'), "README.md");
    assert_eq!(
        with_forward_slashes("src/ui/file_tree.rs", '/'),
        "src/ui/file_tree.rs"
    );
    // The rows that exist in their thousands must not allocate a key.
    assert!(matches!(
        with_forward_slashes("src/ui/file_tree.rs", '/'),
        Cow::Borrowed(_)
    ));
    assert!(matches!(
        with_forward_slashes("README.md", '\\'),
        Cow::Borrowed(_)
    ));
}

#[test]
fn every_status_gets_its_letter_and_its_own_name_shape() {
    // (status, letter, bold, struck through)
    let cases = [
        (DecoStatus::Conflict, "U", true, false),
        (DecoStatus::Deleted, "D", false, true),
        (DecoStatus::Added, "A", false, false),
        (DecoStatus::Untracked, "?", false, false),
        (DecoStatus::Modified, "M", false, false),
        (DecoStatus::Renamed, "R", false, false),
    ];
    for (status, letter, bold, strike) in cases {
        let deco = RowDeco::file(status);
        assert_eq!(deco.letter, letter, "{status:?}");
        assert_eq!(
            deco.tint,
            Some(status),
            "{status:?} colours the name through the shared table"
        );
        assert_eq!(deco.bold, bold, "{status:?}");
        assert_eq!(deco.strike, strike, "{status:?}");
        assert_eq!(deco.badge(), Some((letter, status)), "{status:?}");
    }

    let ignored = RowDeco::file(DecoStatus::Ignored);
    assert_eq!(ignored.letter, "");
    assert!(
        ignored.badge().is_none(),
        "a tree full of `!` is noise, not information"
    );
}

#[test]
fn a_folder_is_only_ever_changed_or_conflicted_and_never_lettered() {
    assert_eq!(RowDeco::dir(DirRollup::default()), RowDeco::default());

    let changed = RowDeco::dir(DirRollup {
        changed: true,
        conflict: false,
    });
    assert_eq!(
        changed.tint,
        Some(DecoStatus::Modified),
        "the same warning a modified file wears"
    );
    assert_eq!(changed.letter, "");
    assert!(changed.badge().is_none(), "a folder is not `M`");

    let conflict = RowDeco::dir(DirRollup {
        changed: true,
        conflict: true,
    });
    assert_eq!(
        conflict.tint,
        Some(DecoStatus::Conflict),
        "a conflict below outranks a mere change below"
    );
    assert_eq!(conflict.letter, "");
    assert!(
        !conflict.bold,
        "the folder points; the file inside it shouts"
    );
}

#[test]
fn dropping_the_file_map_leaves_the_folders_decorated() {
    let mut index = StatusIndex::default();
    index.insert("src/ui/a.rs", DecoStatus::Modified);
    index.drop_files();
    let decor: Decorations = vec![(PathBuf::from("/repo"), Arc::new(index))];

    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/src/ui/a.rs", false, false)),
        RowDeco::default(),
        "no letter survives the circuit breaker"
    );
    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/src", true, false)).tint,
        Some(DecoStatus::Modified),
        "but the folders still say where the work is"
    );
}

#[test]
fn a_gitignored_row_is_left_with_the_styling_it_already_had() {
    let decor = one_repo(&[("target/debug/app", DecoStatus::Untracked)]);
    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/target/debug/app", false, false)).letter,
        "?",
        "the same row without the ignore flag is decorated"
    );
    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/target/debug/app", false, true)),
        RowDeco::default(),
        "dimmed ink and icon are the whole of what an ignored row says"
    );
    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/target", true, true)),
        RowDeco::default(),
        "and an ignored folder does not get a rollup colour either"
    );
}

#[test]
fn the_innermost_repository_answers_for_its_own_rows() {
    let mut outer = StatusIndex::default();
    outer.insert("vendor/lib/a.rs", DecoStatus::Modified);
    let mut inner = StatusIndex::default();
    inner.insert("a.rs", DecoStatus::Conflict);
    let mut decor: Decorations = vec![
        (PathBuf::from("/repo"), Arc::new(outer)),
        (PathBuf::from("/repo/vendor/lib"), Arc::new(inner)),
    ];
    order_innermost_first(&mut decor);

    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/vendor/lib/a.rs", false, false)).letter,
        "U",
        "the submodule, not the repository holding it"
    );
    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/vendor", true, false)).tint,
        Some(DecoStatus::Modified),
        "the outer repository still rolls its own directories up"
    );
    assert_eq!(
        row_decoration(&decor, &tree_entry("/elsewhere/a.rs", false, false)),
        RowDeco::default()
    );
    assert_eq!(
        row_decoration(&decor, &tree_entry("/repo/README.md", false, false)),
        RowDeco::default(),
        "a clean tracked file is left alone"
    );
}

#[test]
fn shell_quote_leaves_safe_paths_and_quotes_the_rest() {
    assert_eq!(shell_quote_for(Path::new("/a/b.txt"), None), "/a/b.txt");
    assert_eq!(shell_quote_for(Path::new("/a dir/f"), None), "'/a dir/f'");
    // An apostrophe is the one character the dialects disagree about, so
    // the shell has to be named — with none given the answer is the
    // platform's, and this assertion is about the POSIX rule.
    assert_eq!(
        shell_quote_for(Path::new("/a'b"), Some("zsh")),
        r"'/a'\''b'"
    );
}

#[test]
fn shell_quote_for_picks_double_quotes_only_for_cmd() {
    let spaced = Path::new("/a dir/f");
    assert_eq!(shell_quote_for(spaced, Some("cmd.exe")), "\"/a dir/f\"");
    assert_eq!(
        shell_quote_for(spaced, Some("C:\\Windows\\System32\\cmd.exe")),
        "\"/a dir/f\"",
        "a full path to cmd still names cmd"
    );
    assert_eq!(shell_quote_for(spaced, Some("CMD.EXE")), "\"/a dir/f\"");
    assert_eq!(
        shell_quote_for(spaced, Some("powershell.exe")),
        "'/a dir/f'"
    );
    assert_eq!(shell_quote_for(spaced, Some("pwsh")), "'/a dir/f'");
    assert_eq!(shell_quote_for(spaced, Some("/bin/bash")), "'/a dir/f'");
    assert_eq!(
        shell_quote_for(spaced, None),
        "'/a dir/f'",
        "an unknown shell keeps the POSIX form"
    );
    assert_eq!(
        shell_quote_for(Path::new("/plain"), Some("cmd.exe")),
        "/plain",
        "a path that needs no quoting stays bare under cmd too"
    );
}

#[test]
fn search_retarget_spawns_once_per_query_and_older_walks_lose() {
    let mut search = SearchState::default();
    let first = search.retarget("fo", false).expect("a new query walks");
    assert!(
        search.retarget("fo", false).is_none(),
        "a repaint mid-walk must not queue a second one"
    );
    let second = search
        .retarget("foo", false)
        .expect("a changed query walks");
    assert_ne!(first, second);

    assert!(
        !search.accept(first, true, vec![entry("stale.rs", false)]),
        "the overtaken walk's hits are dropped"
    );
    assert!(search.accept(second, true, vec![entry("foo.rs", false)]));
    assert_eq!(search.hits.len(), 1);

    let third = search
        .retarget("foo", true)
        .expect("showing dotfiles re-walks");
    assert_ne!(second, third);
    assert!(search.retarget("foo", true).is_none());

    assert!(search.retarget("", true).is_none());
    assert!(search.hits.is_empty());
    search.retarget("foo", true).expect("typing again walks");
    search.restart();
    assert!(search.retarget("foo", true).is_some(), "restart re-walks");
}

#[test]
fn a_search_that_failed_is_not_a_search_with_no_hits() {
    let mut search = SearchState::default();
    let walk = search.retarget("foo", false).expect("a new query walks");
    assert!(search.accept(walk, false, Vec::new()));
    assert!(
        search.failed,
        "an empty list from a host that refused the walk is not an answer"
    );

    // And it is not carried past the query it belongs to.
    let next = search
        .retarget("food", false)
        .expect("a changed query walks");
    assert!(search.accept(next, true, vec![entry("food.rs", false)]));
    assert!(!search.failed);

    let last = search.retarget("foodie", false).expect("and again");
    assert!(search.accept(last, false, Vec::new()));
    assert!(search.failed);
    search.retarget("", false);
    assert!(!search.failed, "an emptied box has nothing to report");
}

#[test]
fn the_tree_reads_the_same_listing_out_of_the_host() {
    let host = anterm_core::host::local::LocalHost::new();
    let tmp = std::env::temp_dir().join(format!("anterm-tree-host-{}", std::process::id()));
    let _ = host.remove(&tmp, true);
    host.create_dir(&tmp.join(".git"), true).unwrap();
    host.create_dir(&tmp.join("src"), true).unwrap();
    host.write_file(&tmp.join(".gitignore"), b"*.log\nbuild/\n")
        .unwrap();
    host.write_file(&tmp.join("src/.gitignore"), b"!keep.log\n")
        .unwrap();
    host.write_file(&tmp.join("drop.log"), b"").unwrap();
    host.write_file(&tmp.join("src/keep.log"), b"").unwrap();
    host.write_file(&tmp.join("src/main.rs"), b"").unwrap();

    let list = |dir: &Path| -> Vec<TreeEntry> {
        host.read_dir(dir, Some(&tmp))
            .unwrap()
            .into_iter()
            .map(|e| TreeEntry {
                path: host.join(dir, &e.name),
                name: e.name,
                is_dir: e.is_dir,
                ignored: e.ignored,
            })
            .collect()
    };
    let ignored = |entries: &[TreeEntry], name: &str| {
        entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .ignored
    };
    let top = list(&tmp);
    assert!(ignored(&top, "drop.log"));
    assert!(ignored(&top, ".git"));
    assert!(!ignored(&top, "src"));
    assert_eq!(
        top.iter().find(|e| e.name == "src").unwrap().path,
        tmp.join("src"),
        "entries carry a full path, rebuilt with the host's separator"
    );
    let nested = list(&tmp.join("src"));
    assert!(!ignored(&nested, "keep.log"), "whitelist un-ignores");
    assert!(!ignored(&nested, "main.rs"));

    let hits = host
        .search(
            std::slice::from_ref(&tmp),
            "log",
            SEARCH_LIMIT,
            SEARCH_MAX_DIRS,
            false,
        )
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, vec!["keep.log"], "ignored hits stay out of search");

    let hidden = host
        .search(
            std::slice::from_ref(&tmp),
            "log",
            SEARCH_LIMIT,
            SEARCH_MAX_DIRS,
            true,
        )
        .unwrap();
    let mut names: Vec<&str> = hidden.iter().map(|h| h.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["drop.log", "keep.log"]);

    let _ = host.remove(&tmp, true);
}

#[cfg(unix)]
#[test]
fn a_symlink_cycle_cannot_make_the_search_walk_forever() {
    let host = anterm_core::host::local::LocalHost::new();
    let tmp = std::env::temp_dir().join(format!("anterm-tree-loop-{}", std::process::id()));
    let _ = host.remove(&tmp, true);
    host.create_dir(&tmp, true).unwrap();
    host.write_file(&tmp.join("needle.rs"), b"").unwrap();
    host.create_dir(&tmp.join("a"), true).unwrap();
    std::os::unix::fs::symlink(tmp.join("a"), tmp.join("a/loop")).unwrap();

    let hits = host
        .search(
            std::slice::from_ref(&tmp),
            "needle",
            SEARCH_LIMIT,
            SEARCH_MAX_DIRS,
            false,
        )
        .expect("the walk terminates rather than recursing forever");
    assert_eq!(
        hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(),
        vec!["needle.rs"],
        "breadth-first order finds the shallow hit before the cycle deepens"
    );

    let listed = host.read_dir(&tmp.join("a"), Some(&tmp)).unwrap();
    let link = listed.iter().find(|e| e.name == "loop").expect("link");
    assert!(link.is_dir, "a link to a directory expands as one");
    assert!(link.is_symlink);

    let _ = host.remove(&tmp, true);
}

#[test]
fn a_rejected_write_drops_the_row_it_guessed() {
    let host = HostId::LOCAL;
    let dir = PathBuf::from("/x");
    let mut children: ByHost<PathBuf, Vec<TreeEntry>> = ByHost::default();
    let names = |children: &ByHost<PathBuf, Vec<TreeEntry>>| -> Vec<String> {
        children
            .get(host, &dir)
            .map(|v| v.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default()
    };
    let seed = |children: &mut ByHost<PathBuf, Vec<TreeEntry>>| {
        children.insert(host, dir.clone(), vec![entry("b.rs", false)]);
    };

    seed(&mut children);
    let new = entry("a.rs", false);
    let before = optimistic_write(&mut children, host, &dir, &TreeWrite::NewFile, &new);
    assert!(before.is_some());
    assert_eq!(names(&children), vec!["a.rs", "b.rs"]);

    seed(&mut children);
    let renamed = TreeEntry {
        name: "z.rs".into(),
        path: PathBuf::from("/x/z.rs"),
        is_dir: false,
        ignored: false,
    };
    optimistic_write(
        &mut children,
        host,
        &dir,
        &TreeWrite::Rename {
            from: PathBuf::from("/x/b.rs"),
        },
        &renamed,
    );
    assert_eq!(names(&children), vec!["z.rs"]);

    seed(&mut children);
    let doomed = entry("b.rs", false);
    optimistic_write(&mut children, host, &dir, &TreeWrite::Delete, &doomed);
    assert!(names(&children).is_empty());

    seed(&mut children);
    let before = optimistic_write(&mut children, host, &dir, &TreeWrite::NewFile, &new);
    rollback_write(&mut children, host, &dir, before);
    assert!(
        children.get(host, &dir).is_none(),
        "a failed write leaves the directory to relist"
    );

    seed(&mut children);
    let before = optimistic_write(&mut children, host, &dir, &TreeWrite::NewFile, &new);
    children.insert(host, dir.clone(), vec![entry("fresh.rs", false)]);
    rollback_write(&mut children, host, &dir, before);
    assert!(
        children.get(host, &dir).is_none(),
        "the stale snapshot never overwrites a newer listing"
    );

    let other = PathBuf::from("/y");
    let before = optimistic_write(&mut children, host, &other, &TreeWrite::NewFile, &new);
    assert!(before.is_none());
    assert!(children.get(host, &other).is_none());
    rollback_write(&mut children, host, &other, before);
    assert!(children.get(host, &other).is_none());
}

mod view_gpui_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::super::*;
    use crate::panel_search::PanelSearchView;
    use gpui::{TestAppContext, VisualTestContext, size};
    use gpui_kit::component::Root;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anterm-ft-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    struct TreeHarness {
        tree: Entity<FileTreeView>,
        _sub: Subscription,
    }

    impl Render for TreeHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            v_flex().size_full().child(self.tree.clone())
        }
    }

    fn open_standalone(
        cx: &mut TestAppContext,
    ) -> (
        Entity<FileTreeView>,
        Rc<RefCell<Vec<FileTreeEvent>>>,
        VisualTestContext,
    ) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            anterm_ui_kit::config::pin_test_config_dir();
            gpui_kit::init(cx);
            let cfg = anterm_ui_kit::config::Config::default();
            cx.set_global(cfg);
            anterm_ui_kit::theme::apply_theme(None, cx);
        });
        let events: Rc<RefCell<Vec<FileTreeEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let tree_slot: Rc<RefCell<Option<Entity<FileTreeView>>>> = Rc::new(RefCell::new(None));
        let handle = cx.open_window(size(px(320.), px(480.)), {
            let events = events.clone();
            let tree_slot = tree_slot.clone();
            move |window, cx| {
                let harness = cx.new(|cx| {
                    let search_input = cx.new(|cx| InputState::new(window, cx));
                    let panel_search =
                        cx.new(|cx| PanelSearchView::new(search_input.clone(), window, cx));
                    let tree =
                        cx.new(|cx| FileTreeView::new(search_input, panel_search, window, cx));
                    let sub = cx.subscribe(&tree, {
                        let events = events.clone();
                        move |_, _, ev: &FileTreeEvent, _| {
                            events.borrow_mut().push(ev.clone());
                        }
                    });
                    *tree_slot.borrow_mut() = Some(tree.clone());
                    TreeHarness { tree, _sub: sub }
                });
                Root::new(harness, window, cx)
            }
        });
        let vcx = VisualTestContext::from_window(handle.into(), cx);
        let tree = tree_slot.borrow().clone().expect("tree entity");
        (tree, events, vcx)
    }

    fn wait_until(
        vcx: &mut VisualTestContext,
        what: &str,
        mut done: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            vcx.background_executor.run_until_parked();
            if done(vcx) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[gpui::test]
    fn large_directory_is_virtualized_with_ten_thousand_entries(cx: &mut TestAppContext) {
        let _serial = serial();
        let (tree, _events, mut vcx) = open_standalone(cx);
        let root = PathBuf::from("/virtual-repo");
        let entries: Vec<TreeEntry> = (0..10_000)
            .map(|i| TreeEntry {
                name: format!("file_{i:05}.rs"),
                path: root.join(format!("file_{i:05}.rs")),
                is_dir: false,
                ignored: false,
            })
            .collect();

        tree.update(&mut vcx, |ft, cx| {
            ft.ctx.host_id = HostId::LOCAL;
            ft.ctx.on_screen = true;
            ft.active_tab_state_mut().roots = vec![root.clone()];
            ft.children.insert(HostId::LOCAL, root.clone(), entries);
            cx.notify();
        });
        row_probe::reset();
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        let total = tree.read_with(&vcx, |ft, _| {
            let state = ft.active_tab_state().unwrap();
            ft.visible_rows(HostId::LOCAL, &state.roots, &state.expanded)
                .len()
        });
        assert_eq!(total, 10_001, "root + 10,000 child entries");
        let painted = row_probe::last();
        assert!(
            painted > 0 && painted < 80,
            "v_virtual_list must render only viewport rows, got {painted} of {total}"
        );

        let deep = root.join("file_09500.rs");
        tree.update(&mut vcx, |ft, cx| {
            assert!(ft.reveal_path(&deep, cx));
        });
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let offset_y = tree.read_with(&vcx, |ft, _| ft.scroll.offset().y);
        assert!(
            offset_y < px(0.),
            "reveal_path should scroll virtual list down to deep item, got {offset_y:?}"
        );
    }

    #[gpui::test]
    fn keyboard_navigation_expands_collapses_and_emits_open_file(cx: &mut TestAppContext) {
        let _serial = serial();
        let (tree, events, mut vcx) = open_standalone(cx);
        let root = PathBuf::from("/proj");
        let src = root.join("src");
        let main_rs = src.join("main.rs");

        tree.update(&mut vcx, |ft, cx| {
            ft.ctx.host_id = HostId::LOCAL;
            ft.ctx.on_screen = true;
            ft.active_tab_state_mut().roots = vec![root.clone()];
            ft.children.insert(
                HostId::LOCAL,
                root.clone(),
                vec![TreeEntry {
                    name: "src".into(),
                    path: src.clone(),
                    is_dir: true,
                    ignored: false,
                }],
            );
            ft.children.insert(
                HostId::LOCAL,
                src.clone(),
                vec![TreeEntry {
                    name: "main.rs".into(),
                    path: main_rs.clone(),
                    is_dir: false,
                    ignored: false,
                }],
            );
            cx.notify();
        });
        vcx.update(|window, cx| {
            let handle = tree.read(cx).focus_handle(cx);
            handle.focus(window, cx);
            window.draw(cx).clear(cx);
        });

        // Down -> root, Down -> src, Right -> expand src, Down -> main.rs, Enter -> OpenFile
        for key in ["down", "down", "right", "down", "enter"] {
            vcx.simulate_keystrokes(key);
            vcx.run_until_parked();
        }
        assert_eq!(
            events.borrow().as_slice(),
            &[FileTreeEvent::OpenFile(main_rs.clone())]
        );

        // Left from main.rs moves selection to parent `src`; Left again collapses `src`.
        vcx.simulate_keystrokes("left");
        vcx.run_until_parked();
        assert_eq!(
            tree.read_with(&vcx, |ft, _| ft
                .active_tab_state()
                .and_then(|s| s.selected.clone())),
            Some(src.clone())
        );
        vcx.simulate_keystrokes("left");
        vcx.run_until_parked();
        assert!(tree.read_with(&vcx, |ft, _| !ft
            .active_tab_state()
            .unwrap()
            .expanded
            .contains(&src)));
    }

    #[gpui::test]
    fn inline_edit_escape_cancels_and_restores_focus(cx: &mut TestAppContext) {
        let _serial = serial();
        let (tree, _events, mut vcx) = open_standalone(cx);
        let root = PathBuf::from("/proj");
        tree.update_in(&mut vcx, |ft, window, cx| {
            ft.ctx.host_id = HostId::LOCAL;
            ft.ctx.on_screen = true;
            ft.active_tab_state_mut().roots = vec![root.clone()];
            ft.children.insert(HostId::LOCAL, root.clone(), Vec::new());
            ft.begin_edit(TreeEditKind::NewFile, &root, true, window, cx);
        });
        vcx.run_until_parked();
        assert!(tree.read_with(&vcx, |ft, _| ft.editing.is_some()));

        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(tree.read_with(&vcx, |ft, _| ft.editing.is_none()));
        let focused = vcx.update(|window, cx| tree.read(cx).focus_handle.is_focused(window));
        assert!(focused, "Escape from inline edit must hand focus back to tree");
    }

    #[gpui::test]
    fn fs_events_skip_host_loads_when_off_screen_and_relist_when_visible(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("fs-events");
        std::fs::write(root.join("a.rs"), "").unwrap();
        let host: SharedHost = anterm_core::host::local::LocalHost::new();
        let (tree, _events, mut vcx) = open_standalone(cx);

        tree.update(&mut vcx, |ft, cx| {
            ft.sync_context(
                FileTreeContext {
                    host_id: HostId::LOCAL,
                    host: Some(host.clone()),
                    pane_cwds: vec![root.clone()],
                    on_screen: true,
                    ..Default::default()
                },
                cx,
            );
            ft.repo_roots.insert(HostId::LOCAL, root.clone(), root.clone());
            ft.refresh_roots(cx);
            let state = ft.active_tab_state().unwrap();
            let (roots, expanded) = (state.roots.clone(), state.expanded.clone());
            ft.request_loads(&host, &roots, &expanded, cx);
        });
        wait_until(&mut vcx, "initial dir listing never landed", |vcx| {
            tree.read_with(vcx, |ft, _| ft.children.get(HostId::LOCAL, &root).is_some())
        });

        // Off-screen: fs event marks stale without starting any host load.
        let added = root.join("b.rs");
        std::fs::write(&added, "").unwrap();
        tree.update(&mut vcx, |ft, cx| {
            ft.ctx.on_screen = false;
            ft.apply_fs_events(HostId::LOCAL, &HashSet::from([added.clone()]), cx);
        });
        let (loads, stale) = tree.read_with(&vcx, |ft, _| {
            (ft.loads.len(), ft.stale.contains(&(HostId::LOCAL, root.clone())))
        });
        assert_eq!(loads, 0, "off-screen tree must not spawn host reads");
        assert!(stale, "change must still be recorded as stale");

        // On-screen: next render refreshes and picks up `b.rs`.
        tree.update(&mut vcx, |ft, cx| {
            ft.ctx.on_screen = true;
            cx.notify();
        });
        wait_until(&mut vcx, "stale dir was never reloaded on screen", |vcx| {
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            tree.read_with(vcx, |ft, _| {
                ft.children
                    .get(HostId::LOCAL, &root)
                    .is_some_and(|list| list.iter().any(|e| e.name == "b.rs"))
            })
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn dropped_file_copies_in_and_conflict_prompts_before_replacing(cx: &mut TestAppContext) {
        let _serial = serial();
        let root = scratch("drop-dst");
        let from = scratch("drop-src");
        std::fs::write(from.join("note.txt"), "v1").unwrap();
        let host: SharedHost = anterm_core::host::local::LocalHost::new();
        let (tree, _events, mut vcx) = open_standalone(cx);

        tree.update_in(&mut vcx, |ft, window, cx| {
            ft.sync_context(
                FileTreeContext {
                    host_id: HostId::LOCAL,
                    host: Some(host.clone()),
                    on_screen: true,
                    ..Default::default()
                },
                cx,
            );
            ft.drop_paths(vec![from.join("note.txt")], root.clone(), window, cx);
        });
        wait_until(&mut vcx, "initial drop copy never landed", |_| {
            root.join("note.txt").exists()
        });
        assert_eq!(std::fs::read_to_string(root.join("note.txt")).unwrap(), "v1");

        // Conflict: Cancel leaves v1 untouched.
        std::fs::write(from.join("note.txt"), "v2").unwrap();
        tree.update_in(&mut vcx, |ft, window, cx| {
            ft.drop_paths(vec![from.join("note.txt")], root.clone(), window, cx);
        });
        wait_until(&mut vcx, "conflict prompt did not appear", |vcx| {
            vcx.has_pending_prompt()
        });
        vcx.simulate_prompt_answer(t(L10nKey::Cancel));
        vcx.background_executor.run_until_parked();
        assert_eq!(std::fs::read_to_string(root.join("note.txt")).unwrap(), "v1");

        // Conflict: Replace overwrites with v2.
        tree.update_in(&mut vcx, |ft, window, cx| {
            ft.drop_paths(vec![from.join("note.txt")], root.clone(), window, cx);
        });
        wait_until(&mut vcx, "second conflict prompt did not appear", |vcx| {
            vcx.has_pending_prompt()
        });
        vcx.simulate_prompt_answer(t(L10nKey::FileDropReplace));
        wait_until(&mut vcx, "replacement never landed", |_| {
            std::fs::read_to_string(root.join("note.txt")).is_ok_and(|s| s == "v2")
        });

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&from);
    }
}
