mod window_drag_tests {
    use gpui::{
        Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, PlatformInput, Point, Render,
        Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    struct Host;
    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(super::super::title_bar_drag(
                div().id("stand-in-title-bar").w_full().h(px(40.)),
                "stand-in-title-bar",
                window,
                cx,
            ))
        }
    }

    struct HandleOverRow {
        occluded: bool,
    }
    impl Render for HandleOverRow {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let handle = div()
                .absolute()
                .top_0()
                .left(px(296.))
                .w(px(8.))
                .h_full()
                .cursor_col_resize()
                .on_mouse_down(MouseButton::Left, |_, window, _| window.refresh());
            let handle = if self.occluded {
                handle.occlude()
            } else {
                handle
            };
            div()
                .relative()
                .size_full()
                .child(super::super::title_bar_drag(
                    div().id("stand-in-title-bar").w_full().h(px(40.)),
                    "stand-in-title-bar",
                    window,
                    cx,
                ))
                .child(handle)
        }
    }

    struct PerFrameCellHost;
    impl Render for PerFrameCellHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let should_move = Rc::new(Cell::new(false));
            div().size_full().child(
                div()
                    .id("per-frame-cell")
                    .w_full()
                    .h(px(40.))
                    .on_mouse_down(MouseButton::Left, {
                        let should_move = should_move.clone();
                        move |_, _, _| should_move.set(true)
                    })
                    .on_mouse_move(move |_, window, _| {
                        if should_move.replace(false) {
                            window.start_window_move();
                        }
                    }),
            )
        }
    }

    fn down(at: Point<Pixels>) -> PlatformInput {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        })
    }

    fn up(at: Point<Pixels>) -> PlatformInput {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        })
    }

    fn moved(at: Point<Pixels>, held: bool) -> PlatformInput {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: held.then_some(MouseButton::Left),
            modifiers: Modifiers::none(),
        })
    }

    const ON_ROW: Point<Pixels> = Point {
        x: px(300.),
        y: px(20.),
    };

    fn drifted(at: Point<Pixels>) -> Point<Pixels> {
        point(at.x + px(12.), at.y + px(3.))
    }

    fn press_repaint_move(vcx: &mut VisualTestContext, at: Point<Pixels>) {
        vcx.update(|window, cx| {
            window.dispatch_event(moved(at, false), cx);
            window.dispatch_event(down(at), cx);
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.dispatch_event(moved(drifted(at), true), cx);
        });
        vcx.run_until_parked();
    }

    #[gpui::test]
    #[should_panic(expected = "not implemented")]
    fn the_arm_survives_a_repaint_between_press_and_move(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Host);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
    }

    #[gpui::test]
    fn a_per_frame_cell_loses_the_arm_to_the_same_repaint(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| PerFrameCellHost);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
    }

    #[gpui::test]
    fn a_press_alone_does_not_move_the_window(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Host);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(ON_ROW, false), cx);
            window.dispatch_event(down(ON_ROW), cx);
            window.dispatch_event(up(ON_ROW), cx);
        });
        vcx.run_until_parked();
    }

    #[gpui::test]
    fn a_release_disarms_so_a_later_hover_does_not_drag(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Host);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(ON_ROW, false), cx);
            window.dispatch_event(down(ON_ROW), cx);
            window.dispatch_event(up(ON_ROW), cx);
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.dispatch_event(moved(drifted(ON_ROW), false), cx);
        });
        vcx.run_until_parked();
    }

    #[gpui::test]
    fn a_press_on_a_resize_handle_does_not_move_the_window(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| HandleOverRow { occluded: true });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
    }

    #[gpui::test]
    #[should_panic(expected = "not implemented")]
    fn a_handle_without_a_blocking_hitbox_hands_the_press_to_the_row(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| HandleOverRow { occluded: false });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        press_repaint_move(&mut vcx, ON_ROW);
    }

    #[gpui::test]
    fn two_rows_on_screen_keep_separate_arms(cx: &mut TestAppContext) {
        struct TwoRows;
        impl Render for TwoRows {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .child(super::super::title_bar_drag(
                        div().id("row-a").w_full().h(px(40.)),
                        "row-a",
                        window,
                        cx,
                    ))
                    .child(super::super::title_bar_drag(
                        div().id("row-b").w_full().h(px(40.)),
                        "row-b",
                        window,
                        cx,
                    ))
            }
        }

        let window = cx.add_window(|_, _| TwoRows);
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.update(|window, cx| {
            window.dispatch_event(moved(point(px(300.), px(20.)), false), cx);
            window.dispatch_event(down(point(px(300.), px(20.))), cx);
        });
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.dispatch_event(moved(point(px(312.), px(60.)), false), cx);
        });
        vcx.run_until_parked();
    }
}

mod unit_tests {
    use super::super::close_reason;
    use super::super::session_spawn::{
        mru_order, native_ssh_pane_alive, one_slot_move, step_in_order,
    };
    use super::super::{
        CloseReason, DOCUMENT_MIN_W, OpenedFrom, Rename, TERMINAL_MIN_W, TITLE_BAR_BORDER,
        TITLE_BAR_HEIGHT, Tab, TabAgentSession, clear_window_override_values, close_prompt,
        document_column_px, inherited_start_dir, join_shell_args, parse_ssh_connect_input,
        parse_ssh_option_words, rename_outcome, side_panel_max, split_shell_args, strip_band,
        title_strip_centre, title_strip_content_centre, title_strip_rule_and_pad, wd_path_saveable,
    };
    use crate::ui::workspace::pane::{Dir, Pane};
    use gpui::{Edges, point, px, size};

    #[test]
    fn title_bar_buttons_rest_out_of_sight_only_when_asked_to_and_nobody_points() {
        // Off: painted whether or not the pointer is anywhere near.
        assert!(super::super::titlebar_chrome_shown(false, false));
        assert!(super::super::titlebar_chrome_shown(false, true));
        // On: painted exactly while the pointer is over the bar.
        assert!(!super::super::titlebar_chrome_shown(true, false));
        assert!(super::super::titlebar_chrome_shown(true, true));
    }

    /// A title-bar row spends [`TITLE_BAR_BORDER`] of its height below its
    /// contents, and there are two ways it does that: it carries the whole point
    /// as a rule painted transparent (the strip's `TitleBar`, the rail's header,
    /// the code panel's, the settings band), or it draws its own thinner hairline
    /// and gives the rest back as padding (the diff overlay's 0.5pt rule). Both
    /// have to leave the contents on the line the traffic lights hang on — the
    /// rows sit side by side in the same band, and half a point of difference is
    /// the one device pixel that reads as a step.
    #[test]
    fn a_rows_own_hairline_and_the_bars_invisible_rule_reach_the_same_centre() {
        for rule in [px(0.), px(0.5), px(TITLE_BAR_BORDER), px(2.)] {
            let (border, pad) = title_strip_rule_and_pad(rule);
            assert!(
                border.as_f32() <= TITLE_BAR_BORDER,
                "a thicker rule than the bar's own point would push the contents \
                 onto a line nothing else is on"
            );
            assert!(
                pad.as_f32() >= 0.,
                "negative padding is not padding, and taffy would swallow it"
            );
            assert_eq!(
                title_strip_content_centre(rule),
                title_strip_centre(),
                "a rule of {} leaves the box short by {} of border and {} of padding",
                rule.as_f32(),
                border.as_f32(),
                pad.as_f32()
            );
        }
    }

    #[test]
    fn a_rename_box_left_alone_is_not_a_rename() {
        // The box opens holding the label already on screen, and `Blur`
        // commits — so this is what happens when the user opens it, thinks
        // better of it, and clicks away.
        assert_eq!(
            rename_outcome("api", "api"),
            Rename::Unchanged,
            "an untouched box asked for nothing"
        );
        assert_eq!(
            rename_outcome("  api  ", "api"),
            Rename::Unchanged,
            "whitespace either side is not an edit"
        );
    }

    #[test]
    fn an_emptied_rename_box_gives_the_tab_back_to_its_pane() {
        // The only way to remove a name once set, so it has to survive the
        // comparison above.
        assert_eq!(rename_outcome("", "api"), Rename::Cleared);
        assert_eq!(rename_outcome("   ", "api"), Rename::Cleared);
    }

    #[test]
    fn a_typed_rename_box_names_the_tab() {
        assert_eq!(
            rename_outcome("billing", "api"),
            Rename::Named("billing".into())
        );
        assert_eq!(
            rename_outcome("  billing  ", "api"),
            Rename::Named("billing".into()),
            "the name is stored trimmed"
        );
    }

    const SIDEBAR_MIN: f32 = crate::ui::workspace::tab_sidebar::MIN_SIDEBAR_WIDTH;
    const PANEL_MIN: f32 = crate::ui::files::right_panel::MIN_WIDTH;

    /// #738: which pane a directional move came from is remembered against the
    /// pane it landed on, not against the direction alone.
    ///
    /// One slot per direction is enough for a single move and back, but the
    /// second step of a walk overwrites the first: left off `3` onto `2` and
    /// left again onto `1` would leave only `1 -> 2`, and the second move back
    /// right — the one out of `2` — would be handed no origin and fall to the
    /// geometry that sent the user to the wrong pane in the first place.
    #[test]
    fn each_pane_remembers_the_move_that_landed_on_it() {
        fn id(n: u64) -> gpui::EntityId {
            gpui::EntityId::from(n)
        }
        let mut tab = Tab::new(Pane::Empty);
        let live = [id(1), id(2), id(3)];

        tab.remember_focus_origin(id(3), id(2), Dir::Left, &live);
        tab.remember_focus_origin(id(2), id(1), Dir::Left, &live);

        // Walking back retraces both steps rather than only the last one.
        assert_eq!(tab.focus_origin(id(1), Dir::Right), Some(id(2)));
        assert_eq!(tab.focus_origin(id(2), Dir::Right), Some(id(3)));
        // Nothing is claimed about a direction no move went in, or about a pane
        // no move has landed on.
        assert_eq!(tab.focus_origin(id(2), Dir::Left), None);
        assert_eq!(tab.focus_origin(id(3), Dir::Right), None);

        // A pane the tab no longer holds takes its entries with it, on either
        // side: with `3` closed, `2` no longer remembers having come from it,
        // and the move back out of `2` is left to geometry.
        tab.remember_focus_origin(id(1), id(2), Dir::Right, &[id(1), id(2)]);
        assert_eq!(tab.focus_origin(id(2), Dir::Right), None);
        assert_eq!(tab.focus_origin(id(2), Dir::Left), Some(id(1)));
    }

    /// #679: the band now starts at the frame padding rather than at the
    /// window's corner, and off Linux CSD there is no padding to start at —
    /// `window_paddings` answers `Edges::all(0)` under server-side decorations,
    /// which is what macOS, Windows and a bare X session all report.
    #[test]
    fn an_undecorated_frame_leaves_the_strips_drop_band_where_it_was() {
        let band = strip_band(size(px(1200.), px(800.)), Edges::all(px(0.)));
        assert_eq!(band.origin, point(px(0.), px(0.)));
        assert_eq!(band.size, size(px(1200.), px(TITLE_BAR_HEIGHT)));
    }

    /// The viewport measures the whole surface, shadow included, so the band
    /// has to lose one padding at each end — not one twice over, and not none.
    /// Getting it wrong puts the outermost chips outside the band drawn to hold
    /// them, and a drop on them reads as a drop on nothing.
    #[test]
    fn a_client_side_frame_pulls_the_drop_band_in_by_the_shadow_on_both_sides() {
        let viewport = size(px(1200.), px(800.));
        let pad = Edges::all(px(12.));
        let band = strip_band(viewport, pad);

        assert_eq!(band.origin, point(pad.left, pad.top));
        assert_eq!(
            band.origin.x + band.size.width,
            viewport.width - pad.right,
            "the band must reach the far edge of the frame, not of the surface"
        );
        assert_eq!(band.size.height, px(TITLE_BAR_HEIGHT));
    }

    /// Fullscreen takes the window buttons and nothing else: the strip keeps
    /// its row, and the room reserved for the buttons at its end comes back.
    /// macOS never had any to take.
    #[test]
    fn fullscreen_drops_the_window_buttons_but_not_their_row() {
        assert!(!super::super::window_controls_drawn(true));
        assert_eq!(super::super::window_controls_w(true), 0.);
        assert_eq!(
            super::super::window_controls_drawn(false),
            !cfg!(target_os = "macos")
        );
        assert_eq!(super::super::window_controls_w(false), super::super::WINDOW_CONTROLS_W);
        assert_eq!(
            crate::ui::workspace::tab_strip::trailing_chrome_w(true),
            crate::ui::workspace::tab_strip::trailing_chrome_tiles_w()
        );
    }

    /// A surface narrower than its own shadow is only reachable mid-resize, but
    /// a negative width would make `Bounds::contains` answer for a rectangle
    /// that is inside out.
    #[test]
    fn a_drop_band_narrower_than_its_frame_collapses_instead_of_inverting() {
        let band = strip_band(size(px(10.), px(800.)), Edges::all(px(12.)));
        assert_eq!(band.size.width, px(0.));
    }

    /// Two panels that each cap themselves at half the window leave the
    /// terminal nothing when both are open, so the cap is what is left after
    /// the terminal's floor and the *other* panel's floor — or half the window,
    /// whichever binds harder.
    #[test]
    fn the_terminal_is_spoken_for_before_either_panel_is() {
        // Mid width: the reservation binds, and a panel dragged to it with the
        // other at its floor leaves the terminal exactly its floor. Two halves
        // left it nothing.
        let mid = 900.;
        let sidebar = side_panel_max(mid, SIDEBAR_MIN, PANEL_MIN);
        assert_eq!(sidebar, mid - TERMINAL_MIN_W - PANEL_MIN);
        assert_eq!(mid - sidebar - PANEL_MIN, TERMINAL_MIN_W);

        // With nothing on the other side, half the window already leaves the
        // terminal the other half, so the older cap is the one that binds and
        // a lone panel behaves exactly as it always did.
        assert_eq!(side_panel_max(mid, SIDEBAR_MIN, 0.), mid / 2.);
    }

    /// A docked document is a third column in the same budget, so it has to be
    /// reserved by the two panels the way they already reserve each other —
    /// otherwise a panel dragged to its old limit takes the width out of the
    /// document, which then has nowhere to take it from but the terminal.
    #[test]
    fn a_docked_document_is_reserved_by_the_panels_too() {
        let wide = 1440.;
        let max = side_panel_max(wide, PANEL_MIN, SIDEBAR_MIN + DOCUMENT_MIN_W);
        assert_eq!(
            wide - SIDEBAR_MIN - DOCUMENT_MIN_W - max,
            TERMINAL_MIN_W,
            "a panel at its cap, with both other columns at their floors,              leaves the terminal exactly its floor"
        );
        assert!(
            max < side_panel_max(wide, PANEL_MIN, SIDEBAR_MIN),
            "the reservation only ever takes width away"
        );
    }

    /// The floors are not what the panels are actually drawn at. Both are
    /// draggable and both persist, so the budget has to be fed the live widths
    /// or a widened sidebar is width the terminal silently loses.
    #[test]
    fn widened_panels_still_leave_the_terminal_its_floor() {
        let viewport = 1440.;
        // Both dragged well past their floors, and the document asked for the
        // widest named share there is.
        let body = viewport - 400. - 320.;
        let document = document_column_px(body, 2. / 3.).expect("720 points seats both");
        assert!(
            body - document >= TERMINAL_MIN_W,
            "terminal got {}",
            body - document
        );

        // Squeezed further, the document is the one that gives up first — and
        // then stops existing rather than dropping under its own floor.
        let squeezed = viewport - 600. - 400.;
        assert_eq!(document_column_px(squeezed, 0.5), None);
    }

    /// The reservation must only ever take width away from a panel. On a wide
    /// window it works out *larger* than the half-window cap that was already
    /// there, and letting it win would widen the ceiling instead.
    #[test]
    fn a_wide_window_still_stops_a_panel_at_half_of_it() {
        let wide = 1440.;
        assert!(
            wide - TERMINAL_MIN_W - PANEL_MIN > wide / 2.,
            "otherwise this test is not testing the case it names"
        );
        assert_eq!(side_panel_max(wide, SIDEBAR_MIN, PANEL_MIN), wide / 2.);
        assert_eq!(side_panel_max(wide, PANEL_MIN, SIDEBAR_MIN), wide / 2.);
    }

    /// Below the width where everything fits, the floor wins over the
    /// reservation: a cap under a panel's own minimum would be a panel drawn
    /// narrower than it can be read at, and the terminal — which can reflow —
    /// takes the shortfall instead.
    #[test]
    fn a_window_too_narrow_for_all_three_falls_back_to_the_floors() {
        let narrow = 720.;
        assert_eq!(side_panel_max(narrow, SIDEBAR_MIN, PANEL_MIN), SIDEBAR_MIN);
        assert_eq!(side_panel_max(narrow, PANEL_MIN, SIDEBAR_MIN), PANEL_MIN);
        // Both panels pinned to their floors leaves the terminal the rest —
        // less than its floor, but more than the 260-odd points two saved
        // widths used to leave it.
        assert!(narrow - SIDEBAR_MIN - PANEL_MIN >= 300.);
    }

    #[test]
    fn a_start_in_path_saves_only_when_it_names_a_real_directory() {
        // Empty is "unset", not a broken path.
        assert!(wd_path_saveable(""));
        assert!(wd_path_saveable("   "));
        let real = std::env::temp_dir();
        let real = real.to_str().expect("temp dir is utf-8 here");
        assert!(wd_path_saveable(real), "{real} exists");
        assert!(
            wd_path_saveable(&format!("  {real}  ")),
            "the commit trims, so the check trims too"
        );
        assert!(!wd_path_saveable("/definitely/not/a/real/dir"));
        // A file is not a directory the shell can start in.
        let file = std::env::temp_dir().join("anterm-wd-saveable-probe");
        std::fs::write(&file, b"x").expect("write probe file");
        assert!(!wd_path_saveable(file.to_str().expect("utf-8")));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_close_question_names_what_it_is_about_to_end() {
        use crate::terminal::view::PaneBusy;
        anterm_i18n::set_locale("en");

        let build = CloseReason::Busy(PaneBusy::Command("cargo build".into()));
        // The command is in the body, not a generic "are you sure".
        let (title, body) = close_prompt(true, &build);
        assert!(body.contains("cargo build"), "{body}");
        assert!(!body.contains("{what}"), "the placeholder leaked: {body}");
        assert!(title.contains("tab"), "{title}");
        // Same reason, but the pane has siblings: the tab survives, so the
        // question must not claim otherwise.
        let (pane_title, _) = close_prompt(false, &build);
        assert!(pane_title.contains("pane"), "{pane_title}");
        assert_ne!(title, pane_title);

        let agent = CloseReason::Busy(PaneBusy::Agent("Claude Code"));
        let (_, body) = close_prompt(true, &agent);
        assert!(body.contains("Claude Code"), "{body}");
        assert!(!body.contains("{agent}"), "the placeholder leaked: {body}");

        // A live SSH connection keeps its own wording rather than being folded
        // into the busy copy.
        let (ssh_title, ssh_body) = close_prompt(true, &CloseReason::LiveSsh);
        assert_ne!(ssh_title, title);
        assert!(!ssh_body.is_empty());
    }

    /// `WhenBusy` is the question anterm always asked; `Always` adds one for an
    /// idle shell without hiding what a busy one is doing; `Never` drops the
    /// busy question — and none of them touches the SSH warning, which the
    /// user turned on separately.
    #[test]
    fn each_confirm_mode_asks_what_it_says_and_the_ssh_warning_always_stands() {
        use crate::core::config::ConfirmClose::{Always, Never, WhenBusy};
        use crate::terminal::view::PaneBusy;
        let build = || Some(PaneBusy::Command("cargo build".into()));

        assert_eq!(close_reason(WhenBusy, false, None), None);
        assert_eq!(
            close_reason(WhenBusy, false, build()),
            Some(CloseReason::Busy(PaneBusy::Command("cargo build".into())))
        );
        assert_eq!(close_reason(Always, false, None), Some(CloseReason::Idle));
        assert_eq!(
            close_reason(Always, false, build()),
            Some(CloseReason::Busy(PaneBusy::Command("cargo build".into())))
        );
        assert_eq!(close_reason(Never, false, build()), None);
        for mode in [Never, WhenBusy, Always] {
            assert_eq!(
                close_reason(mode, true, build()),
                Some(CloseReason::LiveSsh),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn the_idle_and_bulk_questions_say_what_goes() {
        anterm_i18n::set_locale("en");
        let (tab_title, body) = close_prompt(true, &CloseReason::Idle);
        assert!(tab_title.contains("tab"), "{tab_title}");
        assert!(!body.is_empty());
        let (pane_title, _) = close_prompt(false, &CloseReason::Idle);
        assert!(pane_title.contains("pane"), "{pane_title}");

        let (three, _) = close_prompt(true, &CloseReason::Tabs(3));
        assert_eq!(three, "Close 3 tabs?");
        let (one, _) = close_prompt(true, &CloseReason::Tabs(1));
        assert_eq!(one, "Close 1 tab?");
    }

    #[test]
    fn a_layout_that_came_back_short_says_how_short() {
        anterm_i18n::set_locale("en");
        for n in [1usize, 2, 9] {
            let text =
                anterm_i18n::t_plural(anterm_i18n::L10nKey::AppTabsNotRestored, n, &[]);
            assert!(text.contains(&n.to_string()), "{n}: {text}");
            assert!(!text.contains("{count}"), "the placeholder leaked: {text}");
        }
        // Singular is not "1 tabs".
        let one = anterm_i18n::t_plural(anterm_i18n::L10nKey::AppTabsNotRestored, 1, &[]);
        assert!(one.contains("1 tab "), "{one}");
    }

    #[test]
    fn non_windows_reset_preserves_the_synced_windows_backdrop() {
        let mut config = crate::core::config::Config::default();
        config.window_opacity = Some(0.8);
        config.window_blur = Some(true);
        config.window_backdrop = crate::core::config::WindowBackdrop::Mica;

        clear_window_override_values(&mut config, false);

        assert_eq!(config.window_opacity, None);
        assert_eq!(config.window_blur, None);
        assert_eq!(
            config.window_backdrop,
            crate::core::config::WindowBackdrop::Mica,
            "an inert synchronized backdrop is not a local override to reset"
        );
    }

    #[test]
    fn windows_reset_clears_the_local_backdrop_override() {
        let mut config = crate::core::config::Config::default();
        config.window_backdrop = crate::core::config::WindowBackdrop::Acrylic;

        clear_window_override_values(&mut config, true);

        assert_eq!(
            config.window_backdrop,
            crate::core::config::WindowBackdrop::Auto
        );
    }

    #[test]
    fn mru_puts_the_active_tab_first_and_the_last_one_used_behind_it() {
        // Tab 2 is active; 0 was used most recently before it, then 1.
        assert_eq!(mru_order(&[7, 4, 9], 2), vec![2, 0, 1]);
    }

    #[test]
    fn mru_leads_with_the_active_tab_even_before_its_stamp_lands() {
        assert_eq!(mru_order(&[5, 0, 3], 1), vec![1, 0, 2]);
    }

    #[test]
    fn mru_trails_never_activated_tabs_in_strip_order() {
        assert_eq!(mru_order(&[0, 6, 0, 0], 1), vec![1, 0, 2, 3]);
    }

    #[test]
    fn mru_of_a_windowless_workspace_is_empty() {
        assert!(mru_order(&[], 0).is_empty());
    }

    #[test]
    fn stepping_through_tabs_follows_the_strip_and_wraps() {
        let order = [0, 1, 2];
        assert_eq!(step_in_order(&order, 0, true), Some(1));
        assert_eq!(step_in_order(&order, 2, true), Some(0));
        assert_eq!(step_in_order(&order, 0, false), Some(2));
        assert_eq!(step_in_order(&order, 1, false), Some(0));
        // Pressing it again keeps going — this is not the MRU switcher, which
        // bounces between the last two tabs (#867).
        let mut at = 0;
        let seen: Vec<usize> = (0..4)
            .map(|_| {
                at = step_in_order(&order, at, true).unwrap();
                at
            })
            .collect();
        assert_eq!(seen, vec![1, 2, 0, 1]);
    }

    #[test]
    fn stepping_follows_the_grouped_sidebar_order_not_the_index() {
        // Sidebar groups reorder the rows: index 2 is shown second.
        let order = [0, 2, 1, 3];
        assert_eq!(step_in_order(&order, 0, true), Some(2));
        assert_eq!(step_in_order(&order, 2, true), Some(1));
        assert_eq!(step_in_order(&order, 3, true), Some(0));
        assert_eq!(step_in_order(&order, 0, false), Some(3));
    }

    #[test]
    fn stepping_has_nowhere_to_go_with_one_tab() {
        assert_eq!(step_in_order(&[], 0, true), None);
        assert_eq!(step_in_order(&[0], 0, true), None);
        assert_eq!(step_in_order(&[0], 0, false), None);
    }

    #[test]
    fn moving_a_tab_one_slot_puts_it_where_the_neighbour_was() {
        // A move, not a trade: the neighbour it crossed shifts one slot.
        assert_eq!(one_slot_move(&[0, 1, 2], 3, 1, true), Some(vec![0, 2, 1]));
        assert_eq!(one_slot_move(&[0, 1, 2], 3, 1, false), Some(vec![1, 0, 2]));
    }

    #[test]
    fn a_tab_moved_past_an_end_carries_the_whole_run_with_it() {
        // Through the wrap the tab lands at the far end and the run it
        // crossed shifts one slot — `[c, a, b]`, not `[c, b, a]`.
        assert_eq!(one_slot_move(&[0, 1, 2], 3, 2, true), Some(vec![2, 0, 1]));
        assert_eq!(one_slot_move(&[0, 1, 2], 3, 0, false), Some(vec![1, 2, 0]));
    }

    #[test]
    fn a_move_inside_a_sidebar_group_leaves_the_other_groups_alone() {
        // Tabs 0, 2 and 4 are one group, 1 and 3 another: only the group's
        // own slots are reassigned, and the wrap stays inside it.
        assert_eq!(
            one_slot_move(&[0, 2, 4], 5, 2, true),
            Some(vec![0, 1, 4, 3, 2])
        );
        assert_eq!(
            one_slot_move(&[0, 2, 4], 5, 0, false),
            Some(vec![2, 1, 4, 3, 0])
        );
        assert_eq!(
            one_slot_move(&[1, 3], 5, 3, true),
            Some(vec![0, 3, 2, 1, 4])
        );
    }

    #[test]
    fn a_move_with_nothing_to_move_is_refused() {
        assert_eq!(one_slot_move(&[], 0, 0, true), None);
        assert_eq!(one_slot_move(&[0], 3, 0, true), None);
        assert_eq!(one_slot_move(&[0, 1, 2], 3, 7, true), None);
    }

    #[test]
    fn a_saved_host_opened_from_a_local_pane_does_not_start_in_its_local_directory() {
        let local = Some(std::path::PathBuf::from("/Users/alice/dev/anterm"));
        assert_eq!(
            inherited_start_dir(OpenedFrom::FreshSshHost, local.clone(), false),
            None
        );
        let remote = Some(std::path::PathBuf::from("/home/alice/my_service"));
        assert_eq!(
            inherited_start_dir(OpenedFrom::FreshSshHost, remote, true),
            None,
            "another host's directory means nothing on the one being dialled"
        );
    }

    #[test]
    fn redialling_the_same_ssh_host_starts_in_the_far_directory_it_was_in() {
        let remote = Some(std::path::PathBuf::from("/home/alice/my_service"));
        assert_eq!(
            inherited_start_dir(OpenedFrom::SameSshHost, remote.clone(), true),
            remote
        );
        assert_eq!(
            inherited_start_dir(OpenedFrom::SameSshHost, None, true),
            None
        );
    }

    #[test]
    fn a_local_shell_inherits_only_a_local_directory() {
        let dir = Some(std::path::PathBuf::from("/work/repo"));
        assert_eq!(
            inherited_start_dir(OpenedFrom::LocalShell, dir.clone(), false),
            dir
        );
        assert_eq!(
            inherited_start_dir(OpenedFrom::LocalShell, dir, true),
            None,
            "a remote pane's directory is on the far host"
        );
    }

    #[test]
    fn a_native_ssh_pane_the_server_no_longer_has_is_dialled_again() {
        let mut alive = std::collections::HashSet::new();
        alive.insert(4u64);
        assert!(native_ssh_pane_alive(Some(&alive), 4));
        assert!(
            !native_ssh_pane_alive(Some(&alive), 9),
            "gone: dial, don't attach"
        );
        assert!(
            !native_ssh_pane_alive(None, 4),
            "no listing: dial, don't guess"
        );
    }

    #[test]
    fn a_fork_needs_a_command_an_id_and_a_local_pane() {
        let session = |fork_label, session_id: Option<&str>, remote| TabAgentSession {
            fork_label,
            session_id: session_id.map(str::to_string),
            remote,
        };
        assert!(session(Some("Fork Session"), Some("abc"), false).forkable());
        assert!(
            !session(None, Some("abc"), false).forkable(),
            "an agent with no fork command is never forkable"
        );
        assert!(
            !session(Some("Fork Session"), None, false).forkable(),
            "no session id yet — the hooks haven't reported one"
        );
        assert!(
            !session(Some("Fork Session"), Some("abc"), true).forkable(),
            "a remote pane would fork the wrong machine's session"
        );
    }

    #[gpui::test]
    fn a_connecting_pane_saves_the_agent_it_is_rebuilding(cx: &mut gpui::TestAppContext) {
        use crate::core::cli_agent::CLIAgent;
        use crate::core::session::SessionPane;
        use crate::ui::workspace::pane::{Pane, PaneSlot};
        use crate::ui::workspace::pending_pane::{PendingPane, PendingSpawn};
        use gpui::AppContext as _;

        cx.update(|cx| {
            let pending = cx.new(|cx| {
                PendingPane::new(
                    "build-box",
                    PendingSpawn {
                        working_directory: Some(std::path::PathBuf::from("/work")),
                        restore_pane: Some(7),
                        shell: None,
                        agent: Some(CLIAgent::Claude),
                        agent_session_id: Some("sid-abc".to_string()),
                        agent_launch_argv: Some(vec!["claude".to_string()]),
                        run_on_land: None,
                        font_size: 14.0,
                    },
                    cx,
                )
            });
            let saved = super::super::session_spawn::pane_to_session(&Pane::leaf(PaneSlot::Connecting(pending)), cx);
            let SessionPane::Leaf {
                pane_id,
                agent,
                agent_session_id,
                agent_launch_argv,
                ..
            } = saved
            else {
                panic!("a leaf saves as a leaf");
            };
            assert_eq!(pane_id, Some(7), "the id it is re-attaching to");
            assert_eq!(agent, Some(CLIAgent::Claude));
            assert_eq!(agent_session_id.as_deref(), Some("sid-abc"));
            assert_eq!(agent_launch_argv, Some(vec!["claude".to_string()]));
        });
    }

    #[test]
    fn parses_ssh_option_words_with_quotes() {
        assert_eq!(
            parse_ssh_option_words("-p 2222 -J 'jump host' -o \"User=dev\"").unwrap(),
            vec!["-p", "2222", "-J", "jump host", "-o", "User=dev"]
        );
    }

    #[test]
    fn rejects_unclosed_ssh_option_quote() {
        assert!(parse_ssh_option_words("-J 'jump").is_err());
    }

    #[test]
    fn parses_typed_connect_into_native_profile() {
        let p = parse_ssh_connect_input("ssh deploy@10.0.0.5:2222").unwrap();
        assert_eq!(p.profile.host, "10.0.0.5");
        assert_eq!(p.profile.user, "deploy");
        assert_eq!(p.profile.port, 2222);
        assert!(p.proxy_jump.is_none());
    }

    #[test]
    fn parses_typed_connect_flags_and_jump() {
        let p =
            parse_ssh_connect_input("ssh -p 2222 -l dev -i ~/.ssh/id_ed25519 -J 'jump host' host")
                .unwrap();
        assert_eq!(p.profile.host, "host");
        assert_eq!(p.profile.user, "dev");
        assert_eq!(p.profile.port, 2222);
        assert_eq!(
            p.profile.identity_files,
            vec!["~/.ssh/id_ed25519".to_string()]
        );
        assert_eq!(p.proxy_jump.as_deref(), Some("jump host"));

        let p = parse_ssh_connect_input("host -p2222 -o User=deploy -o Port=2200").unwrap();
        assert_eq!(p.profile.user, "deploy");
        assert_eq!(p.profile.port, 2200);
    }

    #[test]
    fn explicit_flags_override_target_userhost() {
        let p = parse_ssh_connect_input("ssh me@host:22 -l other -p 2200").unwrap();
        assert_eq!(p.profile.user, "other");
        assert_eq!(p.profile.port, 2200);
    }

    #[test]
    fn rejects_bad_typed_connect_lines() {
        assert!(parse_ssh_connect_input("ssh -p 2222").is_err());
        assert!(parse_ssh_connect_input("ssh dev uptime").is_err());
        assert!(parse_ssh_connect_input("ssh -- dev").is_err());
        assert!(parse_ssh_connect_input("ssh 'host").is_err());
        assert!(parse_ssh_connect_input("ssh host -p 0").is_err());
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    /// The invariant #551 is fixed with: what `build_shell_inputs` writes into
    /// the Arguments field must parse back to exactly the argv it came from, so
    /// that opening Settings and clicking away cannot rewrite `config.json`.
    #[test]
    fn shell_arguments_round_trip_between_field_text_and_argv() {
        for case in [
            &["-l"][..],
            &["-c", "echo hi"],
            // A space-join cannot even spell these three.
            &[""],
            &["don't"],
            &["--say", "it's \"quoted\""],
            // Backslashes are how a path is spelled on Windows, and the field
            // has to be able to carry one either bare or inside quotes.
            &[r"--dir", r"C:\Users\me"],
            &[r"--dir", r"C:\Program Files\anterm"],
            &["--tag", "#1"],
        ] {
            let argv = argv(case);
            let field_text = join_shell_args(&argv);
            assert_eq!(
                split_shell_args(&field_text).expect("anterm's own refill must parse"),
                argv,
                "round trip through {field_text:?}"
            );
        }
        // Nothing that does not need quoting acquires any: the everyday field
        // still reads exactly like the user typed it.
        assert_eq!(
            join_shell_args(&argv(&["-l", "--color=auto", r"C:\Users\me", "#1"])),
            r"-l --color=auto C:\Users\me #1"
        );
        assert_eq!(join_shell_args(&[]), "");
    }

    /// What the user types has to reach argv meaning what it says: quoted text
    /// is one argument, and nothing outside quotes is an escape or a comment.
    #[test]
    fn shell_arguments_split_the_way_the_field_promises() {
        assert_eq!(
            split_shell_args("--login -c \"echo hi\"").expect("balanced quotes parse"),
            argv(&["--login", "-c", "echo hi"])
        );
        assert_eq!(
            split_shell_args("-c 'echo  hi'").expect("single quotes parse"),
            argv(&["-c", "echo  hi"])
        );
        // A backslash outside quotes is a character, not an escape: there is no
        // shell between this field and argv, and POSIX rules would quietly turn
        // a Windows path into `C:Usersme`.
        assert_eq!(
            split_shell_args(r"--dir C:\Users\me").expect("a bare path parses"),
            argv(&["--dir", r"C:\Users\me"])
        );
        // `#` starts no comment either — swallowing the rest of the line is the
        // silent rewrite this change exists to stop.
        assert_eq!(
            split_shell_args("--tag #1 --verbose").expect("a hash parses"),
            argv(&["--tag", "#1", "--verbose"])
        );
        // Inside double quotes, and only there, `\"` and `\\` stand for
        // themselves — that is how `join_shell_args` spells an argument that
        // carries both kinds of quote.
        assert_eq!(
            split_shell_args("\"it's \\\"quoted\\\"\"").expect("escapes parse"),
            argv(&["it's \"quoted\""])
        );
        assert_eq!(
            split_shell_args("'' -l").expect("an empty argument parses"),
            argv(&["", "-l"])
        );
    }

    /// The other half of the #551 contract: a value that cannot become argv is
    /// refused, not shredded into fragments that happen to contain quotes.
    #[test]
    fn unbalanced_quotes_are_refused_not_shredded() {
        assert!(split_shell_args("-c \"echo hi").is_err());
        assert!(split_shell_args("--name 'anterm").is_err());
        assert!(split_shell_args("\"a\\\"").is_err());
    }
}

pub(crate) mod test_window {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

    pub(crate) fn harness(cx: &mut TestAppContext) -> (Entity<AntermApp>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();

        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.background_executor.run_until_parked();
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<AntermApp>()
                    .ok()
                    .expect("window root wraps a AntermApp")
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (app, vcx)
    }

    /// A window carrying `n` quiet tabs, active on the first.
    pub(crate) fn harness_with_tabs(
        cx: &mut TestAppContext,
        n: usize,
    ) -> (
        Entity<AntermApp>,
        VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::workspace::pane::{Pane, PaneSlot};

        let (app, mut vcx) = harness(cx);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.cursor_blink = false;
            cx.set_global(cfg);
        });
        let streams = app.update_in(&mut vcx, |app, window, cx| {
            let mut streams = Vec::new();
            for i in 0..n {
                let (view, stream) = quiet_test_pane(i as u64 + 1, window, cx);
                app.tabs
                    .push(super::super::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
                streams.push(stream);
            }
            app.active = 0;
            cx.notify();
            streams
        });
        vcx.background_executor.run_until_parked();
        (app, vcx, streams)
    }

    pub(crate) fn harness_with_pane(
        cx: &mut TestAppContext,
    ) -> (
        Entity<AntermApp>,
        VisualTestContext,
        crate::daemon::transport::Stream,
    ) {
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::workspace::pane::{Pane, PaneSlot};

        let (app, mut vcx) = harness(cx);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.cursor_blink = false;
            cx.set_global(cfg);
        });
        let stream = app.update_in(&mut vcx, |app, window, cx| {
            let (view, stream) = quiet_test_pane(1, window, cx);
            app.tabs
                .push(super::super::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            app.active = 0;
            cx.notify();
            stream
        });
        vcx.background_executor.run_until_parked();
        (app, vcx, stream)
    }
}

mod cursor_blink_gpui_tests {
    use super::test_window::harness;
    use crate::core::config::Config;
    use crate::terminal::view::quiet_test_pane;
    use crate::ui::workspace::pane::{Pane, PaneSlot};
    use gpui::TestAppContext;

    #[gpui::test]
    fn only_the_focused_pane_advances_its_blink_phase(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        vcx.update(|_, cx| {
            let mut cfg = cx.global::<Config>().clone();
            cfg.cursor_blink = true;
            cx.set_global(cfg);
        });

        let (left, right, _streams) = app.update_in(&mut vcx, |app, window, cx| {
            let (left, left_stream) = quiet_test_pane(1, window, cx);
            let (right, right_stream) = quiet_test_pane(2, window, cx);
            app.tabs.push(super::super::Tab::new(Pane::split_node(
                gpui::Axis::Horizontal,
                0.5,
                Pane::leaf(PaneSlot::Ready(left.clone())),
                Pane::leaf(PaneSlot::Ready(right.clone())),
            )));
            app.active = 0;
            let left_focus = left.read(cx).focus_handle.clone();
            left_focus.focus(window, cx);
            cx.notify();
            (left, right, (left_stream, right_stream))
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |_, window, cx| {
            assert!(left.read(cx).focus_handle.is_focused(window));
            assert!(!right.read(cx).focus_handle.is_focused(window));
            assert!(left.read(cx).cursor_visible());
            assert!(right.read(cx).cursor_visible());
        });

        vcx.executor()
            .advance_clock(std::time::Duration::from_millis(530));
        vcx.background_executor.run_until_parked();

        app.update(&mut vcx, |_, cx| {
            assert!(!left.read(cx).cursor_visible(), "the focused cursor blinks");
            assert!(
                right.read(cx).cursor_visible(),
                "the unfocused cursor must keep a steady phase"
            );
        });
    }
}

mod ssh_rebuild_gpui_tests {
    use super::test_window::harness_with_pane;
    use crate::ui::workspace::pane::{Pane, PaneSlot};
    use anterm_core::machine::{PaneNode, Tab as TreeTab};
    use gpui::TestAppContext;

    /// Two tabs on pane 1 and pane 2, with the window's sync primed on them the
    /// way a landed pull leaves it.
    fn primed_two_tab_window(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<super::super::AntermApp>,
        gpui::VisualTestContext,
        Vec<crate::daemon::transport::Stream>,
    ) {
        let (app, mut vcx, first) = harness_with_pane(cx);
        let second = app.update_in(&mut vcx, |app, window, cx| {
            let (view, stream) = crate::terminal::view::quiet_test_pane(2, window, cx);
            app.tabs
                .push(super::super::Tab::new(Pane::leaf(PaneSlot::Ready(view))));
            let tabs = app
                .tabs
                .iter()
                .zip([1u64, 2])
                .map(|(tab, pane)| TreeTab {
                    id: tab.tree_id.get(),
                    name: None,
                    tags: Vec::new(),
                    root: PaneNode::Leaf { pane },
                    hibernated: false,
                })
                .collect();
            crate::ui::remote::tree_sync::prime_for_test(cx, tabs);
            stream
        });
        (app, vcx, vec![first, second])
    }

    fn queued_closes(cx: &mut gpui::App) -> Vec<anterm_core::machine::TabId> {
        use anterm_core::daemon::control::ControlRequest;
        crate::ui::remote::tree_sync::queued_for_test(cx)
            .into_iter()
            .filter_map(|op| match op {
                ControlRequest::TabClose { tab, .. }
                | ControlRequest::TabCloseRemembered { tab, .. } => Some(tab),
                _ => None,
            })
            .collect()
    }

    /// Restart Server empties the window and waits out the handoff, and every
    /// delta that lands meanwhile syncs the emptied window. A window missing
    /// its tabs is not a user closing them.
    #[gpui::test]
    fn emptying_the_window_closes_nothing_on_the_machine(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = primed_two_tab_window(cx);
        let closes = app.update_in(&mut vcx, |app, _, cx| {
            app.tabs.clear();
            app.save_session(cx);
            queued_closes(cx)
        });
        assert!(
            closes.is_empty(),
            "this is the regression: Restart Server closed all tabs, \
             unremembered, and left their shells running with no tab to hold them"
        );
    }

    #[gpui::test]
    fn closing_a_tab_closes_it_and_only_it_on_the_machine(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = primed_two_tab_window(cx);
        let (closed, closes) = app.update_in(&mut vcx, |app, window, cx| {
            let closed = app.tabs[1].tree_id.get();
            app.close_tab_inner(1, true, window, cx);
            (closed, queued_closes(cx))
        });
        assert_eq!(closes, vec![closed]);
    }
}

mod restart_server_gpui_tests {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use gpui::{AppContext, TestAppContext};

    /// Clicking Restart Server made the whole app disappear.
    ///
    /// The work that puts the window back together after the restart ends by
    /// rebuilding every local window from the machine tree, and the first thing
    /// that rebuild asks each window is which tabs it is showing — which it
    /// reads back out of the window registry. Run from inside `update_in` on
    /// this window's own entity, the first window it reaches for is the one the
    /// closure already holds leased, and gpui answers a double lease by
    /// panicking, which on the main thread is the process.
    ///
    /// Driven through `settle_after_restart` with a restart that "succeeded",
    /// because the crash is in the part that runs either way, not in the
    /// restart itself.
    #[gpui::test]
    async fn settling_after_a_restart_does_not_lease_the_window_twice(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
            crate::ui::workspace::windows::WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<AntermApp>()
                    .ok()
                    .expect("window root wraps a AntermApp")
            })
            .unwrap();

        // Registered the way an opened window registers itself: without this
        // the rebuild finds no window to ask and never reaches for the entity,
        // which is the whole thing under test.
        let handle = window.into();
        let weak = app.downgrade();
        app.update(cx, |_, cx| {
            crate::ui::workspace::windows::WindowRegistry::register(cx, handle, weak);
        });

        AntermApp::settle_after_restart(app.downgrade(), Ok(()), &mut cx.to_async()).await;

        assert!(
            app.update(cx, |app, _| app.startup_error.is_none()),
            "a restart reported as successful must not leave an error banner"
        );
    }
}

mod shell_menu_gpui_tests {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

    fn harness(cx: &mut TestAppContext) -> (Entity<AntermApp>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
            crate::ui::workspace::windows::WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app = cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<AntermApp>()
                    .ok()
                    .expect("window root wraps a AntermApp")
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (app, vcx)
    }

    fn pump_until(
        app: &Entity<AntermApp>,
        vcx: &mut VisualTestContext,
        done: impl Fn(&AntermApp) -> bool,
    ) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            vcx.background_executor.run_until_parked();
            if app.update(vcx, |app, _| done(app)) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[gpui::test]
    fn a_local_window_lists_this_computers_shells(cx: &mut TestAppContext) {
        let (app, mut vcx) = harness(cx);
        assert!(
            pump_until(&app, &mut vcx, |app| !app.shells.shells.is_empty()),
            "the local probe never landed"
        );
        app.update(&mut vcx, |app, _| {
            assert!(
                !app.shells.default_name.is_empty(),
                "the menu has no default to tag"
            );
        });
    }
}

mod rename_gpui_tests {
    use gpui::TestAppContext;

    use crate::ui::workspace::app::test_window::harness_with_tabs;

    #[gpui::test]
    fn a_rename_box_opens_with_the_caret_after_the_name(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        app.update_in(&mut vcx, |app, window, cx| app.start_rename(0, window, cx));
        vcx.background_executor.run_until_parked();

        app.update(&mut vcx, |app, cx| {
            let input = app
                .renaming
                .as_ref()
                .expect("the rename box is up")
                .input
                .clone();
            let state = input.read(cx);
            let value = state.value().to_string();
            assert!(!value.is_empty(), "the box starts on the current name");
            let end = value.len();
            assert_eq!(
                state.selected_range(),
                end..end,
                "typing has to continue {value:?}, not land in front of it"
            );
        });
    }
    #[gpui::test]
    fn a_rename_rides_out_other_tabs_closing_and_the_strip_reordering(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        app.update_in(&mut vcx, |app, window, cx| {
            app.start_rename(2, window, cx);
            let target = app.tabs[2].tree_id.get();
            let input = app.renaming.as_ref().expect("the box is up").input.clone();
            input.update(cx, |s, cx| s.set_value("mine", window, cx));

            // An unrelated close used to throw the half-typed name away
            // (#598).
            app.close_tab_inner(0, true, window, cx);
            assert!(
                app.renaming.is_some(),
                "closing another tab keeps the rename box"
            );

            // So did a drag-reorder — and the index the commit once landed
            // on had by then drifted onto a different tab.
            let order: Vec<usize> = (0..app.tabs.len()).rev().collect();
            app.apply_tab_order(&order, cx);
            assert!(app.renaming.is_some(), "a reorder keeps the rename box");

            app.commit_rename(window, cx);
            let named: Vec<_> = app
                .tabs
                .iter()
                .filter(|t| t.name.as_deref() == Some("mine"))
                .collect();
            assert_eq!(named.len(), 1, "the name landed on exactly one tab");
            assert_eq!(
                named[0].tree_id.get(),
                target,
                "and that tab is the one the box was opened on"
            );
        });

        // Closing the renaming tab itself still ends the rename.
        app.update_in(&mut vcx, |app, window, cx| {
            app.start_rename(0, window, cx);
            assert!(app.renaming.is_some());
            app.close_tab_inner(0, true, window, cx);
            assert!(app.renaming.is_none(), "losing its own tab closes the box");
        });
    }
}

// The output gate: a pane repaints on PTY output only while it is on screen.
// `AntermApp::render` declares the active tab's panes displayed each frame and
// everything else hidden; a pane nobody has declared — or whose id nobody
// registered — must err toward displayed, because the failure direction that
// matters is a visible pane that stops repainting.
mod displayed_gpui_tests {
    use gpui::TestAppContext;

    use crate::terminal::view::{declare_displayed, displayed_for_test, quiet_test_pane};
    use crate::ui::workspace::app::test_window::harness_with_tabs;

    fn pane_id(app: &super::super::AntermApp, tab: usize) -> gpui::EntityId {
        app.tabs[tab]
            .pane
            .first_leaf()
            .expect("tab has a pane")
            .entity_id()
    }

    #[gpui::test]
    fn the_output_gate_follows_the_active_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            let (front, back) = (pane_id(app, 0), pane_id(app, 1));

            app.declare_displayed_panes(cx);
            assert_eq!(
                displayed_for_test(cx, front),
                Some(true),
                "the active tab's pane repaints on output"
            );
            assert_eq!(
                displayed_for_test(cx, back),
                Some(false),
                "a background tab's pane stays out of the frame loop"
            );

            app.activate(1, window, cx);
            app.declare_displayed_panes(cx);
            assert_eq!(displayed_for_test(cx, front), Some(false));
            assert_eq!(
                displayed_for_test(cx, back),
                Some(true),
                "switching tabs hands the frame loop to the new tab"
            );
        });
    }

    #[gpui::test]
    fn maximized_split_hides_non_maximized_sibling_from_output_gate(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let _extra = app.update_in(&mut vcx, |app, window, cx| {
            let first = app.tabs[0]
                .pane
                .first_leaf()
                .and_then(|s| s.terminal().cloned())
                .expect("first pane");
            let (second, second_stream) = quiet_test_pane(42, window, cx);
            app.tabs[0].pane.split_leaf(
                first.entity_id(),
                gpui::Axis::Horizontal,
                false,
                crate::ui::workspace::pane::PaneSlot::Ready(second.clone()),
            );

            app.declare_displayed_panes(cx);
            assert_eq!(displayed_for_test(cx, first.entity_id()), Some(true));
            assert_eq!(displayed_for_test(cx, second.entity_id()), Some(true));

            app.maximized = Some(first.clone());
            app.declare_displayed_panes(cx);
            assert_eq!(displayed_for_test(cx, first.entity_id()), Some(true));
            assert_eq!(
                displayed_for_test(cx, second.entity_id()),
                Some(false),
                "sibling hidden under maximized split must not repaint on output"
            );

            second_stream
        });
    }

    #[gpui::test]
    fn terminal_canvas_output_skips_cached_sidebar_and_tab_strip(cx: &mut TestAppContext) {
        use super::super::chrome_render_probe;

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);
        // Settle the startup shell probe (which runs on an OS thread and notifies
        // `AntermApp` once when finished) and the initial draw so both cached
        // chrome hosts populate their cache before we measure canvas-only frames.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app.read_with(&vcx, |app, _| !app.shells.shells.is_empty())
            && std::time::Instant::now() < deadline
        {
            vcx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        vcx.run_until_parked();

        chrome_render_probe::reset();
        let active_pane = app.update_in(&mut vcx, |app, _, _| {
            app.tabs[0]
                .pane
                .first_leaf()
                .and_then(|s| s.terminal().cloned())
                .expect("active pane")
        });

        // Simulate 8 high-frequency agent output frames notifying only the canvas.
        for _ in 0..8 {
            active_pane.update(&mut vcx, |view, cx| view.notify_canvas(cx));
            vcx.run_until_parked();
        }

        assert_eq!(
            chrome_render_probe::sidebar_renders(),
            0,
            "cached sidebar must not re-render on pure terminal canvas output"
        );
        assert_eq!(
            chrome_render_probe::strip_renders(),
            0,
            "cached tab strip must not re-render on pure terminal canvas output"
        );

        // Metadata changes (title, agent status, etc.) must invalidate both chrome hosts.
        vcx.update(|_, cx| crate::terminal::view::notify_chrome_meta(cx));
        vcx.run_until_parked();
        assert!(
            chrome_render_probe::strip_renders() >= 1,
            "chrome metadata change must invalidate the cached tab strip"
        );
    }

    #[gpui::test]
    fn a_pane_nobody_declared_counts_as_displayed(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 1);

        let _orphan = app.update_in(&mut vcx, |app, window, cx| {
            // A pane that exists but sits in no tab — the shape of every
            // "path that never declares": it registers as displayed and a
            // declaration pass over the app's own tabs leaves it alone.
            let (view, stream) = quiet_test_pane(99, window, cx);
            let orphan = view.entity_id();
            assert_eq!(
                displayed_for_test(cx, orphan),
                Some(true),
                "a fresh pane defaults to displayed"
            );

            app.declare_displayed_panes(cx);
            assert_eq!(
                displayed_for_test(cx, orphan),
                Some(true),
                "declaring only speaks about panes the app holds"
            );

            // An id nobody registered is declared into the void, not
            // inserted: the registry only ever holds live panes' flags.
            let ghost = gpui::EntityId::from(u64::MAX);
            declare_displayed(cx, [(ghost, false)]);
            assert_eq!(displayed_for_test(cx, ghost), None);

            (view, stream)
        });
    }

    #[gpui::test]
    fn a_released_pane_leaves_the_registry(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        let back = app.update_in(&mut vcx, |app, _window, _cx| {
            let back = pane_id(app, 1);
            app.tabs.remove(1);
            back
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |_, _, cx| {
            assert_eq!(
                displayed_for_test(cx, back),
                None,
                "a closed pane's flag does not outlive it"
            );
        });
    }
}

// Zoom is a tab's view state: it rides with the tab across a switch, while a
// layout change (drag, split, close) still clears it.
mod zoom_gpui_tests {
    use gpui::TestAppContext;

    use crate::ui::workspace::app::test_window::harness_with_tabs;

    #[gpui::test]
    fn a_tabs_zoom_survives_a_round_trip_to_another_tab(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            let leaf = app.tabs[0]
                .pane
                .first_leaf()
                .and_then(|slot| slot.terminal().cloned())
                .expect("tab 0 has a pane");
            app.maximized = Some(leaf.clone());

            app.activate(1, window, cx);
            assert!(app.maximized.is_none(), "tab 1 never zoomed anything");

            app.activate(0, window, cx);
            assert_eq!(
                app.maximized.as_ref().map(|l| l.entity_id()),
                Some(leaf.entity_id()),
                "tab 0's zoom is still where it was left (#599)"
            );

            // A zoom stashed for a pane that is no longer in the tab does not
            // come back — it exited (or was closed) while the tab was away.
            app.maximized = Some(leaf.clone());
            app.activate(1, window, cx);
            app.tabs[0].zoomed = Some(leaf);
            app.tabs[0].pane =
                crate::ui::workspace::pane::Pane::leaf(app.tabs[1].pane.first_leaf().expect("a donor leaf"));
            app.activate(0, window, cx);
            assert!(
                app.maximized.is_none(),
                "a stashed zoom whose pane is gone stays gone"
            );
        });
    }

    /// The mark the chrome wears (#752) has to go out again by every road the
    /// zoom itself leaves by, and it has to name the right tab while several
    /// tabs are each holding one.
    #[gpui::test]
    fn the_zoom_mark_is_on_whichever_tabs_are_hiding_panes(cx: &mut TestAppContext) {
        use crate::terminal::view::quiet_test_pane;
        use crate::ui::workspace::pane::{Pane, PaneSlot};

        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        app.update_in(&mut vcx, |app, window, cx| {
            // A zoom only hides something where there is a sibling to hide, so
            // tabs 0 and 1 get a second pane and tab 2 stays single.
            let mut held = Vec::new();
            for tab in 0..2 {
                let (view, stream) = quiet_test_pane(90 + tab as u64, window, cx);
                held.push(stream);
                let first = app.tabs[tab].pane.first_leaf().expect("tab has a pane");
                app.tabs[tab].pane = Pane::split_node(
                    gpui::Axis::Horizontal,
                    0.5,
                    Pane::leaf(first),
                    Pane::leaf(PaneSlot::Ready(view)),
                );
            }

            for i in 0..app.tabs.len() {
                assert!(!app.tab_is_zoomed(i), "nothing is zoomed yet");
            }

            // Zooming marks the tab it happened in, and only that one.
            app.toggle_maximize(window, cx);
            assert!(app.tab_is_zoomed(0), "the zoomed tab wears the mark");
            assert!(!app.tab_is_zoomed(1));
            assert!(!app.tab_is_zoomed(2));

            // And un-zooming takes it away again.
            app.toggle_maximize(window, cx);
            assert!(!app.tab_is_zoomed(0), "un-zooming clears the mark");

            // The mark rides with its tab across a switch (#599) — an inactive
            // tab holding a zoom still wears it — and two tabs can wear one at
            // the same time, each reading its own handle.
            app.toggle_maximize(window, cx);
            app.activate(1, window, cx);
            assert!(app.tab_is_zoomed(0), "the parked zoom is still a zoom");
            assert!(!app.tab_is_zoomed(1));
            app.toggle_maximize(window, cx);
            assert!(app.tab_is_zoomed(0) && app.tab_is_zoomed(1));
            assert!(!app.tab_is_zoomed(2), "a single-pane tab hides nothing");

            // A parked zoom naming a pane that has since left the tab is no
            // zoom: it would not come back on a switch, so it is not marked.
            let parked = app.tabs[0].zoomed.clone().expect("tab 0 parked a zoom");
            let elsewhere = app.tabs[2]
                .pane
                .first_leaf()
                .and_then(|slot| slot.terminal().cloned());
            app.tabs[0].zoomed = elsewhere;
            assert!(
                !app.tab_is_zoomed(0),
                "a zoom over a pane this tab does not hold is not marked"
            );
            app.tabs[0].zoomed = Some(parked.clone());
            assert!(app.tab_is_zoomed(0));

            // Nor is a zoom over the last pane standing: its siblings closed
            // while the tab was away, and the tab now looks like — and draws
            // as — an ordinary single pane.
            app.tabs[0].pane = Pane::leaf(PaneSlot::Ready(parked));
            assert!(
                !app.tab_is_zoomed(0),
                "a zoom that covers nothing stops being marked"
            );

            assert!(!app.tab_is_zoomed(9), "there is no tab 9 to mark");
            drop(held);
        });
    }
}

mod new_window_action_tests {
    use crate::core::actions::NewWindow;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use crate::ui::workspace::windows::WindowRegistry;
    use gpui::{AppContext as _, TestAppContext, VisualTestContext};

    /// `NewWindow` has to open a window, not merely exist.
    ///
    /// Everything else about the action is a table entry — the `actions!`
    /// row, the keymap slot, the palette command — and every one of those can
    /// be there while the action reaches nothing. This drives the real
    /// dispatch path and then asks the registry, so the assertion is "a second
    /// window is open, on a workspace of its own, and the first one is still
    /// here": the same `windows::open` the switcher calls for "Open in New
    /// Window", with no workspace named.
    #[gpui::test]
    fn dispatching_new_window_opens_a_second_window_beside_the_first(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
            WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<AntermApp>()
                    .ok()
                    .expect("window root wraps a AntermApp")
            })
            .unwrap();
        // Registered the way an opened window registers itself; without it the
        // registry cannot tell the two windows apart afterwards.
        let handle = window.into();
        let weak = app.downgrade();
        app.update(cx, |_, cx| {
            WindowRegistry::register(cx, handle, weak);
        });

        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.background_executor.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| WindowRegistry::count(cx)),
            1,
            "the harness starts with exactly the one window"
        );

        vcx.dispatch_action(NewWindow);
        vcx.background_executor.run_until_parked();

        let open = vcx.update(|_, cx| WindowRegistry::open_windows(cx));
        assert_eq!(
            open.len(),
            2,
            "NewWindow has to reach windows::open; it opened {} window(s)",
            open.len()
        );
        assert!(
            open.iter().any(|(h, _)| *h == handle),
            "the window the action was fired from must survive it"
        );
        assert!(
            open.iter().any(|(h, _)| *h != handle),
            "the new window has its own window handle"
        );
    }

    /// The windowless state is the one `NewWindow` exists for.
    ///
    /// `show_tray_icon` is on by default, so closing the last window retires
    /// anterm to the tray rather than quitting it: the process is alive, the
    /// menu bar is still anterm's, and there is nothing on screen. A listener
    /// that lives only on `AntermApp`'s render root reaches nothing there, and
    /// the chord that means "give me a window" is the one chord that has to
    /// answer. `App::dispatch_action` falls through to the global listeners
    /// when no window is active, which is where `keymap::init` puts this one.
    #[gpui::test]
    fn new_window_answers_with_no_window_to_dispatch_it(cx: &mut TestAppContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
            WindowRegistry::init(cx);
            assert_eq!(
                WindowRegistry::count(cx),
                0,
                "the retired-to-tray state this covers has no window in it"
            );
            cx.dispatch_action(&NewWindow);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                WindowRegistry::count(cx),
                1,
                "NewWindow has to reach windows::open with no window to bubble through"
            );
        });
    }
}

mod unfocused_shortcut_tests {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use crate::ui::workspace::windows::WindowRegistry;
    use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext) -> (Entity<AntermApp>, VisualTestContext) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
            WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view().clone().downcast::<AntermApp>().ok().unwrap()
            })
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.run_until_parked();
        (app, vcx)
    }

    /// Focus on a handle no element tracks — a panel that closed under it, a
    /// field that went away — leaves gpui dispatching keys on the window root
    /// alone, and `AntermApp`'s listeners sit one level below that. The
    /// shortcuts have to answer anyway.
    fn answers_with_focus_off_the_tree(
        action: &str,
        opened: fn(&AntermApp) -> bool,
        blur: bool,
        cx: &mut TestAppContext,
    ) {
        let (app, mut vcx) = open(cx);
        let key = vcx
            .update(|_, cx| crate::ui::chrome::keymap::effective_key(action, cx))
            .unwrap();
        vcx.update(|window, cx| {
            if blur {
                window.blur(cx);
            } else {
                let orphan = cx.focus_handle();
                window.focus(&orphan, cx);
                std::mem::forget(orphan);
            }
            window.refresh();
        });
        vcx.run_until_parked();
        assert!(!app.read_with(&vcx, |app, _| opened(app)));
        vcx.simulate_keystrokes(&key);
        vcx.run_until_parked();
        assert!(
            app.read_with(&vcx, |app, _| opened(app)),
            "{action} ({key}) did nothing with focus off AntermApp's tree (blur: {blur})"
        );
    }

    #[gpui::test]
    fn palette_with_app_focus(cx: &mut TestAppContext) {
        let (app, mut vcx) = open(cx);
        let key = vcx
            .update(|_, cx| crate::ui::chrome::keymap::effective_key("TogglePalette", cx))
            .unwrap();
        app.update_in(&mut vcx, |app, window, cx| app.focus_active(window, cx));
        vcx.run_until_parked();
        vcx.simulate_keystrokes(&key);
        vcx.run_until_parked();
        assert!(app.read_with(&vcx, |app, _| app.search.is_some()));
    }

    #[gpui::test]
    fn palette_after_blur(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree("TogglePalette", |a| a.search.is_some(), true, cx);
    }

    #[gpui::test]
    fn palette_with_orphan_focus(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree("TogglePalette", |a| a.search.is_some(), false, cx);
    }

    #[gpui::test]
    fn settings_with_orphan_focus(cx: &mut TestAppContext) {
        answers_with_focus_off_the_tree(
            "OpenSettings",
            |a| a.settings_window.is_some(),
            false,
            cx,
        );
    }
}

mod close_window_action_tests {
    use crate::core::actions::CloseWindow;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use crate::ui::workspace::windows::WindowRegistry;
    use gpui::{AppContext as _, TestAppContext, VisualTestContext};

    /// `CloseWindow` has to close the window, and close it the way the red
    /// button does.
    ///
    /// The action is otherwise all table entries — the `actions!` row, the
    /// keymap slot, the palette command, the Keybindings label — and every one
    /// of those can be in place while the action reaches nothing at all. So
    /// this drives the real dispatch path and then asks two separate
    /// questions: the window is gone from gpui, *and* it left the
    /// `WindowRegistry` on the way out. The second is what makes it the same
    /// close as the native one — `detach_workspace` is where the session is
    /// saved and the workspace is retired, and a `remove_window` that skipped
    /// it would still pass the first assertion while quietly dropping a
    /// window's tabs on the floor.
    #[gpui::test]
    fn dispatching_close_window_takes_the_window_down_with_its_registration(
        cx: &mut TestAppContext,
    ) {
        crate::core::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            crate::ui::chrome::keymap::init(cx);
            WindowRegistry::init(cx);
        });
        let window = cx.add_window(|window, cx| {
            let app =
                cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
            gpui_kit::component::Root::new(app, window, cx)
        });
        let app = window
            .update(cx, |root, _, _| {
                root.view()
                    .clone()
                    .downcast::<AntermApp>()
                    .ok()
                    .expect("window root wraps a AntermApp")
            })
            .unwrap();
        // Registered the way an opened window registers itself; the registry
        // is where the close has to show up, so an unregistered window would
        // make the assertion below pass for the wrong reason.
        let handle = window.into();
        let weak = app.downgrade();
        app.update(cx, |_, cx| {
            WindowRegistry::register(cx, handle, weak);
        });

        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.background_executor.run_until_parked();
        assert_eq!(
            vcx.update(|_, cx| WindowRegistry::count(cx)),
            1,
            "the harness starts with exactly the one window"
        );

        vcx.dispatch_action(CloseWindow);
        drop(vcx);

        assert!(
            cx.update(|cx| cx.windows().is_empty()),
            "CloseWindow has to reach `remove_window`; the window is still open"
        );
        assert!(
            cx.update(|cx| WindowRegistry::open_windows(cx).is_empty()),
            "the close has to run the same cleanup the red button runs, \
             which is what takes the window out of the registry"
        );
    }
}

/// #843: which pane a tab comes back to.
mod tab_focus_memory_tests {
    use super::super::Tab;
    use super::super::session_spawn::{remember_leaf_in, replace_leaf_in};
    use crate::ui::workspace::pane::{Pane, PaneSlot};
    use crate::ui::workspace::pending_pane::{PendingPane, PendingSpawn};
    use gpui::{
        AppContext as _, Axis, Context, Entity, IntoElement, Render, Styled as _, TestAppContext,
        Window, div,
    };

    /// A window has to exist for focus to live in, but nothing this file asks
    /// is about what a pane paints.
    struct Blank;
    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// A leaf that owns a real focus handle without owning a shell. Focus
    /// tracking asks the slot, not the terminal behind it, so a connecting
    /// pane answers every question here exactly as a running one would — and
    /// connecting panes are watched for focus now too, so this is not a
    /// stand-in for the case under test but one of its cases.
    fn leaf(cx: &mut Context<Blank>) -> Entity<PendingPane> {
        cx.new(|cx| {
            PendingPane::new(
                "test",
                PendingSpawn {
                    working_directory: None,
                    restore_pane: None,
                    shell: None,
                    agent: None,
                    agent_session_id: None,
                    agent_launch_argv: None,
                    run_on_land: None,
                    font_size: 14.,
                },
                cx,
            )
        })
    }

    fn two_pane_tab(a: &Entity<PendingPane>, b: &Entity<PendingPane>) -> Tab {
        Tab::new(Pane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(PaneSlot::Connecting(a.clone())),
            Pane::Leaf(PaneSlot::Connecting(b.clone())),
        ))
    }

    fn one_pane_tab(a: &Entity<PendingPane>) -> Tab {
        Tab::new(Pane::Leaf(PaneSlot::Connecting(a.clone())))
    }

    /// The mechanism behind the report: `remember_active_pane`'s sample asks
    /// which leaf holds focus *at that instant*, and a switch begun with focus
    /// one handle away — the switcher's own search input, a palette closing,
    /// the tab strip — finds no leaf and has nothing to write. This is why the
    /// sample cannot be the only writer, and it is pinned here so that a change
    /// making `focused_leaf` tolerant would have to argue with a test rather
    /// than silently make this fix look unnecessary.
    #[gpui::test]
    fn a_switch_begun_off_the_panes_samples_nothing(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Blank);
        let (a, b, elsewhere) = window
            .update(cx, |_, _, cx| (leaf(cx), leaf(cx), cx.focus_handle()))
            .unwrap();
        let tab = two_pane_tab(&a, &b);

        window
            .update(cx, |_, window, cx| {
                let on_b = b.read(cx).focus_handle.clone();
                window.focus(&on_b, cx);
                assert_eq!(
                    tab.pane.focused_leaf(window, cx).map(|l| l.entity_id()),
                    Some(b.entity_id()),
                    "with focus in the pane the sample would have found it"
                );

                window.focus(&elsewhere, cx);
                assert!(
                    tab.pane.focused_leaf(window, cx).is_none(),
                    "one handle off the pane and the switch-away sample has \
                     nothing to write"
                );
            })
            .unwrap();
    }

    /// So focus-in writes instead, and the tab comes back to the pane focus
    /// was last in even though it had wandered off the panes before the switch
    /// ever started.
    #[gpui::test]
    fn a_tab_comes_back_to_the_pane_focus_was_last_in(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Blank);
        let (a, b) = window.update(cx, |_, _, cx| (leaf(cx), leaf(cx))).unwrap();
        let mut tabs = vec![two_pane_tab(&a, &b)];
        assert_eq!(
            tabs[0].focus_target().map(|l| l.entity_id()),
            Some(a.entity_id()),
            "a tab nobody has worked in yet still opens on its first leaf"
        );

        // The reader clicks into the right-hand pane: focus arrives, and that
        // is the moment the tab is told.
        remember_leaf_in(&mut tabs, b.entity_id());

        // Focus then leaves the panes — the switcher opens, a palette closes —
        // and the tab is switched away from. `remember_active_pane` finds no
        // focused leaf and writes nothing, which is now harmless.
        assert_eq!(
            tabs[0].focus_target().map(|l| l.entity_id()),
            Some(b.entity_id()),
            "#843: the tab has to come back to the pane the reader was in"
        );
    }

    /// A pane dragged into another tab is focused where it lands, so the
    /// arrival has to change that tab's memory and not the one it left — the
    /// reason the tab is found by the leaf rather than taken to be the active
    /// one.
    #[gpui::test]
    fn a_moved_pane_is_remembered_by_the_tab_that_holds_it_now(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Blank);
        let (a, b, c) = window
            .update(cx, |_, _, cx| (leaf(cx), leaf(cx), leaf(cx)))
            .unwrap();
        // Tab 0 is the active one and holds `a`; `b` and `c` live in tab 1.
        let mut tabs = vec![one_pane_tab(&a), two_pane_tab(&b, &c)];

        remember_leaf_in(&mut tabs, c.entity_id());

        assert_eq!(
            tabs[1].focus_target().map(|l| l.entity_id()),
            Some(c.entity_id()),
            "the tab holding the focused pane is the one that remembers it"
        );
        assert_eq!(
            tabs[0].focus_target().map(|l| l.entity_id()),
            Some(a.entity_id()),
            "and no other tab's memory is touched"
        );
    }

    /// A pane that arrives after its slot has gone — a spawn landing on a
    /// closed tab, a leaf killed mid-flight — belongs to no tab, and must not
    /// leave a memory behind for the first tab that happens to be looked at.
    #[gpui::test]
    fn a_leaf_no_tab_holds_is_recorded_nowhere(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Blank);
        let (a, b, gone) = window
            .update(cx, |_, _, cx| (leaf(cx), leaf(cx), leaf(cx)))
            .unwrap();
        let mut tabs = vec![two_pane_tab(&a, &b)];

        remember_leaf_in(&mut tabs, b.entity_id());
        remember_leaf_in(&mut tabs, gone.entity_id());

        assert_eq!(
            tabs[0].focus_target().map(|l| l.entity_id()),
            Some(b.entity_id()),
            "a stranger's arrival leaves the tab's own answer alone"
        );
    }

    /// A pane focused while it was still coming up is remembered under its
    /// *pending* slot, and that id dies the moment the pane lands in its
    /// place. The memory has to come along with the swap, or the landing is
    /// itself what puts the tab back on its first leaf.
    ///
    /// What lands here is another slot rather than a running pane: the swap has
    /// to move an id from one slot to another, and which kind of slot arrived
    /// is no part of the question.
    #[gpui::test]
    fn a_landing_pane_inherits_what_its_pending_slot_was_told(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| Blank);
        let (a, connecting, landed, other) = window
            .update(cx, |_, _, cx| (leaf(cx), leaf(cx), leaf(cx), leaf(cx)))
            .unwrap();
        let mut tabs = vec![two_pane_tab(&a, &connecting)];

        // Focus arrives while the pane is still connecting, then the pane it
        // was waiting for lands in that slot.
        remember_leaf_in(&mut tabs, connecting.entity_id());
        replace_leaf_in(
            &mut tabs,
            connecting.entity_id(),
            PaneSlot::Connecting(landed.clone()),
        );

        assert_eq!(
            tabs[0].focus_target().map(|l| l.entity_id()),
            Some(landed.entity_id()),
            "#843: the memory follows the pane, not the slot it arrived in"
        );

        // A landing somewhere else in the tab is not an answer to this
        // question and does not touch it.
        replace_leaf_in(&mut tabs, a.entity_id(), PaneSlot::Connecting(other));
        assert_eq!(
            tabs[0].focus_target().map(|l| l.entity_id()),
            Some(landed.entity_id()),
            "another pane landing leaves the tab's answer alone"
        );
    }

    /// The wiring, end to end. `watch_pane_focus` is the subscription that
    /// writes the record, and a real round trip through `activate` has to come
    /// back to the pane focus last arrived in — with focus off the panes well
    /// before the switch was made, which is the moment the switch-away sample
    /// cannot see (#843).
    #[gpui::test]
    fn a_tab_switch_returns_to_the_pane_focus_arrived_in(cx: &mut TestAppContext) {
        use super::super::session_spawn::watch_pane_focus;
        use super::test_window::harness;
        use crate::terminal::view::quiet_test_pane;

        let (app, mut vcx) = harness(cx);
        let (left, right, elsewhere, _held) = app.update_in(&mut vcx, |app, window, cx| {
            let (left, left_stream) = quiet_test_pane(1, window, cx);
            let (right, right_stream) = quiet_test_pane(2, window, cx);
            let (only, only_stream) = quiet_test_pane(3, window, cx);
            app.tabs.push(Tab::new(Pane::split_node(
                Axis::Horizontal,
                0.5,
                Pane::leaf(PaneSlot::Ready(left.clone())),
                Pane::leaf(PaneSlot::Ready(right.clone())),
            )));
            app.tabs.push(Tab::new(Pane::leaf(PaneSlot::Ready(only))));
            app.active = 0;
            // The subscription every spawn path registers for the pane it
            // built.
            for view in [&left, &right] {
                let handle = view.read(cx).focus_handle.clone();
                watch_pane_focus(&handle, view.entity_id(), window, cx);
            }
            cx.notify();
            (
                left,
                right,
                cx.focus_handle(),
                (left_stream, right_stream, only_stream),
            )
        });
        vcx.background_executor.run_until_parked();

        // The reader clicks into the right-hand pane.
        app.update_in(&mut vcx, |_, window, cx| {
            let handle = right.read(cx).focus_handle.clone();
            handle.focus(window, cx);
        });
        vcx.background_executor.run_until_parked();

        // Focus then leaves the panes altogether — a palette closing, the tab
        // strip, the switcher's own search input — before the tab is left.
        app.update_in(&mut vcx, |_, window, cx| elsewhere.focus(window, cx));
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| app.activate(1, window, cx));
        vcx.background_executor.run_until_parked();
        app.update_in(&mut vcx, |app, window, cx| app.activate(0, window, cx));
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |_, window, cx| {
            assert!(
                right.read(cx).focus_handle.is_focused(window),
                "#843: the tab has to come back to the pane the reader was in"
            );
            assert!(
                !left.read(cx).focus_handle.is_focused(window),
                "and not to the first leaf"
            );
        });
    }

    /// A split tab on screen with both panes watched the way every spawn path
    /// watches them, plus a handle off the panes for focus to wander to.
    #[allow(clippy::type_complexity)]
    fn watched_split(
        cx: &mut TestAppContext,
    ) -> (
        Entity<super::super::AntermApp>,
        gpui::VisualTestContext,
        Entity<crate::terminal::view::TerminalView>,
        Entity<crate::terminal::view::TerminalView>,
        gpui::FocusHandle,
        impl Sized,
    ) {
        use super::super::session_spawn::watch_pane_focus;
        use super::test_window::harness;
        use crate::terminal::view::quiet_test_pane;

        let (app, mut vcx) = harness(cx);
        let (left, right, elsewhere, held) = app.update_in(&mut vcx, |app, window, cx| {
            let (left, left_stream) = quiet_test_pane(1, window, cx);
            let (right, right_stream) = quiet_test_pane(2, window, cx);
            app.tabs.push(Tab::new(Pane::split_node(
                Axis::Horizontal,
                0.5,
                Pane::leaf(PaneSlot::Ready(left.clone())),
                Pane::leaf(PaneSlot::Ready(right.clone())),
            )));
            app.active = 0;
            for view in [&left, &right] {
                let handle = view.read(cx).focus_handle.clone();
                watch_pane_focus(&handle, view.entity_id(), window, cx);
            }
            cx.notify();
            (left, right, cx.focus_handle(), (left_stream, right_stream))
        });
        vcx.background_executor.run_until_parked();
        (app, vcx, left, right, elsewhere, held)
    }

    /// Zoom in on a pane, zoom back out, and the cursor is still in that pane
    /// (#869). The pane being un-zoomed is the answer outright — it is the one
    /// that was on screen to work in — so it is not a question for the tab's
    /// memory, which only focus-in writes and which nothing guarantees is
    /// current at the instant the toggle lands.
    #[gpui::test]
    fn unzooming_leaves_focus_in_the_pane_that_was_zoomed(cx: &mut TestAppContext) {
        let (app, mut vcx, left, right, _elsewhere, _held) = watched_split(cx);

        // Into the right-hand pane by keyboard, the road the report took.
        app.update_in(&mut vcx, |app, window, cx| {
            left.read(cx).focus_handle.clone().focus(window, cx);
            app.cycle_pane(true, window, cx);
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_maximize(window, cx);
            assert_eq!(
                app.maximized.as_ref().map(|l| l.entity_id()),
                Some(right.entity_id()),
                "the pane focus is in is the one zoomed"
            );
            // Whatever the tab's memory says while zoomed — here the stale
            // answer the report found — must not decide where un-zooming goes.
            app.tabs[0].last_focused = Some(left.entity_id());
            app.toggle_maximize(window, cx);
        });
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            assert!(app.maximized.is_none());
            assert!(
                right.read(cx).focus_handle.is_focused(window),
                "#869: un-zooming keeps focus in the pane that was zoomed"
            );
            assert_eq!(
                app.tabs[0].last_focused,
                Some(right.entity_id()),
                "and the tab remembers it, so a switch away and back agrees"
            );
        });
    }

    /// Zooming with focus off the panes — a palette just closed, the tab strip
    /// clicked — zooms the pane the tab remembers, not whichever leaf happens
    /// to be first: the same pane switching back to the tab would focus.
    #[gpui::test]
    fn zooming_from_off_the_panes_zooms_the_remembered_pane(cx: &mut TestAppContext) {
        let (app, mut vcx, _left, right, elsewhere, _held) = watched_split(cx);

        app.update_in(&mut vcx, |_, window, cx| {
            right.read(cx).focus_handle.clone().focus(window, cx);
        });
        vcx.background_executor.run_until_parked();
        app.update_in(&mut vcx, |_, window, cx| elsewhere.focus(window, cx));
        vcx.background_executor.run_until_parked();

        app.update_in(&mut vcx, |app, window, cx| {
            app.toggle_maximize(window, cx);
            assert_eq!(
                app.maximized.as_ref().map(|l| l.entity_id()),
                Some(right.entity_id()),
                "the zoom lands on the pane the reader was last in"
            );
            assert!(right.read(cx).focus_handle.is_focused(window));
        });
    }
}

mod hibernate_gpui_tests {
    use gpui::TestAppContext;

    use crate::core::session::SessionPane;
    use crate::ui::workspace::app::test_window::harness_with_tabs;
    use crate::ui::workspace::pane::Pane;

    fn leaf_pane_id(pane: &SessionPane) -> Option<u64> {
        match pane {
            SessionPane::Leaf { pane_id, .. } => *pane_id,
            SessionPane::Split { .. } => None,
        }
    }

    /// Asleep, a tab holds nothing running and still holds its place: the
    /// pane id the wake restores from, the name it had on screen, and a seat in
    /// the sync that tells the machine it is asleep rather than gone.
    #[gpui::test]
    fn a_sleeping_tab_keeps_its_place_its_pane_and_its_name(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            let pane_id = app.tabs[1]
                .pane
                .first_leaf()
                .and_then(|slot| slot.terminal().cloned())
                .expect("tab 1 has a pane")
                .read(cx)
                .pane_id;
            let (before, _) = app.tabs[1].label_view(Some(window), cx);

            app.put_to_sleep(1, window, cx);

            assert_eq!(app.tabs.len(), 2, "sleeping is not closing");
            let tab = &app.tabs[1];
            assert!(tab.is_asleep());
            assert!(matches!(tab.pane, Pane::Empty));
            assert!(tab.pane.terminals().is_empty(), "nothing is left running");
            assert_eq!(tab.asleep_layout().and_then(leaf_pane_id), Some(pane_id));
            let (after, _) = tab.label_view(Some(window), cx);
            assert_eq!(
                format!("{:?}", after.label()),
                format!("{:?}", before.label()),
                "it is called what it was called while awake"
            );
            assert!(!after.live);

            let (desired, _, held) = crate::ui::remote::tree_sync::desired_tabs(app, cx);
            assert!(held.is_empty(), "a sleeping tab is not a tab gone missing");
            assert_eq!(desired.len(), 2);
            assert!(!desired[0].hibernated);
            assert!(desired[1].hibernated);
            match &desired[1].root {
                crate::ui::remote::tree_sync::DesiredNode::Leaf { pane, .. } => assert_eq!(*pane, pane_id),
                other => panic!("expected the one leaf, got {other:?}"),
            }
        });
    }

    /// Looking at a tab is what wakes it, so the window has to keep one awake
    /// to look at; the tab shown in a sleeping one's place is the awake tab
    /// used last.
    #[gpui::test]
    fn the_last_awake_tab_has_nothing_to_hand_the_window_to(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 3);

        app.update_in(&mut vcx, |app, window, cx| {
            app.tabs[1].last_used.set(5);
            app.tabs[2].last_used.set(9);
            assert_eq!(app.awake_tab_besides(0), Some(2), "the one used last");

            app.put_to_sleep(2, window, cx);
            assert_eq!(app.awake_tab_besides(0), Some(1));

            app.put_to_sleep(1, window, cx);
            assert_eq!(app.awake_tab_besides(0), None);
            assert!(
                !app.can_hibernate_tab(0, cx),
                "the last awake tab stays awake"
            );
            assert!(
                !app.can_hibernate_tab(1, cx),
                "nor is a sleeping tab put to sleep twice"
            );
        });
    }

    /// Closing a sleeping tab closes it like any other, and what Reopen Closed
    /// Tab gets back is the layout it slept with — not the empty stand-in.
    #[gpui::test]
    fn a_sleeping_tab_closes_into_what_it_would_have_woken_as(cx: &mut TestAppContext) {
        let (app, mut vcx, _streams) = harness_with_tabs(cx, 2);

        app.update_in(&mut vcx, |app, window, cx| {
            app.put_to_sleep(1, window, cx);
            let layout_id = app.tabs[1].asleep_layout().and_then(leaf_pane_id);
            assert!(layout_id.is_some());

            app.close_tab(1, window, cx);

            assert_eq!(app.tabs.len(), 1);
            let closed = app.closed.last().expect("the closed tab is remembered");
            assert_eq!(leaf_pane_id(&closed.pane), layout_id);
            assert!(!closed.hibernated, "reopening brings it back awake");
        });
    }
}

#[gpui::test]
fn workspace_updates_do_not_redraw_settings_window(cx: &mut gpui::TestAppContext) {
    use super::AntermApp;
    use super::settings_bridge::build_settings_host;
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::chrome::settings::window::{SettingsWindow, render_probe};
    use gpui::{AppContext as _, VisualTestContext};

    crate::core::config::pin_test_config_dir();
    cx.executor().allow_parking();
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Config::default());
        crate::ui::chrome::presets::load_registry(cx);
        crate::ui::chrome::theme::apply_theme(None, cx);
        crate::ui::chrome::keymap::init(cx);
    });
    let mut app_slot = None;
    let ws_window = cx.add_window(|window, cx| {
        let app = cx.new(|cx| AntermApp::with_session(Some(Session::default()), window, cx));
        app_slot = Some(app.clone());
        gpui_kit::component::Root::new(app, window, cx)
    });
    let app = app_slot.unwrap();
    let _settings_window = cx.add_window(|window, cx| {
        let host = build_settings_host(&app, Some(ws_window.into()));
        let view = cx.new(|cx| SettingsWindow::new(Some(host), None, window, cx));
        gpui_kit::component::Root::new(view, window, cx)
    });
    cx.run_until_parked();
    let _ = render_probe::take();

    let mut ws_vcx = VisualTestContext::from_window(ws_window.into(), cx);
    for _ in 0..5 {
        app.update_in(&mut ws_vcx, |app, window, cx| {
            app.home_cursor_on = !app.home_cursor_on;
            window.refresh();
            cx.notify();
        });
        ws_vcx.run_until_parked();
    }
    assert_eq!(
        render_probe::take(),
        0,
        "workspace notifications must not redraw the autonomous SettingsWindow"
    );
}
