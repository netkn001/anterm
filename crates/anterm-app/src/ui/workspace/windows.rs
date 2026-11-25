use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Styled as _,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowDecorations, WindowOptions, point, px,
    size,
};
use gpui_kit::component::{Root, TitleBar};

use crate::core::config::{Config, StartupMode};
use crate::core::window_state::{WindowGeometry as _, WindowState};
use crate::ui::workspace::app::AntermApp;

const CASCADE_STEP: f32 = 28.0;

const DEFAULT_SIZE: (f32, f32) = (1440.0, 900.0);

/// The smallest window anterm still holds its shape in.
///
/// Below this the chrome stops being chrome: the sidebar is at its 180px floor
/// with barely forty columns beside it, and the overlays the app drops on top —
/// the palette at 560 wide — have nowhere to land.
const MIN_SIZE: (f32, f32) = (720.0, 520.0);

struct WindowEntry {
    handle: AnyWindowHandle,
    app: WeakEntity<AntermApp>,
}

#[derive(Default)]
pub struct WindowRegistry {
    windows: Vec<WindowEntry>,
}

impl Global for WindowRegistry {}

impl WindowRegistry {
    pub fn init(cx: &mut App) {
        cx.set_global(Self::default());
    }

    pub fn count(cx: &mut App) -> usize {
        Self::sweep(cx);
        cx.global::<Self>().windows.len()
    }

    pub fn open_windows(cx: &mut App) -> Vec<(AnyWindowHandle, WeakEntity<AntermApp>)> {
        Self::sweep(cx);
        cx.global::<Self>()
            .windows
            .iter()
            .map(|w| (w.handle, w.app.clone()))
            .collect()
    }

    pub fn primary(cx: &mut App) -> Option<(AnyWindowHandle, WeakEntity<AntermApp>)> {
        Self::sweep(cx);
        cx.global::<Self>()
            .windows
            .first()
            .map(|w| (w.handle, w.app.clone()))
    }

    pub fn note_focused(cx: &mut App, handle: AnyWindowHandle) {
        let windows = &mut cx.global_mut::<Self>().windows;
        if let Some(pos) = windows.iter().position(|w| w.handle == handle)
            && pos > 0
        {
            let entry = windows.remove(pos);
            windows.insert(0, entry);
        }
    }

    pub fn app_in(cx: &mut App, window: &Window) -> Option<gpui::Entity<AntermApp>> {
        Self::sweep(cx);
        let handle = window.window_handle();
        cx.global::<Self>()
            .windows
            .iter()
            .find(|w| w.handle == handle)
            .and_then(|w| w.app.upgrade())
    }

    pub fn refresh_locale(cx: &mut App, except: Option<AnyWindowHandle>) {
        Self::sweep(cx);
        let windows: Vec<_> = cx
            .global::<Self>()
            .windows
            .iter()
            .filter(|entry| Some(entry.handle) != except)
            .map(|entry| (entry.handle, entry.app.clone()))
            .collect();
        for (handle, app) in windows {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = app.update(cx, |app, cx| app.refresh_locale_state(window, cx));
                window.refresh();
            });
        }
    }

    /// Rebuilds every window's shell inventory from disk.
    pub fn refresh_shells(cx: &mut App) {
        Self::sweep(cx);
        let apps: Vec<_> = cx
            .global::<Self>()
            .windows
            .iter()
            .map(|entry| entry.app.clone())
            .collect();
        for app in apps {
            let _ = app.update(cx, |app, cx| app.refresh_shells(cx));
        }
    }

    pub(crate) fn register(cx: &mut App, handle: AnyWindowHandle, app: WeakEntity<AntermApp>) {
        cx.global_mut::<Self>()
            .windows
            .insert(0, WindowEntry { handle, app });
    }

    pub fn unregister(cx: &mut App, handle: AnyWindowHandle) {
        cx.global_mut::<Self>()
            .windows
            .retain(|w| w.handle != handle);
    }

    fn sweep(cx: &mut App) {
        let has_dead = cx
            .global::<Self>()
            .windows
            .iter()
            .any(|w| w.app.upgrade().is_none());
        if !has_dead {
            return;
        }
        cx.global_mut::<Self>()
            .windows
            .retain(|w| w.app.upgrade().is_some());
    }
}

pub fn open(cx: &mut App) {
    open_at(cx, None);
}

/// Reveals the primary window (opening one if none is up) and activates `tab`.
#[allow(dead_code)]
pub fn open_at_tab(cx: &mut App, tab: anterm_core::machine::TabId) {
    let target = WindowRegistry::primary(cx).or_else(|| {
        open_at(cx, None);
        WindowRegistry::primary(cx)
    });
    let Some((handle, app)) = target else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.activate_window();
        let _ = app.update(cx, |this, cx| {
            this.activate_tree_tab(tab, window, cx);
        });
    });
}

pub fn open_at(cx: &mut App, initial_cwd: Option<std::path::PathBuf>) {
    let options = window_options(cx);
    let mut created: Option<gpui::Entity<AntermApp>> = None;
    let opened = cx.open_window(options, |window, cx| {
        let app = cx.new(|cx| match initial_cwd.clone() {
            Some(cwd) => AntermApp::new_at(Some(cwd), window, cx),
            None => AntermApp::new(window, cx),
        });
        created = Some(app.clone());
        cx.new(|cx| Root::new(app, window, cx).bg(gpui::transparent_black()))
    });

    let handle = match opened {
        Ok(handle) => handle,
        Err(e) => {
            log::error!("failed to open window: {e}");
            return;
        }
    };
    let Some(app) = created else {
        log::error!("opened a window but its AntermApp was never built; not registering");
        return;
    };

    WindowRegistry::register(cx, handle.into(), app.downgrade());
    refresh_menu(cx);
}

pub fn open_from_cli(cx: &mut App, path: Option<std::path::PathBuf>) {
    let Some((handle, app)) = WindowRegistry::primary(cx) else {
        cx.activate(true);
        open_at(cx, path);
        return;
    };
    let Some(app) = app.upgrade() else {
        return;
    };

    cx.activate(true);
    let _ = handle.update(cx, move |_, window, cx| {
        if let Some(path) = path {
            app.update(cx, |app, cx| app.new_tab_at(path, window, cx));
        }
        window.activate_window();
    });
}

/// Runs a local command in a new tab of the primary window, opening one if
/// none is up.
pub fn run_local_command(cx: &mut App, cwd: std::path::PathBuf, command: String) {
    let Some((handle, app)) = WindowRegistry::primary(cx) else {
        open_at(cx, Some(cwd.clone()));
        let Some((handle, app)) = WindowRegistry::primary(cx) else {
            return;
        };
        if crate::ui::remote::tree_sync::park_command_while_pulling(cx, &cwd, &command) {
            activate_handle(cx, handle);
        } else {
            run_command_in_active_terminal(cx, handle, app, command);
        }
        return;
    };
    if crate::ui::remote::tree_sync::park_command_while_pulling(cx, &cwd, &command) {
        activate_handle(cx, handle);
        return;
    }
    run_local_command_in(cx, handle, app, cwd, command);
}

fn activate_handle(cx: &mut App, handle: AnyWindowHandle) {
    cx.activate(true);
    let _ = handle.update(cx, |_, window, _| window.activate_window());
}

fn run_command_in_active_terminal(
    cx: &mut App,
    handle: AnyWindowHandle,
    app: WeakEntity<AntermApp>,
    command: String,
) {
    let Some(app) = app.upgrade() else {
        return;
    };
    cx.activate(true);
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| {
            app.run_in_active_terminal(&command, window, cx)
        });
        window.activate_window();
    });
}

fn run_local_command_in(
    cx: &mut App,
    handle: AnyWindowHandle,
    app: WeakEntity<AntermApp>,
    cwd: std::path::PathBuf,
    command: String,
) {
    let Some(app) = app.upgrade() else {
        return;
    };
    cx.activate(true);
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| app.new_tab_running(cwd, command, window, cx));
        window.activate_window();
    });
}

/// Opens an `ssh://` link in the primary window, opening one if none is up.
pub fn quick_connect_from_url(cx: &mut App, ssh: anterm_core::ssh_profile::QuickConnect) {
    let target = WindowRegistry::primary(cx).or_else(|| {
        open_at(cx, None);
        WindowRegistry::primary(cx)
    });
    let Some((handle, app)) = target else {
        return;
    };
    if crate::ui::remote::tree_sync::park_ssh_while_pulling(cx, &ssh) {
        activate_handle(cx, handle);
        return;
    }
    let Some(app) = app.upgrade() else {
        return;
    };
    cx.activate(true);
    let _ = handle.update(cx, move |_, window, cx| {
        app.update(cx, |app, cx| app.quick_connect(ssh, window, cx));
        window.activate_window();
    });
}

/// Brings the UI back when macOS relaunches an anterm that is already running.
pub fn reopen(cx: &mut App) {
    reopen_with(cx, open_at);
}

fn reopen_with(cx: &mut App, open: impl FnOnce(&mut App, Option<std::path::PathBuf>)) {
    match WindowRegistry::primary(cx) {
        Some((handle, _)) => activate_handle(cx, handle),
        None => open(cx, None),
    }
}

pub fn refresh_menu(cx: &mut App) {
    crate::ui::chrome::theme::set_menus(cx);
}

fn window_options(cx: &mut App) -> WindowOptions {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    static APP_ICON: std::sync::LazyLock<Option<std::sync::Arc<image::RgbaImage>>> =
        std::sync::LazyLock::new(|| {
            image::load_from_memory(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/app-icon.png"
            )))
            .ok()
            .map(|image| std::sync::Arc::new(image.thumbnail(256, 256).into_rgba8()))
        });

    let remember = cx.global::<Config>().remember_window_size;
    let remembered = remember.then(WindowState::load).flatten();

    let existing = WindowRegistry::count(cx);
    let bounds = match remembered {
        Some(state) => {
            let bounds = at_least_min_size(state.bounds());
            if cx.displays().iter().any(|d| d.bounds().intersects(&bounds)) {
                bounds
            } else {
                Bounds::centered(None, bounds.size, cx)
            }
        }
        None => Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx),
    };
    let bounds = cascade(bounds, existing);

    let window_bounds = match cx.global::<Config>().startup_mode {
        _ if existing > 0 => WindowBounds::Windowed(bounds),
        StartupMode::Normal => WindowBounds::Windowed(bounds),
        StartupMode::Maximized => WindowBounds::Maximized(bounds),
        StartupMode::Fullscreen => WindowBounds::Fullscreen(bounds),
    };

    WindowOptions {
        window_bounds: Some(window_bounds),
        app_id: Some("anterm".to_owned()),
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        icon: APP_ICON.as_ref().cloned(),
        titlebar: Some(TitlebarOptions {
            traffic_light_position: Some(crate::ui::chrome::theme::traffic_light_position()),
            ..TitleBar::title_bar_options()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_background: crate::ui::chrome::theme::background_appearance(cx),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        ..Default::default()
    }
}

/// Grow a remembered bound back up to `MIN_SIZE`.
fn at_least_min_size(bounds: Bounds<gpui::Pixels>) -> Bounds<gpui::Pixels> {
    Bounds {
        origin: bounds.origin,
        size: size(
            bounds.size.width.max(px(MIN_SIZE.0)),
            bounds.size.height.max(px(MIN_SIZE.1)),
        ),
    }
}

fn cascade(bounds: Bounds<gpui::Pixels>, existing: usize) -> Bounds<gpui::Pixels> {
    if existing == 0 {
        return bounds;
    }
    let step = (existing % 5) as f32 * CASCADE_STEP;
    Bounds {
        origin: bounds.origin + point(px(step), px(step)),
        size: bounds.size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds_at(x: f32, y: f32) -> Bounds<gpui::Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(800.), px(600.)),
        }
    }

    #[test]
    fn the_first_window_is_not_cascaded() {
        let b = bounds_at(100., 100.);
        assert_eq!(cascade(b, 0).origin, b.origin);
    }

    #[test]
    fn each_extra_window_steps_down_and_right() {
        let b = bounds_at(100., 100.);
        assert_eq!(
            cascade(b, 1).origin,
            point(px(100. + CASCADE_STEP), px(100. + CASCADE_STEP))
        );
        assert_eq!(
            cascade(b, 2).origin,
            point(px(100. + 2. * CASCADE_STEP), px(100. + 2. * CASCADE_STEP))
        );
        assert_eq!(cascade(b, 3).size, b.size);
    }

    #[test]
    fn a_remembered_bound_under_the_minimum_is_grown_back_to_it() {
        let undersized = Bounds {
            origin: point(px(700.), px(60.)),
            size: size(px(641.), px(830.)),
        };
        let grown = at_least_min_size(undersized);
        assert_eq!(grown.size.width, px(MIN_SIZE.0));
        assert_eq!(grown.size.height, px(830.), "a tall enough height is kept");
        assert_eq!(grown.origin, undersized.origin, "the corner does not move");

        let short = at_least_min_size(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1200.), px(300.)),
        });
        assert_eq!(short.size, size(px(1200.), px(MIN_SIZE.1)));
    }

    #[test]
    fn a_remembered_bound_at_or_over_the_minimum_is_left_alone() {
        let b = bounds_at(100., 100.);
        assert_eq!(at_least_min_size(b).size, b.size);
        let exact = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(MIN_SIZE.0), px(MIN_SIZE.1)),
        };
        assert_eq!(at_least_min_size(exact).size, exact.size);
    }

    #[test]
    fn cascade_wraps_so_windows_never_march_off_screen() {
        let b = bounds_at(100., 100.);
        assert_eq!(cascade(b, 5).origin, b.origin);
        assert_eq!(cascade(b, 6).origin, cascade(b, 1).origin);
    }

    #[gpui::test]
    fn a_window_asks_the_compositor_for_client_side_decorations(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            WindowRegistry::init(cx);
            let mut config = Config::default();
            config.remember_window_size = false;
            cx.set_global(config);

            let options = window_options(cx);
            assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
            assert!(
                options.titlebar.is_some_and(|t| t.appears_transparent),
                "the in-app title bar is what the client-side request stands in for"
            );
        });
    }

    #[gpui::test]
    fn a_reopen_with_no_window_up_opens_one(cx: &mut gpui::TestAppContext) {
        let mut opened = false;

        cx.update(|cx| {
            WindowRegistry::init(cx);
            reopen_with(cx, |_, path| {
                assert!(path.is_none());
                opened = true;
            });
        });

        assert!(opened);
    }

    #[gpui::test]
    fn a_reopen_with_a_window_up_activates_it_instead_of_opening_another(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::VisualContext as _;

        let (app, mut vcx) = crate::ui::workspace::app::test_window::harness(cx);
        let handle = vcx.window_handle();
        app.update_in(&mut vcx, |_, _, cx| {
            WindowRegistry::init(cx);
            WindowRegistry::register(cx, handle, app.downgrade());

            reopen_with(cx, |_, _| {
                panic!("a reopen with a window up opened another window");
            });
            assert_eq!(WindowRegistry::count(cx), 1);
        });
    }
}
