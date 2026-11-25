use std::collections::HashSet;
use std::path::PathBuf;

use super::*;

#[test]
fn a_fresh_open_starts_at_the_shell_directory() {
    let shell = Some("/srv/app".to_string());
    let last = Some("/var/log".to_string());
    assert_eq!(
        sftp_start_dir(true, shell.clone(), last.clone()).as_deref(),
        Some("/srv/app")
    );
    assert_eq!(
        sftp_start_dir(true, None, last.clone()).as_deref(),
        Some("/var/log"),
        "no shell cwd (no shell integration) falls back to the last browsed"
    );
    assert_eq!(sftp_start_dir(true, None, None), None);
}

#[test]
fn a_pane_switch_returns_to_the_directory_browsed_there() {
    let shell = Some("/srv/app".to_string());
    let last = Some("/var/log".to_string());
    assert_eq!(
        sftp_start_dir(false, shell.clone(), last).as_deref(),
        Some("/var/log")
    );
    assert_eq!(
        sftp_start_dir(false, shell, None).as_deref(),
        Some("/srv/app"),
        "a pane never browsed starts at its shell directory"
    );
}

fn upload(job_id: u64, state: SftpJobState) -> SftpJobProgress {
    SftpJobProgress {
        job_id,
        pane_id: 1,
        kind: SftpTransferKind::Upload,
        state,
        current: String::new(),
        bytes_done: 0,
        bytes_total: 0,
        error: None,
        local: "/here/note.txt".into(),
        remote: "/there/note.txt".into(),
    }
}

#[test]
fn an_upload_owes_a_listing_until_it_stops_running() {
    let owed = HashSet::from([7]);
    let running = uploads_still_running(&owed, &[upload(7, SftpJobState::Running)]);
    assert_eq!(
        running, owed,
        "a listing taken now shows the temporary name"
    );

    for done in [
        SftpJobState::Done,
        SftpJobState::Error,
        SftpJobState::Cancelled,
    ] {
        let running = uploads_still_running(&owed, &[upload(7, done)]);
        assert!(running.is_empty(), "{done:?} still owes the listing");
    }
}

#[test]
fn a_job_that_falls_off_the_list_is_not_waited_on_forever() {
    let owed = HashSet::from([7]);
    assert!(uploads_still_running(&owed, &[]).is_empty());
}

#[test]
fn one_upload_finishing_does_not_settle_the_one_beside_it() {
    let owed = HashSet::from([7, 8]);
    let running = uploads_still_running(
        &owed,
        &[
            upload(7, SftpJobState::Done),
            upload(8, SftpJobState::Running),
        ],
    );
    assert_eq!(running, HashSet::from([8]));
}

#[test]
fn a_failed_poll_keeps_the_transfers_it_cannot_see() {
    let previous = vec![upload(7, SftpJobState::Running)];
    let (jobs, failure) = apply_poll(previous.clone(), Err("broken pipe".into()));
    assert_eq!(jobs.len(), 1, "the last list anyone saw is still the truth");
    assert_eq!(jobs[0].job_id, 7);
    assert_eq!(failure.as_deref(), Some("broken pipe"));

    assert_eq!(
        uploads_still_running(&HashSet::from([7]), &jobs),
        HashSet::from([7])
    );
}

#[test]
fn a_poll_that_got_through_replaces_the_list_and_clears_the_failure() {
    let previous = vec![upload(7, SftpJobState::Running)];
    let (jobs, failure) = apply_poll(previous, Ok(vec![upload(8, SftpJobState::Done)]));
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].job_id, 8);
    assert!(failure.is_none());

    let (jobs, failure) = apply_poll(jobs, Ok(Vec::new()));
    assert!(jobs.is_empty());
    assert!(failure.is_none());
}

#[test]
fn a_second_download_is_numbered_rather_than_written_over_the_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    let free = HashSet::new();

    assert_eq!(
        free_local_path(d, "notes.txt", &free),
        Some(d.join("notes.txt"))
    );

    std::fs::write(d.join("notes.txt"), b"first").expect("write");
    assert_eq!(
        free_local_path(d, "notes.txt", &free),
        Some(d.join("notes (2).txt"))
    );
    std::fs::write(d.join("notes (2).txt"), b"second").expect("write");
    assert_eq!(
        free_local_path(d, "notes.txt", &free),
        Some(d.join("notes (3).txt"))
    );

    std::fs::write(d.join("Makefile"), b"x").expect("write");
    assert_eq!(
        free_local_path(d, "Makefile", &free),
        Some(d.join("Makefile (2)"))
    );
    std::fs::write(d.join(".env"), b"x").expect("write");
    assert_eq!(free_local_path(d, ".env", &free), Some(d.join(".env (2)")));

    assert_eq!(
        std::fs::read(d.join("notes.txt")).expect("read"),
        b"first".to_vec()
    );
}

#[test]
fn a_download_still_in_flight_holds_its_name_before_the_file_exists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();

    let claimed: HashSet<PathBuf> = [d.join("notes.txt")].into_iter().collect();
    assert_eq!(
        free_local_path(d, "notes.txt", &claimed),
        Some(d.join("notes (2).txt"))
    );
}

#[test]
fn every_name_taken_reports_rather_than_overwriting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    claimed.insert(d.join("notes.txt"));
    for n in 2..1000u32 {
        claimed.insert(d.join(format!("notes ({n}).txt")));
    }
    assert_eq!(free_local_path(d, "notes.txt", &claimed), None);
}

#[test]
fn the_overwrite_question_counts_correctly_in_every_locale() {
    use anterm_i18n::{L10nKey, t_plural};
    for locale in ["en", "zh-CN"] {
        anterm_i18n::set_locale(locale);
        for n in [1usize, 2, 7] {
            let body = t_plural(L10nKey::SftpReplaceBody, n, &[("names", "a.txt")]);
            assert!(body.contains("a.txt"), "{locale}/{n}: {body}");
            assert!(!body.contains("{names}"), "{locale}/{n}: {body}");
        }
    }
    anterm_i18n::set_locale("en");
}

fn entry(name: &str, kind: SftpEntryKind, target_is_dir: bool) -> SftpEntry {
    SftpEntry {
        name: name.to_string(),
        kind,
        size: 0,
        mtime: 0,
        permissions: 0,
        target_is_dir,
    }
}

#[test]
fn breadcrumb_segments_splits_absolute_paths() {
    assert_eq!(breadcrumb_segments("/"), vec![("/".into(), "/".into())]);
    assert_eq!(
        breadcrumb_segments("/home/deploy"),
        vec![
            ("/".to_string(), "/".to_string()),
            ("home".to_string(), "/home".to_string()),
            ("deploy".to_string(), "/home/deploy".to_string()),
        ]
    );
    assert_eq!(
        breadcrumb_segments("/项目/子"),
        vec![
            ("/".to_string(), "/".to_string()),
            ("项目".to_string(), "/项目".to_string()),
            ("子".to_string(), "/项目/子".to_string()),
        ]
    );
}

#[test]
fn sort_puts_dirs_first_then_name_case_insensitively() {
    let entries = vec![
        entry("Zebra.txt", SftpEntryKind::File, false),
        entry("apple", SftpEntryKind::Dir, false),
        entry("beta.txt", SftpEntryKind::File, false),
        entry("Alpha", SftpEntryKind::Dir, false),
        entry("link-to-dir", SftpEntryKind::Symlink, true),
        entry("link-to-file", SftpEntryKind::Symlink, false),
    ];
    let sorted: Vec<&str> = sorted_filtered_entries(&entries, "")
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(
        sorted,
        vec![
            "Alpha",
            "apple",
            "link-to-dir",
            "beta.txt",
            "link-to-file",
            "Zebra.txt",
        ]
    );
}

#[test]
fn filter_is_case_insensitive_substring() {
    let entries = vec![
        entry("README.md", SftpEntryKind::File, false),
        entry("src", SftpEntryKind::Dir, false),
        entry("Cargo.toml", SftpEntryKind::File, false),
    ];
    let names: Vec<&str> = sorted_filtered_entries(&entries, "a")
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, vec!["Cargo.toml", "README.md"]);
}

#[test]
fn human_size_scales_units() {
    assert_eq!(human_size(0), "0 B");
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1024), "1.0K");
    assert_eq!(human_size(1536), "1.5K");
    assert_eq!(human_size(1024 * 1024), "1.0M");
}

#[test]
fn mode_string_renders_rwx() {
    assert_eq!(mode_string(0o755), "rwxr-xr-x");
    assert_eq!(mode_string(0o644), "rw-r--r--");
    assert_eq!(mode_string(0o000), "---------");
    assert_eq!(mode_string(0o777), "rwxrwxrwx");
}

mod view_tests {
    use super::*;
    use anterm_ui_kit::config::Config;
    use gpui::{Entity, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct RecordedEvents {
        opened_files: Vec<PathBuf>,
        goto_shell_cwd: Vec<u64>,
        focus_restored: usize,
    }

    fn mount_panel(
        cx: &mut TestAppContext,
    ) -> (
        Entity<SftpPanelView>,
        Rc<RefCell<RecordedEvents>>,
        VisualTestContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::presets::load_registry(cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
        });
        let events = Rc::new(RefCell::new(RecordedEvents::default()));
        let events_clone = events.clone();
        let mut slot = None;
        let window = cx.add_window(|window, cx| {
            let panel = cx.new(|cx| SftpPanelView::new(window, cx));
            cx.subscribe(&panel, move |_, _, ev: &SftpEvent, _| {
                let mut rec = events_clone.borrow_mut();
                match ev {
                    SftpEvent::OpenRemoteFile { path, .. } => rec.opened_files.push(path.clone()),
                    SftpEvent::GotoShellCwd { pane_id } => rec.goto_shell_cwd.push(*pane_id),
                    SftpEvent::Intent(WorkspaceIntent::FocusActiveTerminal) => {
                        rec.focus_restored += 1;
                    }
                    SftpEvent::Intent(_) => {}
                }
            })
            .detach();
            slot = Some(panel.clone());
            gpui_kit::component::Root::new(panel, window, cx)
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (slot.unwrap(), events, vcx)
    }

    #[gpui::test]
    fn cancelling_edit_form_clears_edit_and_emits_focus_intent(cx: &mut TestAppContext) {
        let (panel, events, mut vcx) = mount_panel(cx);
        panel.update_in(&mut vcx, |p, window, cx| {
            p.open_pane_id = Some(42);
            p.begin_new_folder(window, cx);
            assert!(p.editing.is_some());
            assert_eq!(p.editing_sub.len(), 1);
            p.cancel_edit(cx);
            assert!(p.editing.is_none());
            assert!(p.editing_sub.is_empty());
        });
        vcx.run_until_parked();
        assert_eq!(events.borrow().focus_restored, 1);
    }

    #[gpui::test]
    fn large_directory_is_virtualized_and_file_open_emits_event(cx: &mut TestAppContext) {
        let (panel, events, mut vcx) = mount_panel(cx);
        panel.update(&mut vcx, |p, cx| {
            p.open_pane_id = Some(9);
            p.cwd = "/srv/app".into();
            p.entries = (0..500)
                .map(|i| entry(&format!("file-{i:04}.rs"), SftpEntryKind::File, false))
                .collect();
            p.entries_rev = p.entries_rev.wrapping_add(1);
            cx.notify();
        });
        let _ = row_probe::take();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        let built = row_probe::take();
        assert!(built > 0, "virtual list should render visible rows");
        assert!(
            built < 120,
            "500-entry SFTP directory must be virtualized (built {built} rows)"
        );

        panel.update_in(&mut vcx, |p, window, cx| {
            p.open_entry(entry("file-0001.rs", SftpEntryKind::File, false), cx);
            p.menu_action(SftpMenuAction::GotoShellCwd, window, cx);
        });
        vcx.run_until_parked();
        assert_eq!(
            events.borrow().opened_files,
            vec![PathBuf::from("/srv/app/file-0001.rs")]
        );
        assert_eq!(events.borrow().goto_shell_cwd, vec![9]);
    }
}
