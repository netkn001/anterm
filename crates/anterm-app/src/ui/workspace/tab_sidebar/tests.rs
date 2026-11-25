use super::*;
use crate::ui::chrome::search::{TagPopoverRow, next_tag_color, popover_rows};

#[test]
fn reveal_shift_moves_only_rows_out_of_view() {
    let view = (px(100.), px(400.));
    assert_eq!(reveal_shift((px(200.), px(230.)), px(0.), view), px(0.));
    assert_eq!(reveal_shift((px(90.), px(120.)), px(0.), view), px(0.));
    assert_eq!(reveal_shift((px(390.), px(420.)), px(0.), view), px(0.));
    assert_eq!(reveal_shift((px(10.), px(40.)), px(0.), view), px(90.));
    assert_eq!(reveal_shift((px(500.), px(530.)), px(0.), view), px(-130.));
}

#[test]
fn reordered_rows_permutes_flat_list_and_filtered_subsets() {
    // Unfiltered flat list: drag 0 -> 2.
    assert_eq!(
        reordered_rows(4, &[0, 1, 2, 3], 0, 2),
        Some(vec![1, 2, 0, 3])
    );
    // Drag upward: 3 -> 1.
    assert_eq!(
        reordered_rows(4, &[0, 1, 2, 3], 3, 1),
        Some(vec![0, 3, 1, 2])
    );
    // No-op drag returns None.
    assert_eq!(reordered_rows(4, &[0, 1, 2, 3], 2, 2), None);

    // Filtered subset: tabs 1 and 3 are hidden and stay at indices 1 and 3
    // while visible [0, 2, 4] permutes across slots [0, 2, 4].
    assert_eq!(
        reordered_rows(5, &[0, 2, 4], 0, 2),
        Some(vec![2, 1, 4, 3, 0])
    );
    assert_eq!(
        reordered_rows(5, &[0, 2, 4], 2, 0),
        Some(vec![4, 1, 0, 3, 2])
    );
}

#[test]
fn popover_rows_filters_case_insensitively_and_offers_create() {
    let mut catalog = SidebarTags::default();
    let prod = catalog.create("Prod", TagColor::Red).unwrap();
    let project = catalog.create("Project", TagColor::Blue).unwrap();
    let dev = catalog.create("Dev", TagColor::Green).unwrap();

    // Blank query lists all tags and offers no Create row.
    assert_eq!(
        popover_rows(&catalog, "   "),
        vec![
            TagPopoverRow::Existing(prod),
            TagPopoverRow::Existing(project),
            TagPopoverRow::Existing(dev),
        ]
    );

    // Substring match that doesn't exact-match appends Create row.
    assert_eq!(
        popover_rows(&catalog, "pro"),
        vec![
            TagPopoverRow::Existing(prod),
            TagPopoverRow::Existing(project),
            TagPopoverRow::Create("pro".into()),
        ]
    );

    // Case-insensitive exact match omits Create row.
    assert_eq!(
        popover_rows(&catalog, "  PROD "),
        vec![TagPopoverRow::Existing(prod)]
    );

    // Unmatched query shows only Create row.
    assert_eq!(
        popover_rows(&catalog, "Urgent"),
        vec![TagPopoverRow::Create("Urgent".into())]
    );
}

#[test]
fn next_tag_color_cycles_all_variants() {
    let mut c = TagColor::None;
    let mut seen = vec![c];
    for _ in 0..TagColor::ALL.len() - 1 {
        c = next_tag_color(c);
        seen.push(c);
    }
    assert_eq!(seen.as_slice(), TagColor::ALL.as_slice());
    assert_eq!(next_tag_color(c), TagColor::None);
}

mod sidebar_tag_tests {
    use super::super::*;
    use crate::ui::workspace::app::test_window::harness_with_tabs;
    use gpui::TestAppContext;

    fn drawn(app: &AntermApp, i: usize) -> bool {
        app.sidebar_slots.borrow()[i].size.height > px(0.)
    }

    #[gpui::test]
    fn create_attach_toggle_and_multi_tag_on_same_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let (prod, ops) = app.update(&mut vcx, |app, cx| {
            let prod = app
                .create_and_attach_tag(0, "Prod", TagColor::Red, cx)
                .expect("created");
            let ops = app
                .create_and_attach_tag(0, "Ops", TagColor::Blue, cx)
                .expect("created");
            assert_eq!(*app.tabs[0].tags.borrow(), vec![prod, ops]);
            assert!(app.tabs[1].tags.borrow().is_empty());

            // Case-insensitive create reuses existing tag id and attaches to tab 1.
            let same = app
                .create_and_attach_tag(1, "prod", TagColor::Green, cx)
                .expect("reused");
            assert_eq!(same, prod);
            assert_eq!(app.sidebar_tags.tags.len(), 2);
            assert_eq!(*app.tabs[1].tags.borrow(), vec![prod]);

            // Toggling detaches and re-attaches cleanly.
            app.toggle_tab_tag(0, prod, cx);
            assert_eq!(*app.tabs[0].tags.borrow(), vec![ops]);
            app.toggle_tab_tag(0, prod, cx);
            assert_eq!(*app.tabs[0].tags.borrow(), vec![ops, prod]);
            (prod, ops)
        });

        app.update(&mut vcx, |app, _cx| {
            let resolved = resolve_tab_tags(&app.tabs[0], &app.sidebar_tags);
            let ids: Vec<TagId> = resolved.into_iter().map(|t| t.id).collect();
            assert_eq!(ids, vec![ops, prod]);
        });
    }

    #[gpui::test]
    fn single_select_tag_filter_hides_non_matching_rows_and_clears_on_reclick(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        let (alpha, beta) = app.update(&mut vcx, |app, cx| {
            cx.global_mut::<Config>().tab_bar_position = crate::core::config::TabBarPosition::Left;
            let alpha = app
                .create_and_attach_tag(0, "Alpha", TagColor::Teal, cx)
                .unwrap();
            let beta = app
                .create_and_attach_tag(2, "Beta", TagColor::Purple, cx)
                .unwrap();
            app.toggle_tab_tag(2, alpha, cx);
            (alpha, beta)
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert!(drawn(app, 0) && drawn(app, 1) && drawn(app, 2));
            // Grouped by primary tag in catalog order (`alpha`: [0], `beta`: [2], untagged: [1]).
            assert_eq!(app.visual_tab_order(cx), vec![0, 2, 1]);
            app.toggle_tag_filter(alpha, cx);
            assert_eq!(app.active_tag_filter, Some(alpha));
            assert_eq!(app.visual_tab_order(cx), vec![0, 2]);
        });
        vcx.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            assert!(drawn(app, 0));
            assert!(!drawn(app, 1), "filtered-out tab leaves no drawn slot");
            assert!(drawn(app, 2));

            // Clicking a second tag switches single-select filter to it.
            app.toggle_tag_filter(beta, cx);
            assert_eq!(app.active_tag_filter, Some(beta));
            assert_eq!(app.visual_tab_order(cx), vec![2]);

            // Clicking active tag again clears filter back to tag-grouped order.
            app.toggle_tag_filter(beta, cx);
            assert_eq!(app.active_tag_filter, None);
            assert_eq!(app.visual_tab_order(cx), vec![0, 2, 1]);
        });
    }

    #[gpui::test]
    fn sidebar_groups_tabs_by_primary_tag_in_catalog_order_with_untagged_last(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 4);

        app.update(&mut vcx, |app, cx| {
            cx.global_mut::<Config>().tab_bar_position = crate::core::config::TabBarPosition::Left;
            let frontend = app
                .create_and_attach_tag(2, "Frontend", TagColor::Blue, cx)
                .unwrap();
            let ops = app
                .create_and_attach_tag(1, "Ops", TagColor::Red, cx)
                .unwrap();
            // Tab 3 also gets Frontend, tab 0 stays untagged.
            app.toggle_tab_tag(3, frontend, cx);

            let sections = sidebar_tag_sections(&app.tabs, &app.sidebar_tags, None);
            assert_eq!(
                sections,
                vec![
                    layout::TagSection {
                        tag: Some(frontend),
                        tabs: vec![2, 3],
                    },
                    layout::TagSection {
                        tag: Some(ops),
                        tabs: vec![1],
                    },
                    layout::TagSection {
                        tag: None,
                        tabs: vec![0],
                    },
                ]
            );
            assert_eq!(app.visual_tab_order(cx), vec![2, 3, 1, 0]);
            assert_eq!(app.tab_run(2, cx), vec![2, 3]);
            assert_eq!(app.tab_run(0, cx), vec![0]);
        });
    }

    #[gpui::test]
    fn delete_tag_detaches_from_all_tabs_clears_filter_and_never_closes_tabs(
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update(&mut vcx, |app, cx| {
            let tag = app
                .create_and_attach_tag(0, "Temp", TagColor::Amber, cx)
                .unwrap();
            app.toggle_tab_tag(1, tag, cx);
            app.toggle_tag_filter(tag, cx);
            assert_eq!(app.active_tag_filter, Some(tag));

            app.delete_tag(tag, cx);

            assert_eq!(app.tabs.len(), 2, "deleting a tag never closes tabs");
            assert!(app.sidebar_tags.is_empty());
            assert_eq!(app.active_tag_filter, None);
            assert!(app.tabs[0].tags.borrow().is_empty());
            assert!(app.tabs[1].tags.borrow().is_empty());
        });
    }

    #[gpui::test]
    fn rename_color_and_inline_tag_rename_commit_and_cancel(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let tag_id = app.update_in(&mut vcx, |app, window, cx| {
            let id = app
                .create_and_attach_tag(0, "Work", TagColor::None, cx)
                .unwrap();
            app.set_tag_color(id, TagColor::Pink, cx);
            assert_eq!(app.sidebar_tags.get(id).unwrap().color, TagColor::Pink);

            // Start inline rename, edit text, commit.
            app.start_tag_rename(id, window, cx);
            assert_eq!(app.tag_rename.as_ref().map(|r| r.tag), Some(id));
            let input = app.tag_rename.as_ref().unwrap().input.clone();
            input.update(cx, |s, cx| s.set_value("Backend", window, cx));
            app.commit_tag_rename(window, cx);
            assert!(app.tag_rename.is_none());
            assert_eq!(app.sidebar_tags.get(id).unwrap().name, "Backend");
            assert_eq!(
                *app.tabs[0].tags.borrow(),
                vec![id],
                "renaming preserves tag membership"
            );

            // Start inline rename again, change text, then cancel.
            app.start_tag_rename(id, window, cx);
            let input = app.tag_rename.as_ref().unwrap().input.clone();
            input.update(cx, |s, cx| s.set_value("Ignored", window, cx));
            app.cancel_tag_rename(window, cx);
            assert!(app.tag_rename.is_none());
            assert_eq!(app.sidebar_tags.get(id).unwrap().name, "Backend");
            id
        });

        app.update(&mut vcx, |app, cx| {
            // Adopting a catalog without `tag_id` drops active filter cleanly.
            app.toggle_tag_filter(tag_id, cx);
            assert_eq!(app.active_tag_filter, Some(tag_id));
            app.adopt_sidebar_tags(SidebarTags::default(), cx);
            assert_eq!(app.active_tag_filter, None);
        });
    }

    #[gpui::test]
    fn tag_popover_keyboard_flow_creates_toggles_and_dismisses(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let view = app.update_in(&mut vcx, |app, window, cx| {
            app.open_tag_popover(0, window, cx);
            let view = app.search.clone().expect("unified palette open on EditTags");
            assert!(view.read(cx).is_editing_tags());
            view
        });
        vcx.run_until_parked();

        // Type a new tag name and cycle color once (None -> Red), then confirm.
        vcx.simulate_input("Release");
        vcx.run_until_parked();
        view.update(&mut vcx, |v, cx| v.cycle_edit_tags_create_color(cx));
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();

        let tag_id = app.read_with(&vcx, |app, _| {
            assert_eq!(app.sidebar_tags.tags.len(), 1);
            let tag = &app.sidebar_tags.tags[0];
            assert_eq!(tag.name, "Release");
            assert_eq!(tag.color, TagColor::Red);
            let id = tag.id;
            assert_eq!(*app.tabs[0].tags.borrow(), vec![id]);
            id
        });
        let _ = tag_id;

        // Confirming again on the existing row detaches it.
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.tabs[0].tags.borrow().is_empty()));

        // Escape dismisses the standalone EditTags modal cleanly.
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_none()));
    }

    #[gpui::test]
    fn tag_popover_direct_color_pick_and_inline_existing_recolor(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let view = app.update_in(&mut vcx, |app, window, cx| {
            assert!(tag_visual(TagColor::None, cx).is_none());
            for color in TagColor::ALL.into_iter().filter(|&c| c != TagColor::None) {
                let vis = tag_visual(color, cx).expect("colored tag yields TagVisual");
                assert!(vis.pill_bg.a > 0.08);
                assert!(vis.pill_bg_selected.a > vis.pill_bg.a);
                assert!(vis.pill_border_selected.a > vis.pill_border.a);
            }

            app.open_tag_popover(0, window, cx);
            app.search.clone().expect("EditTags open")
        });
        vcx.run_until_parked();

        // Direct swatch selection picks Teal without cycling.
        vcx.simulate_input("Infra");
        vcx.run_until_parked();
        view.update(&mut vcx, |v, cx| {
            v.set_edit_tags_create_color(TagColor::Teal, cx)
        });
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();

        let tag_id = app.read_with(&vcx, |app, _| {
            let tag = &app.sidebar_tags.tags[0];
            assert_eq!(tag.name, "Infra");
            assert_eq!(tag.color, TagColor::Teal);
            tag.id
        });

        // Toggle inline color picker for existing tag, recolor to Purple, toggle closed.
        view.update(&mut vcx, |v, cx| {
            v.toggle_edit_tags_color_editor(tag_id, cx);
        });
        app.update(&mut vcx, |app, cx| {
            app.set_tag_color(tag_id, TagColor::Purple, cx);
        });
        assert_eq!(
            app.read_with(&vcx, |app, _| app.sidebar_tags.get(tag_id).unwrap().color),
            TagColor::Purple
        );
        view.update(&mut vcx, |v, cx| {
            v.toggle_edit_tags_color_editor(tag_id, cx);
        });
    }

    #[gpui::test]
    fn tag_popover_inline_rename_commit_cancel_and_delete(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let (tag_id, view) = app.update_in(&mut vcx, |app, window, cx| {
            let tag_id = app
                .create_and_attach_tag(0, "OldName", TagColor::Blue, cx)
                .unwrap();
            app.open_tag_popover(0, window, cx);
            (tag_id, app.search.clone().expect("EditTags open"))
        });
        vcx.run_until_parked();

        // Start inline rename, cancel -> preserves original name and keeps modal open.
        view.update_in(&mut vcx, |v, window, cx| {
            v.start_edit_tags_rename(tag_id, window, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("escape");
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
        assert_eq!(
            app.read_with(&vcx, |app, _| app.sidebar_tags.get(tag_id).unwrap().name.clone()),
            "OldName"
        );

        // Start inline rename again, type new name, Enter commits & updates live catalog.
        view.update_in(&mut vcx, |v, window, cx| {
            v.start_edit_tags_rename(tag_id, window, cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("cmd-a");
        vcx.simulate_input("NewName");
        vcx.run_until_parked();
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        app.read_with(&vcx, |app, _| {
            assert_eq!(app.sidebar_tags.get(tag_id).unwrap().name, "NewName");
            assert_eq!(*app.tabs[0].tags.borrow(), vec![tag_id]);
        });

        // Delete from EditTags detaches tag cleanly while keeping modal open.
        view.update_in(&mut vcx, |v, window, cx| {
            v.delete_tag_in_edit_tags(tag_id, window, cx);
        });
        vcx.run_until_parked();
        app.read_with(&vcx, |app, _| {
            assert!(app.sidebar_tags.is_empty());
            assert!(app.tabs[0].tags.borrow().is_empty());
            assert!(app.search.is_some());
        });
    }

    #[gpui::test]
    fn large_tab_sidebar_is_virtualized(cx: &mut gpui::TestAppContext) {
        use crate::core::config::TabBarPosition;
        use crate::ui::workspace::app::test_window::harness_with_tabs;
        use crate::ui::workspace::pane::Pane;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);
        super::row_probe::reset();
        app.update_in(&mut vcx, |app, window, cx| {
            cx.global_mut::<Config>().tab_bar_position = TabBarPosition::Left;
            app.sidebar_collapsed = false;
            let leaf = app.tabs[0].pane.first_leaf().expect("initial leaf").clone();
            for i in 1..300 {
                let mut tab = crate::ui::workspace::app::Tab::new(Pane::Leaf(leaf.clone()));
                tab.name = Some(format!("Tab {i}"));
                app.tabs.push(tab);
            }
            cx.notify();
            window.refresh();
        });
        vcx.run_until_parked();
        let rendered = super::row_probe::last();
        assert!(rendered > 0, "virtualized sidebar should render visible rows");
        assert!(
            rendered < 80,
            "300-tab sidebar must be virtualized (rendered {rendered} rows)"
        );
    }
}
