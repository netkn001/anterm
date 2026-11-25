use super::*;
use super::io::{ExternalChange, Observed, classify_external_change, is_program, selected_lines};

/// A temp dir as the editor will see paths under it: through the local
/// host's `canonicalize`, the call `load_file` makes. That resolves
/// macOS's `/var` → `/private/var` and Windows' 8.3 short names
/// (`RUNNER~1`), and never answers in Windows' `\\?\` form.
pub fn test_real_dir(dir: &Path) -> PathBuf {
    anterm_core::host::local::LocalHost::new()
        .canonicalize(dir)
        .unwrap()
}

/// Handing a file the editor cannot read to the desktop is how a click
/// opens a PNG. It must not be how a click runs a build's output.
#[cfg(unix)]
#[test]
fn a_file_the_desktop_would_run_is_not_handed_to_it() {
    use std::os::unix::fs::PermissionsExt;

    let dir = std::env::temp_dir().join(format!("anterm-program-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create dir");
    let image = dir.join("shot.png");
    let program = dir.join("built");
    std::fs::write(&image, b"\x89PNG\0\0").expect("write image");
    std::fs::write(&program, b"\x7fELF\0\0").expect("write program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("mark executable");

    assert!(!is_program(&image), "a picture is only ever shown");
    assert!(is_program(&program), "a binary would be launched");
    assert!(
        !is_program(&dir),
        "a directory is not a program, whatever its mode says"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn language_map_covers_common_extensions() {
    for (path, lang) in [
        ("a/b/main.rs", "rust"),
        ("x.tsx", "tsx"),
        ("x.jsx", "javascript"),
        ("x.yml", "yaml"),
        ("Makefile", "make"),
        ("CMakeLists.txt", "cmake"),
        (".zshrc", "bash"),
        ("notes.md", "markdown"),
        ("query.SQL", "sql"),
        ("unknown.xyz", "text"),
        ("no_ext", "text"),
    ] {
        assert_eq!(language_for_path(Path::new(path)), lang, "path {path}");
    }
}

fn mt(secs: i64, nanos: u32) -> Option<MTime> {
    Some(MTime { secs, nanos })
}

#[test]
fn external_changes_are_told_apart_from_our_own_saves() {
    let ours = mt(100, 0);
    let seen = |m| Observed::Present(m);

    assert_eq!(
        classify_external_change(false, false, ours, seen(ours)),
        ExternalChange::Ignore
    );
    assert_eq!(
        classify_external_change(false, false, ours, seen(mt(101, 0))),
        ExternalChange::Reload
    );
    assert_eq!(
        classify_external_change(false, true, ours, seen(mt(101, 0))),
        ExternalChange::Conflict
    );
    assert_eq!(
        classify_external_change(false, false, mt(100, 0), seen(mt(100, 1))),
        ExternalChange::Reload
    );
    assert_eq!(
        classify_external_change(true, false, ours, seen(mt(101, 0))),
        ExternalChange::Ignore
    );
    assert_eq!(
        classify_external_change(false, false, None, seen(None)),
        ExternalChange::Reload
    );
}

#[test]
fn a_file_that_vanished_is_reported_deleted_whether_or_not_it_was_edited() {
    assert_eq!(
        classify_external_change(false, false, mt(1, 0), Observed::Missing),
        ExternalChange::Deleted
    );
    assert_eq!(
        classify_external_change(false, true, mt(1, 0), Observed::Missing),
        ExternalChange::Deleted
    );
    // Our own save replaces the file by rename, and a watcher can catch
    // the instant between.
    assert_eq!(
        classify_external_change(true, false, mt(1, 0), Observed::Missing),
        ExternalChange::Ignore
    );
}

#[test]
fn the_fingerprint_does_not_depend_on_how_the_text_is_chunked() {
    let text = "fn main() {\n    println!(\"中文\");\n}\n";
    let whole = Fingerprint::of_str(text);
    let (a, b) = text.split_at(13);
    assert_eq!(
        Fingerprint::of_chunks(text.len(), [a, b].into_iter()),
        whole
    );
    assert_ne!(Fingerprint::of_str("fn main() {}\n"), whole);
}

#[test]
fn only_the_span_that_changed_is_replaced() {
    assert_eq!(differing_span("abc", "abc"), None);
    assert_eq!(
        differing_span("hello world", "hello there world"),
        Some((6, 6, 12))
    );
    assert_eq!(differing_span("abc", "abXc"), Some((2, 2, 3)));
    assert_eq!(differing_span("abc", ""), Some((0, 3, 0)));
    assert_eq!(differing_span("", "new"), Some((0, 0, 3)));
    // "aaa" → "aa": the overlap of prefix and suffix must not double-count.
    assert_eq!(differing_span("aaa", "aa"), Some((2, 3, 2)));
}

#[test]
fn a_changed_span_never_splits_a_character() {
    // 中 and 丰 share their first two bytes in UTF-8.
    let (s, oe, ne) = differing_span("x中y", "x丰y").unwrap();
    let (old, new) = ("x中y", "x丰y");
    assert!(old.is_char_boundary(s) && new.is_char_boundary(s));
    assert_eq!(&old[s..oe], "中");
    assert_eq!(&new[s..ne], "丰");
}

#[test]
fn an_agent_is_told_the_lines_a_selection_covers() {
    let text = gpui_kit::component::input::Rope::from("one\ntwo\nthree\n");
    assert_eq!(selected_lines(&text, 0..0), None);
    assert_eq!(selected_lines(&text, 1..2), Some((1, 1)));
    assert_eq!(selected_lines(&text, 2..9), Some((1, 3)));
    // Whole lines picked by dragging to the start of the next one.
    assert_eq!(selected_lines(&text, 4..14), Some((2, 3)));
    assert_eq!(selected_lines(&text, 0..4), Some((1, 1)));
}

#[test]
fn go_to_line_reads_the_forms_people_type() {
    assert_eq!(parse_line_target("120"), Some((120, 1)));
    assert_eq!(parse_line_target(" 120:4 "), Some((120, 4)));
    assert_eq!(parse_line_target(":7"), Some((7, 1)));
    assert_eq!(parse_line_target("7,3"), Some((7, 3)));
    assert_eq!(parse_line_target("7:"), Some((7, 1)));
    assert_eq!(parse_line_target("0"), None);
    assert_eq!(parse_line_target("abc"), None);
    assert_eq!(parse_line_target(""), None);
}

#[test]
fn a_strip_opens_new_files_beside_the_current_one_and_closes_toward_the_right() {
    let ids: Vec<BufferId> = (1..=4u64).map(gpui::EntityId::from).collect();
    let mut code = TabCode::new();
    code.show(ids[0]);
    code.show(ids[1]);
    assert_eq!(code.files, vec![ids[0], ids[1]]);
    code.active = 0;
    code.show(ids[2]);
    assert_eq!(
        code.files,
        vec![ids[0], ids[2], ids[1]],
        "opened right of the active one"
    );
    assert_eq!(code.active, 1);

    // Showing one already open only brings it forward.
    code.show(ids[1]);
    assert_eq!(code.files.len(), 3);
    assert_eq!(code.active, 2);

    // Closing the active file brings its right-hand neighbour forward,
    // or the left one at the end of the strip.
    code.active = 1;
    assert!(code.forget(ids[2]));
    assert_eq!(code.files, vec![ids[0], ids[1]]);
    assert_eq!(code.active, 1);
    assert!(code.forget(ids[1]));
    assert_eq!(code.active, 0);
    // Closing one left of the active file keeps the same file in front.
    code.show(ids[3]);
    assert_eq!(code.files, vec![ids[0], ids[3]]);
    assert!(code.forget(ids[0]));
    assert_eq!(code.active_id(), Some(ids[3]));
    assert!(!code.forget(ids[0]));
}

#[test]
fn files_arriving_in_the_background_keep_their_order_and_the_front_file() {
    let ids: Vec<BufferId> = (1..=4u64).map(gpui::EntityId::from).collect();
    let mut code = TabCode::new();
    code.show(ids[0]);
    // A restore lists files in the order they were recorded; showing each
    // beside the active one would have reversed them.
    code.adopt(&ids[1..]);
    assert_eq!(code.files, ids);
    assert_eq!(code.active_id(), Some(ids[0]));
    code.adopt(&ids[2..3]);
    assert_eq!(
        code.files.len(),
        4,
        "a file already listed is not listed twice"
    );
}

mod close_gpui_tests {
    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::super::nav::gpui_tests::open_rs;
    use crate::workspace::test_support::harness_with_tabs;
    use crate::{CodeEditorEvent, CodeEditorWorkspace};

    fn panel(app: &Entity<CodeEditorWorkspace>, vcx: &mut VisualTestContext) -> (bool, usize) {
        app.read_with(vcx, |app, _| {
            let code = app.tab_code().expect("the tab is still there");
            (code.visible, code.files.len())
        })
    }

    fn settle(vcx: &mut VisualTestContext) {
        vcx.run_until_parked();
        let _ = vcx.update(|window, cx| window.draw(cx));
        vcx.run_until_parked();
    }

    /// Closing the last file keeps focus in the empty editor panel; a second
    /// close hides the panel and emits `FocusActiveTerminal`; a third returns
    /// `false` so the workspace handles closing the terminal tab.
    #[gpui::test]
    fn closing_the_last_file_keeps_close_on_the_editor(cx: &mut TestAppContext) {
        let (app, _tabs, mut vcx) = harness_with_tabs(cx, 1);
        anterm_i18n::set_locale("en");
        let focus_emitted = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let focus_ref = focus_emitted.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&app, move |_, ev: &CodeEditorEvent, _| {
                if matches!(ev, CodeEditorEvent::FocusActiveTerminal) {
                    focus_ref.set(focus_ref.get() + 1);
                }
            })
            .detach();
        });

        open_rs(&app, &mut vcx, "/close/a.rs");
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 1));

        let handled = app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_active_if_focused(window, cx)
        });
        assert!(handled);
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 0), "the file closes first");
        assert_eq!(focus_emitted.get(), 0);

        let handled = app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_active_if_focused(window, cx)
        });
        assert!(handled);
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (false, 0), "then the empty editor");
        assert_eq!(focus_emitted.get(), 1, "and returns focus to the terminal");

        let handled = app.update_in(&mut vcx, |app, window, cx| {
            app.editor_close_active_if_focused(window, cx)
        });
        assert!(!handled, "only now does close belong to the terminal");
    }

    /// Switching tabs isolates each tab's editor panel and focus memory.
    #[gpui::test]
    fn switching_to_an_empty_editor_leaves_the_focus_alone(cx: &mut TestAppContext) {
        let (app, tabs, mut vcx) = harness_with_tabs(cx, 2);
        anterm_i18n::set_locale("en");
        app.update_in(&mut vcx, |app, _, _| {
            app.ctx.active_tab = Some(tabs[1]);
            app.tab_code_mut_or_init().unwrap().visible = true;
            app.ctx.active_tab = Some(tabs[0]);
        });
        open_rs(&app, &mut vcx, "/close/a.rs");
        settle(&mut vcx);
        app.update_in(&mut vcx, |app, _, _| {
            app.ctx.active_tab = Some(tabs[1]);
        });
        settle(&mut vcx);
        assert_eq!(panel(&app, &mut vcx), (true, 0));
        assert!(
            !app.update_in(&mut vcx, |app, window, cx| {
                app.editor_close_active_if_focused(window, cx)
            }),
            "unfocused empty panel in tab 1 must not steal close"
        );
    }

    /// Regression test: drawing the Helix block cursor when a multi-byte UTF-8
    /// character (CJK / punctuation / emoji) spans `head + 8` must never slice
    /// inside a code point during `window.draw`.
    #[gpui::test]
    fn helix_block_cursor_renders_across_multibyte_utf8_markdown(cx: &mut TestAppContext) {
        let (app, tabs, mut vcx) = harness_with_tabs(cx, 1);
        anterm_i18n::set_locale("en");
        let md = "1234567中\n# Electron DOM Mirror\n\nCodex、WorkBuddy 和 🚀\n";
        let char_offsets: Vec<usize> = md.char_indices().map(|(i, _)| i).collect();

        let id = app.update_in(&mut vcx, |app, window, cx| {
            let cfg = cx.global_mut::<anterm_ui_kit::config::Config>();
            cfg.helix_mode = true;
            cfg.editor_markdown_preview = false;

            let host = app.active_host(cx).expect("a host");
            let tab = tabs[0];
            let indent = anterm_editor::text::detect_indent(md, "markdown");
            let id = app.editor_new_buffer(
                host,
                std::path::PathBuf::from("/repro/README.md"),
                None,
                md.to_string(),
                anterm_editor::text::TextFormat::default(),
                anterm_editor::text::EditorConfig::default(),
                indent,
                None,
                window,
                cx,
            );
            app.editor_show_in_tab(tab, id, true, window, cx);
            id
        });

        settle(&mut vcx);

        for off in char_offsets {
            app.update_in(&mut vcx, |app, window, cx| {
                if let Some(f) = app.buffer_mut(id) {
                    f.helix.anchor = off;
                    f.helix.head = off;
                }
                app.editor_sync_helix_selection(id, window, cx);
            });
            settle(&mut vcx);
        }
    }
}
