use super::*;
use crate::ui::workspace::app::{AntermApp, test_window::harness_with_tabs};
use gpui::{TestAppContext, VisualTestContext};

fn open(app: &Entity<AntermApp>, vcx: &mut VisualTestContext) -> Entity<SearchView> {
    app.read_with(vcx, |app, _| {
        app.search.clone().expect("the search is open")
    })
}

fn first_kind(view: &Entity<SearchView>, vcx: &mut VisualTestContext) -> Option<CommandKind> {
    view.read_with(vcx, |view, cx| {
        let delegate = view.list.read(cx).delegate();
        Some(delegate.item_at(delegate.first_row()?)?.kind.clone())
    })
}

#[gpui::test]
fn toggle_search_opens_pure_command_palette(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    anterm_i18n::set_locale("en");
    app.update_in(&mut vcx, |app, window, cx| app.toggle_search(window, cx));
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    view.read_with(&vcx, |view, _| assert_eq!(view.tab(), SearchTab::Actions));

    vcx.simulate_input("split right");
    vcx.run_until_parked();
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::SplitRight.into())
    );
}

#[gpui::test]
fn add_connection_moves_to_an_empty_hosts_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    anterm_i18n::set_locale("en");
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "ssh-add-connection", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::SearchHosts.into())
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, cx| {
        assert_eq!(view.tab(), SearchTab::Hosts);
        assert!(view.can_pop());
        assert_eq!(view.list.read(cx).delegate().query, "");
    });

    // Escape pops back to Actions with the parked query restored.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, cx| {
        assert_eq!(view.tab(), SearchTab::Actions);
        assert!(!view.can_pop());
        assert_eq!(view.input.read(cx).value().as_ref(), "ssh-add-connection");
    });
}

#[gpui::test]
fn quick_open_goes_to_the_files_tab_and_back_out(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    anterm_i18n::set_locale("en");
    app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    view.read_with(&vcx, |view, _| assert_eq!(view.tab(), SearchTab::Files));

    view.update_in(&mut vcx, |view, window, cx| {
        view.set_tab(SearchTab::Actions, None, window, cx)
    });
    app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
    vcx.run_until_parked();
    view.read_with(&vcx, |view, _| assert_eq!(view.tab(), SearchTab::Files));

    app.update_in(&mut vcx, |app, window, cx| app.quick_open_file(window, cx));
    vcx.run_until_parked();
    assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
}

#[gpui::test]
fn a_standalone_tab_keeps_its_list_on_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    anterm_i18n::set_locale("en");
    for tab in [
        SearchTab::Actions,
        SearchTab::Tabs,
        SearchTab::Tags,
        SearchTab::Files,
        SearchTab::Symbols,
        SearchTab::Locations,
    ] {
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(tab, "", window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        view.update_in(&mut vcx, |view, window, cx| {
            view.step_tab(true, window, cx);
            view.step_tab(false, window, cx);
        });
        view.read_with(&vcx, |view, _| assert_eq!(view.tab(), tab));
        app.update_in(&mut vcx, |app, window, cx| app.close_search(window, cx));
        vcx.run_until_parked();
    }
}

#[gpui::test]
fn go_to_file_moves_to_an_empty_files_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    anterm_i18n::set_locale("en");
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "go-to-file", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::QuickOpenFile.into())
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, cx| {
        assert_eq!(view.tab(), SearchTab::Files);
        assert_eq!(view.list.read(cx).delegate().query, "");
    });
}

#[gpui::test]
fn a_file_list_that_lands_late_fills_the_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    anterm_i18n::set_locale("en");
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Files, "lib", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(first_kind(&view, &mut vcx), None);

    let root = std::path::PathBuf::from("/repo");
    let index = super::super::files::build_index(
        std::slice::from_ref(&root),
        vec![root.join("src/lib.rs")],
        false,
    );
    view.update_in(&mut vcx, |view, window, cx| {
        view.set_files(FileList::Ready(std::sync::Arc::new(index)), window, cx)
    });
    vcx.run_until_parked();
    assert!(matches!(
        first_kind(&view, &mut vcx),
        Some(CommandKind::Pick(PickerTarget::OpenFile { .. }))
    ));
}

#[gpui::test]
fn a_query_after_one_that_found_nothing_still_arms_its_first_row(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
    anterm_i18n::set_locale("en");
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);

    vcx.simulate_input("split rightq");
    vcx.run_until_parked();
    assert_eq!(first_kind(&view, &mut vcx), None, "nothing matches");
    vcx.simulate_keystrokes("backspace");
    vcx.run_until_parked();

    let selected = view.read_with(&vcx, |view, cx| view.list.read(cx).selected_index());
    assert!(selected.is_some(), "Return has a row to run");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
}

#[gpui::test]
fn switch_tab_from_palette_lists_mru_and_switches_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
    anterm_i18n::set_locale("en");
    let (tag_a, tab_ids) = app.update_in(&mut vcx, |app, window, cx| {
        app.tabs[0].name = Some("alpha".into());
        app.tabs[1].name = Some("beta".into());
        app.tabs[2].name = Some("gamma".into());
        let tag_a = app
            .create_and_attach_tag(0, "Frontend", crate::core::tag::TagColor::Blue, cx)
            .unwrap();
        let _tag_b = app
            .create_and_attach_tag(2, "Backend", crate::core::tag::TagColor::Green, cx)
            .unwrap();
        app.activate(2, window, cx);
        app.touch_active_tab();
        app.activate(0, window, cx);
        app.touch_active_tab();
        app.toggle_tag_filter(tag_a, cx);
        let ids: Vec<_> = app.tabs.iter().map(|t| t.tree_id.get()).collect();
        (tag_a, ids)
    });
    vcx.run_until_parked();
    assert_eq!(app.read_with(&vcx, |app, _| app.active), 0);
    assert_eq!(
        app.read_with(&vcx, |app, _| app.active_tag_filter),
        Some(tag_a)
    );

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "switch tab", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::OpenTabPicker.into())
    );

    // Enter pushes SearchTab::Tabs onto the stack with MRU non-active tab first.
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, cx| {
        assert_eq!(view.tab(), SearchTab::Tabs);
        assert!(view.can_pop());
        let delegate = view.list.read(cx).delegate();
        let rows = &delegate.sections[0].rows;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, PickerTarget::GoToTab(tab_ids[2]).into());
        assert_eq!(rows[2].kind, PickerTarget::GoToTab(tab_ids[0]).into());
        assert_eq!(rows[2].note.as_deref(), Some("✓"));
    });

    // Search by attached tag name ("backend") finds tab 2 and switches to it.
    vcx.simulate_input("backend");
    vcx.run_until_parked();
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PickerTarget::GoToTab(tab_ids[2]).into())
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.active, 2);
        assert_eq!(
            app.active_tag_filter, None,
            "jumping to a tab outside the filter clears the filter"
        );
    });
}

#[gpui::test]
fn switch_tag_from_palette_filters_and_activates_tagged_tab(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);
    anterm_i18n::set_locale("en");
    let (frontend, backend) = app.update(&mut vcx, |app, cx| {
        let frontend = app
            .create_and_attach_tag(0, "Frontend", crate::core::tag::TagColor::Blue, cx)
            .unwrap();
        let backend = app
            .create_and_attach_tag(2, "Backend", crate::core::tag::TagColor::Green, cx)
            .unwrap();
        (frontend, backend)
    });
    vcx.run_until_parked();
    assert_eq!(app.read_with(&vcx, |app, _| app.active), 0);

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "switch tag", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::OpenTagPicker.into())
    );

    // Enter pushes Switch Tag; since filter is All, tags come first and All is last with ✓.
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, cx| {
        assert_eq!(view.tab(), SearchTab::Tags);
        let rows = &view.list.read(cx).delegate().sections[0].rows;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, PickerTarget::SelectTag(Some(frontend)).into());
        assert_eq!(rows[1].kind, PickerTarget::SelectTag(Some(backend)).into());
        assert_eq!(rows[2].kind, PickerTarget::SelectTag(None).into());
        assert_eq!(rows[2].note.as_deref(), Some("✓"));
    });

    // Pick Backend -> filters sidebar to Backend and activates tab 2.
    vcx.simulate_input("backend");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.active_tag_filter, Some(backend));
        assert_eq!(app.active, 2);
    });

    // Re-opening Switch Tag via action puts `All` first so Enter clears the filter.
    app.update_in(&mut vcx, |app, window, cx| app.toggle_switch_tag(window, cx));
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PickerTarget::SelectTag(None).into())
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.active_tag_filter, None);
        assert_eq!(app.active, 2);
    });

    // Calling toggle_switch_tag twice toggles open then closed.
    app.update_in(&mut vcx, |app, window, cx| app.toggle_switch_tag(window, cx));
    vcx.run_until_parked();
    assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
    app.update_in(&mut vcx, |app, window, cx| app.toggle_switch_tag(window, cx));
    vcx.run_until_parked();
    assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
}

#[gpui::test]
fn palette_split_left_and_split_up_place_new_pane_before_target(cx: &mut TestAppContext) {
    use crate::ui::workspace::pane::Pane;

    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    anterm_i18n::set_locale("en");

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "split left", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::SplitLeft.into())
    );

    app.update_in(&mut vcx, |app, window, cx| app.close_search(window, cx));
    vcx.run_until_parked();

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "split up", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::SplitUp.into())
    );

    let (slot0, slot1) = app.read_with(&vcx, |app, _| {
        (
            app.tabs[0].pane.first_leaf().unwrap(),
            app.tabs[1].pane.first_leaf().unwrap(),
        )
    });
    let (id0, id1) = (slot0.entity_id(), slot1.entity_id());

    let mut h = Pane::leaf(slot0.clone());
    assert!(h.split_leaf(id0, gpui::Axis::Horizontal, true, slot1.clone()));
    let h_ids: Vec<_> = h.leaves().into_iter().map(|l| l.entity_id()).collect();
    assert_eq!(h_ids, vec![id1, id0]);

    let mut v = Pane::leaf(slot0);
    assert!(v.split_leaf(id0, gpui::Axis::Vertical, true, slot1));
    let v_ids: Vec<_> = v.leaves().into_iter().map(|l| l.entity_id()).collect();
    assert_eq!(v_ids, vec![id1, id0]);
}

#[gpui::test]
fn rename_tab_from_palette_edits_in_place_resets_and_escapes_back(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    anterm_i18n::set_locale("en");

    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "rename", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::RenameTab.into())
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_some(), "palette stays open for rename");
        assert!(app.renaming.is_none(), "must not jump to sidebar rename");
    });
    view.read_with(&vcx, |view, cx| {
        assert!(view.is_renaming());
        assert!(view.can_pop());
        assert!(!view.input.read(cx).value().is_empty());
    });

    // Escape pops back to the parked command palette query without changing the tab.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, cx| {
        assert!(!view.is_renaming());
        assert_eq!(view.input.read(cx).value().as_ref(), "rename");
    });
    assert_eq!(app.read_with(&vcx, |app, _| app.tabs[0].name.clone()), None);

    // Enter again -> type new name -> Enter commits and closes palette.
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.update_in(&mut vcx, |view, window, cx| {
        view.input
            .update(cx, |s, cx| s.set_value("Backend API", window, cx));
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.tabs[0].name.as_deref(), Some("Backend API"));
    });

    // Standalone prompt: clearing to empty and pressing Backspace stays in prompt,
    // and Enter resets the custom name back to automatic (`None`).
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_rename_tab_prompt(window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    view.update_in(&mut vcx, |view, window, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "Backend API");
        view.input.update(cx, |s, cx| s.set_value("", window, cx));
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("backspace");
    vcx.run_until_parked();
    view.read_with(&vcx, |view, _| {
        assert!(
            view.is_renaming(),
            "backspace on empty rename prompt must not exit rename mode"
        );
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.tabs[0].name, None);
    });

    // Searching "rename tag", "delete tag", or "manage tags" resolves to `EditTabTags`.
    for query in ["rename tag", "delete tag", "manage tags"] {
        app.update_in(&mut vcx, |app, window, cx| {
            app.open_search(SearchTab::Actions, query, window, cx)
        });
        vcx.run_until_parked();
        let view = open(&app, &mut vcx);
        assert_eq!(
            first_kind(&view, &mut vcx),
            Some(PaletteAction::EditTabTags.into()),
            "query `{query}` should resolve to EditTabTags"
        );
        app.update_in(&mut vcx, |app, window, cx| app.close_search(window, cx));
        vcx.run_until_parked();
    }
}

#[gpui::test]
fn edit_tab_tags_and_secondary_row_actions_run_in_palette_stack(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    anterm_i18n::set_locale("en");
    app.update(&mut vcx, |app, cx| {
        app.tabs[0].name = Some("alpha".into());
        app.tabs[1].name = Some("beta".into());
        app.create_and_attach_tag(0, "Ops", crate::core::tag::TagColor::Blue, cx);
    });
    vcx.run_until_parked();

    // 1. Cmd+P -> Edit Tab Tags pushes in-place onto the stack; Esc pops back to Cmd+P.
    app.update_in(&mut vcx, |app, window, cx| {
        app.open_search(SearchTab::Actions, "edit tab tags", window, cx)
    });
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::EditTabTags.into())
    );

    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    view.read_with(&vcx, |v, _| {
        assert!(v.is_editing_tags(), "Edit Tab Tags opens inside palette");
        assert!(v.can_pop(), "can pop back to Actions");
    });

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    view.read_with(&vcx, |v, cx| {
        assert!(!v.is_editing_tags());
        assert_eq!(v.tab(), SearchTab::Actions);
        assert_eq!(v.input.read(cx).value().as_ref(), "edit tab tags");
    });

    app.update_in(&mut vcx, |app, window, cx| app.close_search(window, cx));
    vcx.run_until_parked();

    // 2. Switch Tab -> secondary edit gesture renames the highlighted non-active tab (tab 1) in-place!
    app.update_in(&mut vcx, |app, window, cx| app.toggle_switch_tab(window, cx));
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    view.update_in(&mut vcx, |v, window, cx| {
        assert!(v.on_edit_gesture(window, cx));
    });
    vcx.run_until_parked();
    view.update_in(&mut vcx, |v, window, cx| {
        assert!(v.is_renaming());
        assert_eq!(v.input.read(cx).value().as_ref(), "beta");
        v.input
            .update(cx, |s, cx| s.set_value("beta-renamed", window, cx));
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.tabs[1].name.as_deref(), Some("beta-renamed"));
        assert_eq!(app.active, 0, "renaming tab 1 did not switch away from tab 0");
    });

    // 3. Switch Tab -> secondary tags gesture opens EditTags targeting tab 1!
    app.update_in(&mut vcx, |app, window, cx| app.toggle_switch_tab(window, cx));
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);
    let tab1_id = app.read_with(&vcx, |app, _| app.tabs[1].tree_id.get());
    view.update_in(&mut vcx, |v, window, cx| {
        assert!(v.on_tags_gesture(window, cx));
    });
    vcx.run_until_parked();
    view.read_with(&vcx, |v, _| {
        assert_eq!(v.edit_tags_page().map(|p| p.tab), Some(tab1_id));
        assert!(v.can_pop());
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(
        app.read_with(&vcx, |app, _| app.tabs[1].tags.borrow().len()),
        1
    );

    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    view.read_with(&vcx, |v, _| assert_eq!(v.tab(), SearchTab::Tabs));
}

#[gpui::test]
fn root_palette_prefix_routing_switches_tabs_tags_and_files_live(cx: &mut TestAppContext) {
    let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
    anterm_i18n::set_locale("en");
    let (tab1_id, ops_id) = app.update(&mut vcx, |app, cx| {
        app.tabs[0].name = Some("alpha".into());
        app.tabs[1].name = Some("beta".into());
        let ops = app
            .create_and_attach_tag(1, "Ops", crate::core::tag::TagColor::Teal, cx)
            .unwrap();
        (app.tabs[1].tree_id.get(), ops)
    });
    vcx.run_until_parked();

    // Open pure Command Palette (`Cmd+P`).
    app.update_in(&mut vcx, |app, window, cx| app.toggle_search(window, cx));
    vcx.run_until_parked();
    let view = open(&app, &mut vcx);

    // `@beta` live-routes to Tabs and selects tab 1 without leaving the root page.
    vcx.simulate_input("@beta");
    vcx.run_until_parked();
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PickerTarget::GoToTab(tab1_id).into())
    );
    view.read_with(&vcx, |v, cx| {
        assert_eq!(
            v.effective_picker_scope(cx),
            Some((PickerScope::Tab(SearchTab::Tabs), true))
        );
    });

    // `#ops` live-routes to Tags and selects the Ops tag.
    vcx.simulate_keystrokes("cmd-a");
    vcx.simulate_input("#ops");
    vcx.run_until_parked();
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PickerTarget::SelectTag(Some(ops_id)).into())
    );

    // `>split left` stays in Actions with leading `>` stripped.
    vcx.simulate_keystrokes("cmd-a");
    vcx.simulate_input(">split left");
    vcx.run_until_parked();
    assert_eq!(
        first_kind(&view, &mut vcx),
        Some(PaletteAction::SplitLeft.into())
    );

    // Pressing Enter on `#ops` filters to Ops and activates tab 1!
    vcx.simulate_keystrokes("cmd-a");
    vcx.simulate_input("#ops");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    app.read_with(&vcx, |app, _| {
        assert!(app.search.is_none());
        assert_eq!(app.active_tag_filter, Some(ops_id));
        assert_eq!(app.active, 1);
    });
}
