//! App-level callback hooks registered once at startup by `anterm-app`.
//!
//! Keeps lower-level crates (`anterm-ui-kit::keymap`, `anterm-settings-ui`)
//! completely decoupled from `anterm-app`'s `WindowRegistry` and `AntermApp`.

use gpui::{AnyWindowHandle, App, Global};

#[derive(Default)]
struct AppHooks {
    open_new_window: Option<fn(&mut App)>,
    init_palette: Option<fn(&mut App)>,
    refresh_locale: Option<fn(&mut App, Option<AnyWindowHandle>)>,
    refresh_shells: Option<fn(&mut App)>,
}

impl Global for AppHooks {}

/// Register application shell callbacks during `anterm-app` startup.
pub fn register(
    cx: &mut App,
    open_new_window: fn(&mut App),
    init_palette: fn(&mut App),
    refresh_locale: fn(&mut App, Option<AnyWindowHandle>),
    refresh_shells: fn(&mut App),
) {
    cx.set_global(AppHooks {
        open_new_window: Some(open_new_window),
        init_palette: Some(init_palette),
        refresh_locale: Some(refresh_locale),
        refresh_shells: Some(refresh_shells),
    });
}

pub fn open_new_window(cx: &mut App) {
    if let Some(f) = cx.try_global::<AppHooks>().and_then(|h| h.open_new_window) {
        f(cx);
    }
}

pub fn init_palette(cx: &mut App) {
    if let Some(f) = cx.try_global::<AppHooks>().and_then(|h| h.init_palette) {
        f(cx);
    }
}

pub fn refresh_locale(cx: &mut App, skip: Option<AnyWindowHandle>) {
    if let Some(f) = cx.try_global::<AppHooks>().and_then(|h| h.refresh_locale) {
        f(cx, skip);
    }
}

pub fn refresh_shells(cx: &mut App) {
    if let Some(f) = cx.try_global::<AppHooks>().and_then(|h| h.refresh_shells) {
        f(cx);
    }
}
