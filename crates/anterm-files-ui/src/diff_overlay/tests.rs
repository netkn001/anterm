use super::body::{oversized_summary, short_rev, spliced_range, unified_marker};
use super::*;
use anterm_core::git::status::DecoStatus;
use anterm_ui_kit::git::diff::{
    self as git_diff, AUTO_COLLAPSE_LINES, CommitLabel, DiffLine, FileStatus, LineKind,
    MAX_RENDERED_FILES, Truncation,
};
use crate::diff_list::{build_rows, file_expanded};
use crate::scm::status::status_glyph;
use anterm_editor::diff::split_hunk;
use anterm_i18n::set_locale;

#[test]
fn full_window_diff_background_is_opaque_with_or_without_a_preset() {
    let active = anterm_ui_kit::presets::ActiveBackground {
        fill: anterm_ui_kit::presets::Fill::Solid(0x12_34_56),
        opacity: Some(0.2),
        image: None,
    };
    let mut fallback: Hsla = gpui::rgb(0x65_43_21).into();
    fallback.a = 0.3;
    let mut opaque_fallback = fallback;
    opaque_fallback.a = 1.0;

    assert_eq!(
        diff_overlay_background(Some(&active), fallback),
        anterm_ui_kit::theme::window_background_opaque(&active),
        "the active preset must keep its fill while discarding workspace translucency"
    );
    assert_eq!(
        diff_overlay_background(None, fallback),
        opaque_fallback.into(),
        "the theme fallback must also block the window material"
    );
}

fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old_no: old,
        new_no: new,
        text: text.to_string(),
    }
}

#[test]
fn the_probe_key_separates_the_sources_over_one_directory() {
    let host = anterm_ui_kit::host_ops::HostId::LOCAL;
    let cwd = Path::new("/repo");
    let worktree = probe_key(host, cwd, &DiffSource::Worktree);
    assert_ne!(worktree, probe_key(host, cwd, &DiffSource::Staged));
    assert_ne!(worktree, probe_key(host, cwd, &DiffSource::Head));
    assert_ne!(
        probe_key(host, cwd, &DiffSource::commit("a")),
        probe_key(host, cwd, &DiffSource::commit("b")),
        "two commits are two probes"
    );
    assert_eq!(worktree, probe_key(host, cwd, &DiffSource::Worktree));
    assert_ne!(
        worktree,
        probe_key(host, Path::new("/other"), &DiffSource::Worktree)
    );
    assert_eq!(
        probe_key(host, cwd, &DiffSource::commit("a")),
        probe_key(
            host,
            cwd,
            &DiffSource::Commit {
                rev: "a".into(),
                label: Some(CommitLabel {
                    subject: "s".into(),
                    author: "Ada".into(),
                    at: 1,
                }),
            }
        )
    );
}

#[test]
fn every_file_status_lands_on_a_shared_decoration() {
    use DecoStatus as D;
    for (status, want) in [
        (FileStatus::Added, D::Added),
        (FileStatus::Modified, D::Modified),
        (FileStatus::Deleted, D::Deleted),
        (FileStatus::Renamed, D::Renamed),
        (FileStatus::Copied, D::Renamed),
        (FileStatus::TypeChanged, D::Modified),
        (FileStatus::Unmerged, D::Conflict),
    ] {
        assert_eq!(deco_status(status), want, "{status:?}");
    }
    assert_eq!(status_glyph(deco_status(FileStatus::Unmerged)), "U");
    assert_eq!(status_glyph(deco_status(FileStatus::Copied)), "R");
}

#[test]
fn the_change_column_uses_the_typographic_minus() {
    assert_eq!(unified_marker(LineKind::Added), "+");
    assert_eq!(unified_marker(LineKind::Removed), "\u{2212}");
    assert_ne!(
        unified_marker(LineKind::Removed),
        "-",
        "the ASCII hyphen is narrower than `+`, and the column would wobble"
    );
    assert_eq!(
        unified_marker(LineKind::Context),
        "",
        "a context line is neither, and a placeholder glyph would be noise"
    );
}

#[test]
fn the_header_shortens_an_object_id_and_nothing_else() {
    let oid = "3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a";
    assert_eq!(short_rev(oid), "3f2a1b9c");
    assert_eq!(short_rev("v26.7.5"), "v26.7.5");
    assert_eq!(
        short_rev("origin/main"),
        "origin/main",
        "half a ref name says less than the whole of it"
    );
    assert_eq!(short_rev("3f2a1b9"), "3f2a1b9", "already short");
}

#[test]
fn each_source_names_itself_in_the_header() {
    let branch = || "main".to_string();
    let plain = source_subject(&DiffSource::Worktree, branch());
    assert_eq!((plain.icon, plain.text.as_str()), (BRANCH_ICON, "main"));
    assert_eq!(plain.chip, None);
    assert!(!plain.is_rev);

    assert_eq!(source_subject(&DiffSource::Head, branch()).chip, None);

    let staged = source_subject(&DiffSource::Staged, branch());
    assert_eq!(staged.icon, BRANCH_ICON, "still a branch, still its name");
    assert_eq!(
        staged.chip,
        Some("STAGED"),
        "without it the staged patch is indistinguishable from the unstaged one"
    );

    let commit = source_subject(
        &DiffSource::commit("3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a"),
        branch(),
    );
    assert_eq!(commit.icon, COMMIT_ICON);
    assert_eq!(commit.text, "3f2a1b9c", "the branch is not what is shown");
    assert!(commit.is_rev);

    let range = source_subject(
        &DiffSource::Range {
            base: "main".into(),
            head: "feature".into(),
        },
        branch(),
    );
    assert_eq!(range.icon, COMMIT_ICON);
    assert_eq!(range.text, "main…feature");
}

#[test]
fn a_labelled_commit_says_what_it_was_about() {
    let label = CommitLabel {
        subject: "fix(scm): stop the panel asking twice".into(),
        author: "Ada".into(),
        at: 1_786_255_391,
    };
    let with = source_subject(
        &DiffSource::Commit {
            rev: "3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a".into(),
            label: Some(label.clone()),
        },
        "main".to_string(),
    );
    assert_eq!(with.text, "3f2a1b9c", "the sha is still the identifier");
    assert_eq!(
        with.label.as_ref().map(|l| l.subject.as_str()),
        Some(label.subject.as_str())
    );

    assert!(
        source_subject(&DiffSource::Worktree, "main".into())
            .label
            .is_none()
    );
    assert!(
        source_subject(&DiffSource::Head, "main".into())
            .label
            .is_none()
    );
    assert!(
        source_subject(&DiffSource::commit("deadbeef"), "main".into())
            .label
            .is_none(),
        "a commit nobody has read yet has nothing to say"
    );
    let empty = source_subject(
        &DiffSource::Commit {
            rev: "deadbeef".into(),
            label: Some(CommitLabel::default()),
        },
        "main".into(),
    );
    assert!(empty.label.is_none());
}

#[test]
fn the_byline_drops_the_separator_along_with_the_half_it_joined() {
    let now = 1_786_255_391 + 7200;
    let full = CommitLabel {
        subject: "s".into(),
        author: "Ada".into(),
        at: 1_786_255_391,
    };
    assert_eq!(label_byline(&full, now), "Ada · 2h");
    assert_eq!(
        label_byline(
            &CommitLabel {
                author: String::new(),
                ..full.clone()
            },
            now
        ),
        "2h",
        "a commit with no author is not `· 2h`"
    );
    assert_eq!(
        label_byline(&CommitLabel { at: 0, ..full }, now),
        "Ada",
        "and a timestamp that would not parse is not `Ada · 56y`"
    );
}

const BRANCH_ICON: &str = "icons/git-branch.svg";
const COMMIT_ICON: &str = "icons/git-commit.svg";

fn small_file(path: &str, added: u32) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        old_path: None,
        status: FileStatus::Modified,
        added,
        removed: 0,
        binary: false,
        truncated: None,
        hunks: vec![git_diff::Hunk {
            header: "@@ -1,1 +1,1 @@".to_string(),
            lines: (0..added)
                .map(|i| line(LineKind::Added, None, Some(i + 1), "x"))
                .collect(),
        }],
    }
}

fn context_heavy_file(path: &str) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        old_path: None,
        status: FileStatus::Modified,
        added: 1,
        removed: 0,
        binary: false,
        truncated: None,
        hunks: vec![git_diff::Hunk {
            header: "@@ -1,7 +1,7 @@".to_string(),
            lines: (0..6)
                .map(|i| line(LineKind::Context, Some(i + 1), Some(i + 1), "ctx"))
                .chain(std::iter::once(line(LineKind::Added, None, Some(7), "x")))
                .collect(),
        }],
    }
}

fn choices<const N: usize>(pairs: [(&str, bool); N]) -> HashMap<String, bool> {
    pairs.into_iter().map(|(p, v)| (p.to_string(), v)).collect()
}

fn banner(snap: &DiffSnapshot) -> String {
    set_locale("en");
    oversized_summary(snap, &snap.stats())
}

#[test]
fn per_file_collapse_is_unchanged_below_the_repo_threshold() {
    let small = small_file("small.rs", 10);
    let big = small_file("big.rs", AUTO_COLLAPSE_LINES + 1);
    let none = HashMap::new();
    assert!(file_expanded(&small, &none, false));
    assert!(!file_expanded(&big, &none, false));

    let picked = choices([("small.rs", false), ("big.rs", true)]);
    assert!(!file_expanded(&small, &picked, false));
    assert!(file_expanded(&big, &picked, false));
}

#[test]
fn repo_wide_collapse_overrides_the_per_file_default() {
    let small = small_file("small.rs", 10);
    let none = HashMap::new();
    assert!(file_expanded(&small, &none, false));
    assert!(!file_expanded(&small, &none, true), "collapsed en masse");

    assert!(
        file_expanded(&small, &choices([("small.rs", true)]), true),
        "the user's own click still opens it"
    );
}

#[test]
fn explicit_choices_survive_an_oversized_transition() {
    let opened = small_file("opened.rs", 10);
    let closed = small_file("closed.rs", 10);
    let untouched = small_file("untouched.rs", 10);
    let picked = choices([("opened.rs", true), ("closed.rs", false)]);

    for collapse_all in [true, false] {
        assert!(
            file_expanded(&opened, &picked, collapse_all),
            "an explicitly opened file stays open (collapse_all={collapse_all})"
        );
        assert!(
            !file_expanded(&closed, &picked, collapse_all),
            "an explicitly closed file stays closed (collapse_all={collapse_all})"
        );
    }
    assert!(!file_expanded(&untouched, &picked, true));
    assert!(file_expanded(&untouched, &picked, false));
}

#[test]
fn many_medium_files_are_oversized_and_build_no_rows() {
    let snap = DiffSnapshot {
        files: (0..60)
            .map(|i| small_file(&format!("f{i}.rs"), 150))
            .collect(),
        ..Default::default()
    };
    assert!(
        snap.files.iter().all(|f| f.added <= AUTO_COLLAPSE_LINES),
        "no single file is over the per-file threshold"
    );
    assert!(snap.stats().oversized);

    let none = HashMap::new();
    let rows_expanded: usize = snap
        .files
        .iter()
        .filter(|f| file_expanded(f, &none, false))
        .flat_map(|f| &f.hunks)
        .map(|h| split_hunk(&h.lines).len())
        .sum();
    let rows_collapsed: usize = snap
        .files
        .iter()
        .filter(|f| file_expanded(f, &none, true))
        .flat_map(|f| &f.hunks)
        .map(|h| split_hunk(&h.lines).len())
        .sum();
    assert_eq!(rows_expanded, 9000, "what the old rule would have built");
    assert_eq!(rows_collapsed, 0);
}

#[test]
fn an_ordinary_busy_tree_is_not_oversized() {
    let snap = DiffSnapshot {
        files: (0..40)
            .map(|i| {
                let mut f = context_heavy_file(&format!("f{i}.rs"));
                f.hunks = std::iter::repeat_n(f.hunks[0].clone(), 10).collect();
                f.added = 10;
                f
            })
            .collect(),
        ..Default::default()
    };
    let (added, removed) = snap.totals();
    assert!(
        snap.stats().retained_lines > (added + removed) as usize * 4,
        "the context lines dominate, as they do in a real diff"
    );
    assert!(
        !snap.stats().oversized,
        "an ordinary afternoon must not read as a tree too large to render ({} retained lines)",
        snap.stats().retained_lines
    );
    let none = HashMap::new();
    assert!(snap.files.iter().all(|f| file_expanded(f, &none, false)));
}

#[test]
fn an_empty_snapshot_reads_as_clean_only_when_the_probe_worked() {
    let clean = DiffSnapshot {
        branch: "main".into(),
        ..Default::default()
    };
    assert!(empty_snapshot(&clean));
    assert!(!clean.read_failed, "nothing went wrong: this tree is clean");

    let broken = DiffSnapshot {
        branch: "main".into(),
        read_failed: true,
        ..Default::default()
    };
    assert!(
        empty_snapshot(&broken),
        "indistinguishable by shape — which is the point"
    );
    assert!(broken.read_failed, "and distinguishable by this");

    let partial = DiffSnapshot {
        files: vec![small_file("one.rs", 3)],
        read_failed: true,
        ..Default::default()
    };
    assert!(!empty_snapshot(&partial));
}

#[test]
fn a_huge_untracked_list_does_not_collapse_the_diff() {
    let snap = DiffSnapshot {
        files: vec![small_file("one.rs", 3)],
        untracked: (0..git_diff::MAX_UNTRACKED)
            .map(|i| format!("node_modules/p{i}/index.js"))
            .collect(),
        untracked_total: 40_000,
        ..Default::default()
    };
    assert!(snap.stats().retained_lines < git_diff::AUTO_COLLAPSE_TOTAL_LINES);
    assert!(snap.files.len() < git_diff::AUTO_COLLAPSE_TOTAL_FILES);
    assert!(
        !snap.stats().oversized,
        "collapsing the diff would not have removed a single untracked row"
    );
    assert_eq!(snap.untracked.len(), git_diff::MAX_UNTRACKED);
    assert_eq!(
        snap.untracked.len().min(MAX_RENDERED_FILES),
        MAX_RENDERED_FILES
    );
    assert_eq!(snap.untracked_count(), 40_000);
}

#[test]
fn only_the_rows_that_changed_are_spliced() {
    let snap = DiffSnapshot {
        files: vec![
            small_file("a.rs", 2),
            small_file("b.rs", 2),
            small_file("c.rs", 2),
        ],
        ..Default::default()
    };
    let shut = |path: &str| -> HashMap<String, bool> {
        [(path.to_string(), false)].into_iter().collect()
    };
    let open = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false);
    let middle_shut = build_rows(&snap, &shut("b.rs"), None, DiffViewMode::Unified, false);

    let (replaced, with) = spliced_range(&open, &middle_shut);
    assert_eq!((replaced.start, with), (5, 1));
    assert_eq!(
        open.len() - replaced.end,
        middle_shut.len() - (replaced.start + with)
    );

    let (replaced, with) = spliced_range(&open, &open);
    assert_eq!(
        (replaced.start, replaced.end, with),
        (open.len(), open.len(), 0)
    );
}

#[test]
fn a_list_that_was_empty_or_becomes_empty_splices_in_one_piece() {
    let snap = DiffSnapshot {
        files: vec![small_file("a.rs", 2)],
        ..Default::default()
    };
    let rows = build_rows(&snap, &HashMap::new(), None, DiffViewMode::Unified, false);

    let (replaced, with) = spliced_range(&[], &rows);
    assert_eq!((replaced.start, replaced.end, with), (0, 0, rows.len()));

    let (replaced, with) = spliced_range(&rows, &[]);
    assert_eq!((replaced.start, replaced.end, with), (0, rows.len(), 0));
}

#[test]
fn an_equal_snapshot_leaves_the_key_pointing_at_the_one_that_landed() {
    let held = Arc::new(DiffSnapshot {
        files: vec![small_file("a.rs", 2)],
        ..Default::default()
    });
    let landed = Arc::new(DiffSnapshot {
        files: vec![small_file("a.rs", 2)],
        ..Default::default()
    });
    assert!(!Arc::ptr_eq(&held, &landed));

    let expanded = HashMap::new();
    let mut key = RowsFrom {
        snap: &held,
        preview: None,
        mode: DiffViewMode::Unified,
        focused: None,
        oversized: false,
        expanded: &expanded,
    }
    .to_key();
    let landed_from = RowsFrom {
        snap: &landed,
        preview: None,
        mode: DiffViewMode::Unified,
        focused: None,
        oversized: false,
        expanded: &expanded,
    };

    assert!(key.describes(&landed_from));
    key.retarget(&landed_from);
    assert!(Arc::ptr_eq(&key.snap, &landed));
}

#[test]
fn a_focused_untracked_file_asks_for_a_preview_not_the_list() {
    let snap = DiffSnapshot {
        files: vec![small_file("tracked.rs", 3)],
        untracked: vec!["new.md".to_string()],
        ..Default::default()
    };
    assert_eq!(untracked_focus(&snap, Some("new.md")), Some("new.md"));
    assert_eq!(untracked_focus(&snap, Some("tracked.rs")), None);
    assert_eq!(untracked_focus(&snap, Some("absent.rs")), None);
    assert_eq!(untracked_focus(&snap, None), None);
}

#[test]
fn untracked_rows_are_capped_but_the_count_stays_true() {
    let snap = DiffSnapshot {
        untracked: (0..git_diff::MAX_UNTRACKED)
            .map(|i| format!("p{i}"))
            .collect(),
        untracked_total: 12_345,
        ..Default::default()
    };
    let rendered = snap.untracked.len().min(MAX_RENDERED_FILES);
    assert_eq!(rendered, MAX_RENDERED_FILES);
    assert_eq!(snap.untracked_count(), 12_345);
    assert_eq!(snap.untracked_count() - rendered, 12_045);
}

#[test]
fn untracked_count_falls_back_to_the_retained_length() {
    let snap = DiffSnapshot {
        untracked: vec!["a".into(), "b".into(), "c".into()],
        ..Default::default()
    };
    assert_eq!(snap.untracked_total, 0);
    assert_eq!(snap.untracked_count(), 3);
}

#[test]
fn file_cards_are_capped() {
    let snap = DiffSnapshot {
        files: (0..MAX_RENDERED_FILES + 25)
            .map(|i| small_file(&format!("f{i}.rs"), 1))
            .collect(),
        ..Default::default()
    };
    let shown = snap.files.len().min(MAX_RENDERED_FILES);
    assert_eq!(shown, MAX_RENDERED_FILES);
    assert_eq!(snap.files.len() - shown, 25);
}

#[test]
fn the_banner_names_the_budget_when_context_outweighs_the_changes() {
    let files: Vec<FileDiff> = (0..200)
        .map(|i| context_heavy_file(&format!("f{i}.rs")))
        .collect();

    let mut truncated = files.clone();
    truncated.push(FileDiff {
        hunks: vec![],
        truncated: Some(Truncation::Budget),
        ..context_heavy_file("dropped.rs")
    });
    let snap = DiffSnapshot {
        files: truncated,
        ..Default::default()
    };
    let (added, removed) = snap.totals();
    let total = (added + removed) as usize;
    assert!(snap.stats().retained_lines > total);
    assert!(snap.stats().budget_exhausted);

    let summary = banner(&snap);
    assert!(summary.contains("budget"), "{summary}");
    assert!(summary.contains(&format!("{total} changed lines")), "{summary}");

    let whole = DiffSnapshot {
        files,
        ..Default::default()
    };
    assert!(!whole.stats().budget_exhausted);
    assert!(!banner(&whole).contains("budget"));
}

#[test]
fn the_banner_names_the_per_file_cap_when_context_outweighs_the_changes() {
    let files: Vec<FileDiff> = (0..200)
        .map(|i| context_heavy_file(&format!("f{i}.rs")))
        .collect();

    let mut cut = files.clone();
    cut.push(FileDiff {
        truncated: Some(Truncation::PerFile),
        ..context_heavy_file("huge.rs")
    });
    let snap = DiffSnapshot {
        files: cut,
        ..Default::default()
    };
    let (added, removed) = snap.totals();
    let total = (added + removed) as usize;
    assert!(snap.stats().retained_lines > total);
    assert!(!snap.stats().budget_exhausted);

    let summary = banner(&snap);
    assert!(summary.contains("per-file cap"), "{summary}");
    assert!(summary.contains(&format!("{total} changed lines")), "{summary}");

    let whole = DiffSnapshot {
        files: files.clone(),
        ..Default::default()
    };
    assert!(!banner(&whole).contains("per-file"));

    let mut both = files;
    both.push(FileDiff {
        truncated: Some(Truncation::PerFile),
        ..context_heavy_file("huge.rs")
    });
    both.push(FileDiff {
        hunks: vec![],
        truncated: Some(Truncation::Budget),
        ..context_heavy_file("dropped.rs")
    });
    let summary = banner(&DiffSnapshot {
        files: both,
        ..Default::default()
    });
    assert!(summary.contains("budget"), "{summary}");
    assert!(summary.contains("per-file cap"), "{summary}");
}

#[test]
#[ignore = "measurement, not an assertion"]
fn bench_snapshot_share() {
    use std::time::Instant;

    for (label, files, per_file) in [
        ("unbudgeted (v26.7.5 shape)", 300, 300),
        ("budgeted (this build retains)", 300, 67),
    ] {
        let snap = Arc::new(DiffSnapshot {
            files: (0..files)
                .map(|i| small_file(&format!("f{i}.rs"), per_file))
                .collect(),
            ..Default::default()
        });
        let lines: usize = snap.stats().retained_lines;

        let t = Instant::now();
        for _ in 0..10 {
            let _deep = (*snap).clone();
        }
        let deep = t.elapsed() / 10;

        let t = Instant::now();
        for _ in 0..100 {
            let _shared = Arc::clone(&snap);
        }
        let shared = t.elapsed() / 100;
        println!(
            "{label}: {files} files / {lines} lines — deep clone {deep:?} vs \
             Arc::clone {shared:?}, per holder on the UI thread"
        );
    }
}

mod view_tests {
    use super::*;
    use anterm_ui_kit::config::Config;
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn mount_overlay(
        cx: &mut TestAppContext,
        source: DiffSource,
        snap: Arc<DiffSnapshot>,
    ) -> (
        Entity<DiffOverlayView>,
        Rc<RefCell<Vec<DiffOverlayEvent>>>,
        VisualTestContext,
    ) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::presets::load_registry(cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
            // Pre-mark the probe key inflight so `DiffOverlayView::new` does
            // not spawn an external git process in unit tests.
            let key = probe_key(HostId::LOCAL, Path::new("/repo"), &source);
            cx.default_global::<DiffProbeTracker>().inflight.insert(key);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let events_clone = events.clone();
        let mut slot = None;
        let window = cx.add_window(|window, cx| {
            let overlay = cx.new(|cx| {
                let mut v =
                    DiffOverlayView::new(HostId::LOCAL, PathBuf::from("/repo"), source, None, cx);
                v.loading = false;
                v.load = DiffLoad::Ready(snap);
                v
            });
            cx.subscribe(&overlay, move |_, _, ev: &DiffOverlayEvent, _| {
                events_clone.borrow_mut().push(ev.clone());
            })
            .detach();
            slot = Some(overlay.clone());
            gpui_kit::component::Root::new(overlay, window, cx)
        });
        cx.update(|cx| {
            cx.default_global::<DiffProbeTracker>().restale.clear();
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (slot.unwrap(), events, vcx)
    }

    #[gpui::test]
    fn large_patch_is_virtualized_and_selection_copies_to_clipboard(cx: &mut TestAppContext) {
        let mut file = FileDiff {
            path: "src/lib.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            added: 400,
            removed: 0,
            binary: false,
            truncated: None,
            hunks: vec![git_diff::Hunk {
                header: "@@ -1,0 +1,400 @@".into(),
                lines: (0..400)
                    .map(|i| line(LineKind::Added, None, Some(i + 1), &format!("line {i}")))
                    .collect(),
            }],
        };
        file.added = 400;
        let snap = Arc::new(DiffSnapshot {
            root: PathBuf::from("/repo"),
            branch: "main".into(),
            files: vec![file],
            ..Default::default()
        });
        let (overlay, _events, mut vcx) = mount_overlay(cx, DiffSource::Head, snap);

        // Expand the large file explicitly so all 400 lines become virtual rows.
        overlay.update(&mut vcx, |o, cx| {
            o.set_expanded("src/lib.rs".into(), true, cx);
        });
        #[cfg(not(target_os = "windows"))]
        {
            let _ = row_probe::take();
            vcx.update(|window, _| window.refresh());
            vcx.run_until_parked();
            let built = row_probe::take();
            assert!(built > 0, "virtual list should build visible rows");
            assert!(
                built < 120,
                "400-line diff must be virtualized (built {built} rows)"
            );
        }

        // Select lines 0..=2 and copy to clipboard.
        let at_start = RowAt {
            path: SharedString::from("src/lib.rs"),
            id: anterm_editor::diff::RowId { hunk: 0, row: 0 },
        };
        let at_end = RowAt {
            path: SharedString::from("src/lib.rs"),
            id: anterm_editor::diff::RowId { hunk: 0, row: 2 },
        };
        overlay.update_in(&mut vcx, |o, window, cx| {
            o.start_selection(&at_start, DiffViewMode::Unified, None, window, cx);
            o.extend_selection(&at_end, None, Some(MouseButton::Left), cx);
            o.end_selection(cx);
            o.copy_selection(cx);
        });
        let copied = vcx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(copied.as_deref(), Some("line 0\nline 1\nline 2"));
    }

    #[gpui::test]
    fn helix_keys_emit_open_file_and_close_events(cx: &mut TestAppContext) {
        let snap = Arc::new(DiffSnapshot {
            root: PathBuf::from("/repo"),
            branch: "main".into(),
            files: vec![small_file("src/main.rs", 2)],
            ..Default::default()
        });
        let (overlay, events, mut vcx) = mount_overlay(cx, DiffSource::Worktree, snap);
        overlay.update(&mut vcx, |o, cx| {
            let enter = KeyDownEvent {
                keystroke: gpui::Keystroke::parse("enter").unwrap(),
                is_held: false,
                prefer_character_input: false,
            };
            assert!(o.handle_helix_key(&enter, cx));
            let quit = KeyDownEvent {
                keystroke: gpui::Keystroke::parse("q").unwrap(),
                is_held: false,
                prefer_character_input: false,
            };
            assert!(o.handle_helix_key(&quit, cx));
        });
        vcx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            vec![
                DiffOverlayEvent::OpenFile {
                    path: PathBuf::from("/repo/src/main.rs")
                },
                DiffOverlayEvent::Close,
            ]
        );
    }

    #[gpui::test]
    fn probe_tracker_delivers_snapshot_to_matching_overlay_listeners(cx: &mut TestAppContext) {
        let initial = Arc::new(DiffSnapshot {
            root: PathBuf::from("/repo"),
            branch: "old".into(),
            ..Default::default()
        });
        let (overlay, _events, mut vcx) = mount_overlay(cx, DiffSource::Head, initial);
        let fresh = Arc::new(DiffSnapshot {
            root: PathBuf::from("/repo"),
            branch: "new-branch".into(),
            files: vec![small_file("a.rs", 1)],
            ..Default::default()
        });
        vcx.update(|_, cx| {
            let host = anterm_ui_kit::host_registry::HostRegistry::local(cx);
            let key = probe_key(HostId::LOCAL, Path::new("/repo"), &DiffSource::Head);
            DiffProbeTracker::land(
                host,
                key,
                PathBuf::from("/repo"),
                DiffSource::Head,
                Some(fresh),
                cx,
            );
        });
        vcx.run_until_parked();
        let branch = overlay.read_with(&vcx, |o, _| match &o.load {
            DiffLoad::Ready(s) => s.branch.clone(),
            _ => String::new(),
        });
        assert_eq!(branch, "new-branch");
    }
}
