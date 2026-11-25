use super::info::{InfoRow, InfoValue, format_rtt, forwards_port};
use super::{MIN_WIDTH, RightPanelContext, RightPanelView};
use anterm_ui_kit::config::{Config, RightPanelTab};
use anterm_core::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind};

fn forward(kind: SshForwardKind, target_host: &str, target_port: u16) -> ManagedForward {
    ManagedForward {
        id: 1,
        pane_id: 7,
        kind,
        bind_host: "127.0.0.1".to_string(),
        bind_port: 51000,
        target_host: target_host.to_string(),
        target_port,
        description: None,
        status: ForwardStatus::Listening,
        enabled: true,
    }
}

#[test]
fn a_port_row_claims_only_the_forward_that_reaches_it() {
    assert!(forwards_port(
        &forward(SshForwardKind::Local, "localhost", 3000),
        3000
    ));
    assert!(
        forwards_port(&forward(SshForwardKind::Local, "127.0.0.1", 3000), 3000),
        "the far side's loopback spells itself several ways"
    );
    assert!(
        !forwards_port(&forward(SshForwardKind::Local, "localhost", 3000), 8080),
        "a different port is a different row"
    );
    assert!(
        !forwards_port(&forward(SshForwardKind::Local, "10.0.0.5", 3000), 3000),
        "same number, another machine — pairing them would claim it leads \
         somewhere it does not"
    );
    assert!(
        !forwards_port(&forward(SshForwardKind::Remote, "localhost", 3000), 3000),
        "a remote forward listens on the far side, so it is not how this \
         port is reached from here"
    );
}

#[test]
fn a_switched_off_forward_is_not_the_way_to_a_port() {
    let mut off = forward(SshForwardKind::Local, "localhost", 3000);
    off.enabled = false;
    assert!(!forwards_port(&off, 3000));
}

fn diff(added: u32, removed: u32, open: bool) -> InfoRow {
    InfoRow {
        label: "changes",
        value: InfoValue::Diff {
            added,
            removed,
            open: open.then(|| {
                (
                    anterm_ui_kit::host_ops::HostId::LOCAL,
                    std::path::PathBuf::from("/w/repo"),
                )
            }),
        },
        copy: None,
        reveal: None,
    }
}

#[test]
fn a_row_lights_up_only_when_there_is_something_behind_it() {
    assert!(
        !InfoRow::text("shell", "zsh".into()).interactive(),
        "a plain readout is not a control"
    );
    assert!(
        InfoRow::text("branch", "main".into())
            .copyable()
            .interactive(),
        "a copy tile is something to hover for"
    );
    assert!(
        InfoRow {
            reveal: Some(std::path::PathBuf::from("/w/repo")),
            ..InfoRow::text("cwd", "/w/repo".into())
        }
        .interactive(),
        "so is Reveal, even with nothing else on the row"
    );
}

#[test]
fn counts_are_a_button_only_when_there_is_a_diff_to_open() {
    assert!(
        diff(3, 1, true).interactive(),
        "changes with somewhere to go open the overlay"
    );
    assert!(
        !diff(0, 0, false).interactive(),
        "a clean tree is a readout, not a link"
    );
    assert!(
        !diff(3, 1, false).interactive(),
        "no target means no link, however dirty the tree"
    );
}

#[test]
fn a_round_trip_is_read_at_the_precision_it_is_worth() {
    use std::time::Duration;
    assert_eq!(format_rtt(Duration::from_micros(120)), "<1 ms");
    assert_eq!(format_rtt(Duration::from_micros(999)), "<1 ms");
    assert_eq!(format_rtt(Duration::from_millis(1)), "1 ms");
    assert_eq!(format_rtt(Duration::from_micros(23_400)), "23 ms");
    assert_eq!(format_rtt(Duration::from_millis(999)), "999 ms");
    assert_eq!(format_rtt(Duration::from_micros(999_600)), "1.0 s");
    assert_eq!(format_rtt(Duration::from_millis(1_450)), "1.4 s");
    assert_eq!(format_rtt(Duration::from_secs(4)), "4.0 s");
}

#[test]
fn copyable_takes_the_text_the_row_shows_and_nothing_else() {
    assert_eq!(
        InfoRow::text("ssh", "box".into())
            .copyable()
            .copy
            .as_deref(),
        Some("box")
    );
    assert_eq!(
        diff(3, 1, true).copyable().copy,
        None,
        "there is no sensible clipboard form of two coloured numbers"
    );
}

mod standalone_gpui_tests {
    use super::*;
    use gpui::{Entity, Render, TestAppContext, VisualTestContext, Window, prelude::*, px, size};
    use gpui_kit::component::{Root, v_flex};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct PanelHarness {
        panel: Entity<RightPanelView>,
    }

    impl Render for PanelHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            v_flex().size_full().child(self.panel.clone())
        }
    }

    fn mount_panel(cx: &mut TestAppContext) -> (Entity<RightPanelView>, VisualTestContext) {
        cx.executor().allow_parking();
        anterm_ui_kit::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::theme::apply_theme(None, cx);
        });
        let slot: Rc<RefCell<Option<Entity<RightPanelView>>>> = Rc::new(RefCell::new(None));
        let window = cx.open_window(size(px(960.), px(600.)), {
            let slot = slot.clone();
            move |window, cx| {
                let panel = cx.new(|cx| RightPanelView::new(window, cx));
                *slot.borrow_mut() = Some(panel.clone());
                let harness = cx.new(|_| PanelHarness { panel });
                Root::new(harness, window, cx)
            }
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        let panel = slot.borrow().clone().expect("panel entity");
        (panel, vcx)
    }

    #[gpui::test]
    fn switching_tabs_and_visibility_persists_and_clamps_width(cx: &mut TestAppContext) {
        let (panel, mut vcx) = mount_panel(cx);

        panel.update(&mut vcx, |rp, cx| {
            rp.sync_context(
                RightPanelContext {
                    has_tabs: true,
                    others_floor: 0.,
                    ..Default::default()
                },
                cx,
            );
            rp.set_tab(RightPanelTab::Scm, cx);
        });
        vcx.update(|window, cx| window.draw(cx).clear(cx));

        panel.read_with(&vcx, |rp, cx| {
            assert!(rp.visible);
            assert_eq!(rp.tab, RightPanelTab::Scm);
            assert_eq!(cx.global::<Config>().right_panel_tab, RightPanelTab::Scm);
            assert!(cx.global::<Config>().right_panel_visible);
        });

        // Search normalizes onto Files and renders all three tabs cleanly.
        for tab in [RightPanelTab::Search, RightPanelTab::Info, RightPanelTab::Files] {
            panel.update(&mut vcx, |rp, cx| rp.set_tab(tab, cx));
            vcx.update(|window, cx| window.draw(cx).clear(cx));
        }
        assert_eq!(panel.read_with(&vcx, |rp, _| rp.tab), RightPanelTab::Files);

        // Width clamping respects MIN_WIDTH floor and window ceiling.
        panel.update(&mut vcx, |rp, _| rp.set_width(40.));
        let clamped_low = vcx.update(|window, cx| panel.read(cx).width_px(0., window, cx));
        assert!(clamped_low >= MIN_WIDTH);

        panel.update(&mut vcx, |rp, _| rp.set_width(5000.));
        let clamped_high = vcx.update(|window, cx| panel.read(cx).width_px(0., window, cx));
        assert!(clamped_high < 960.);

        panel.update(&mut vcx, |rp, cx| rp.toggle_visible(cx));
        assert!(!panel.read_with(&vcx, |rp, _| rp.visible));
        assert!(!vcx.update(|_, cx| cx.global::<Config>().right_panel_visible));
    }
}
