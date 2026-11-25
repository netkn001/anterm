//! DEC mode 2031, colour palette update notifications: a program that switches
//! it on (`CSI ? 2031 h`) is sent `CSI ? 997 ; 1 n` (dark) or `; 2 n` (light)
//! whenever the theme's background changes, and `CSI ? 996 n` asks for the
//! same report. Claude Code's `theme: auto` depends on it: it reads OSC 11 once
//! at startup and again only when a 997 arrives.
//! <https://contour-terminal.org/vt-extensions/color-palette-update-notifications/>

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alacritty_terminal::event::Event as AlacEvent;
use alacritty_terminal::vte::ansi::Rgb;
use gpui::{App, Context};
use gpui_kit::component::ActiveTheme;
use anterm_core::term_modes::{COLOR_SCHEME_UPDATES, TerminalModes};

use crate::view::TerminalView;
use anterm_ui_kit::presets::is_dark;

/// The report for a background, dark or light by the same test the theme
/// presets use.
pub fn report(bg: Rgb) -> String {
    let rgb = u32::from(bg.r) << 16 | u32::from(bg.g) << 8 | u32::from(bg.b);
    format!("\x1b[?997;{}n", if is_dark(rgb) { 1 } else { 2 })
}

/// Fold pane output into `modes` and publish whether 2031 is on.
pub fn fold(modes: &mut TerminalModes, bytes: &[u8], on: &AtomicBool) {
    modes.feed(bytes);
    on.store(modes.is_on(COLOR_SCHEME_UPDATES), Ordering::Relaxed);
}

/// The answer to a `CSI ? 996 n`. Routed as a background colour request so
/// the view answers it from the theme it is drawing now, and a replay drops it
/// like every other reply.
pub fn query_reply() -> AlacEvent {
    AlacEvent::ColorRequest(257, Arc::new(report))
}

fn background(cx: &App) -> Rgb {
    super::palette::hsla_to_rgb(cx.theme().background)
}

/// Push a report to the pane each time the theme's background changes while
/// its program has 2031 on. Every path to a new theme (a preset pick, a
/// follow-system flip, a config reload) ends in the `Theme` global.
pub fn watch(cx: &mut Context<TerminalView>) {
    let mut last = background(cx);
    cx.observe_global::<gpui_kit::component::Theme>(move |view, cx| {
        let now = background(cx);
        if std::mem::replace(&mut last, now) != now && view.terminal.color_scheme_updates() {
            view.terminal.write(report(now).into_bytes());
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_background_reports_dark_and_a_light_one_light() {
        assert_eq!(report(Rgb { r: 0, g: 43, b: 54 }), "\x1b[?997;1n");
        assert_eq!(
            report(Rgb {
                r: 253,
                g: 246,
                b: 227
            }),
            "\x1b[?997;2n"
        );
    }
}
