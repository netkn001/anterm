use gpui::{Context, ModifiersChangedEvent, Window};

use crate::ui::workspace::app::AntermApp;

const BADGE_DELAY_MS: u64 = 200;

/// Modifier-hold tab-number badge overlay state.
#[derive(Default)]
pub(crate) struct ModHintState {
    pub(crate) visible: bool,
    pub(crate) generation: u64,
}

pub(crate) fn tab_badge_label(index: usize) -> String {
    (index + 1).to_string()
}

impl AntermApp {
    pub(crate) fn on_modifiers_changed(
        &mut self,
        ev: &ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &ev.modifiers;
        self.set_link_modifier(crate::platform::is_secondary(m), cx);

        let hold = crate::ui::chrome::keymap::tab_badge_hold(cx);

        self.mod_hints.generation = self.mod_hints.generation.wrapping_add(1);
        if hold.as_ref() != Some(m) {
            self.dismiss_mod_hint(cx);
            return;
        }

        let generation = self.mod_hints.generation;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(BADGE_DELAY_MS)).await;
            let _ = this.update(cx, |this, cx| {
                if this.mod_hints.generation == generation {
                    this.mod_hints.visible = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn set_link_modifier(&mut self, down: bool, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            for leaf in tab.pane.terminals() {
                leaf.update(cx, |view, cx| {
                    view.refresh_link_hover(down, cx);
                });
            }
        }
    }

    pub(crate) fn dismiss_mod_hint(&mut self, cx: &mut Context<Self>) {
        self.mod_hints.generation = self.mod_hints.generation.wrapping_add(1);
        if self.mod_hints.visible {
            self.mod_hints.visible = false;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_badge_label_is_the_bare_digit() {
        assert_eq!(tab_badge_label(0), "1");
        assert_eq!(tab_badge_label(8), "9");
    }
}

#[cfg(test)]
mod gpui_tests {
    use crate::core::config::Config;
    use crate::core::session::Session;
    use crate::ui::workspace::app::AntermApp;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    fn harness(cx: &mut TestAppContext) -> (WindowHandle<AntermApp>, VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
        });
        let window =
            cx.add_window(|window, cx| AntermApp::with_session(Some(Session::default()), window, cx));
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.background_executor.run_until_parked();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (window, vcx)
    }

    fn badges_shown(window: &WindowHandle<AntermApp>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |app, _, _| app.mod_hints.visible)
            .expect("the app window stays open")
    }

    /// The hold the badges answer to, read from the same place they do rather
    /// than assumed — the shipped one is ⌘ on macOS and Alt off it.
    fn hold_modifiers(window: &WindowHandle<AntermApp>, cx: &mut TestAppContext) -> Modifiers {
        window
            .update(cx, |_, _, cx| crate::ui::chrome::keymap::tab_badge_hold(cx))
            .expect("the app window stays open")
            .expect("the shipped tab chords are on a modifier")
    }

    fn wait_for_badges(window: &WindowHandle<AntermApp>, cx: &mut TestAppContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.background_executor.run_until_parked();
            if badges_shown(window, cx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "badges never appeared within 5s of the held tab chord"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[gpui::test]
    fn deactivation_dismisses_visible_badges(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        let hold = hold_modifiers(&window, cx);
        vcx.simulate_modifiers_change(hold);
        wait_for_badges(&window, cx);

        vcx.deactivate_window();

        assert!(
            !badges_shown(&window, cx),
            "deactivation must dismiss the badges — the modifier release will never reach this window"
        );
    }

    #[gpui::test]
    fn deactivation_cancels_a_pending_reveal(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        let hold = hold_modifiers(&window, cx);
        vcx.simulate_modifiers_change(hold);
        vcx.deactivate_window();

        std::thread::sleep(std::time::Duration::from_millis(400));
        cx.background_executor.run_until_parked();

        assert!(
            !badges_shown(&window, cx),
            "a reveal scheduled before deactivation must not fire after it"
        );
    }

    /// The regression: the badges answered to the platform's secondary
    /// modifier rather than to the chord the numbers are drawn for. On Windows
    /// and Linux, where the nine ship on Alt, Ctrl was the only key that lit
    /// them — and a tab row rebound onto Alt, the case reported, raised nothing
    /// however long it was held.
    #[gpui::test]
    fn the_bound_modifier_raises_the_badges_and_another_one_does_not(cx: &mut TestAppContext) {
        let (window, mut vcx) = harness(cx);
        vcx.update(|_, cx| {
            // Exactly Alt+1…9, as the Settings page writes a rebind: a list,
            // so the shipped chord is gone rather than sitting beside it.
            cx.global_mut::<Config>().keybindings = serde_json::from_value(serde_json::json!({
                "ActivateTab1": ["alt-1"],
            }))
            .expect("the binding loads");
        });

        vcx.simulate_modifiers_change(Modifiers::control());
        std::thread::sleep(std::time::Duration::from_millis(400));
        cx.background_executor.run_until_parked();
        assert!(
            !badges_shown(&window, cx),
            "Ctrl is not the chord the tabs are numbered for here any more"
        );

        vcx.simulate_modifiers_change(Modifiers::none());
        vcx.simulate_modifiers_change(Modifiers::alt());
        wait_for_badges(&window, cx);
    }
}
