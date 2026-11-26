extern crate gpui_kit as gpui;

mod core;
mod daemon;
pub(crate) use anterm_ui_kit::git;
pub(crate) use anterm_ui_kit::platform;
mod startup;
pub(crate) use anterm_terminal as terminal;
mod ui;

use crate::core::config::Config;
use crate::core::config_store::ConfigStore;
#[cfg(target_os = "macos")]
use crate::platform::bootstrap::{apply_font_thicken, set_dock_icon_for_bare_binary};
#[cfg(unix)]
use crate::platform::bootstrap::enrich_path_from_login_shell;
#[cfg(windows)]
use crate::startup::cli::update_install_dir_from;
use crate::startup::cli::{
    apply_config_dir_arg, explorer_menu_action_from, forward_open_path, handle_external_opens,
    open_path_from,
};
use crate::startup::config_watch::{notify_config_load_failed, spawn_config_watcher};
use crate::ui::chrome::assets::Assets;
use crate::ui::chrome::keymap;
use gpui::*;

/// Register `anterm-app`'s application-level hooks into `anterm-ui-kit` so
/// lower-level crates (`keymap`, `anterm-settings-ui`) can open windows,
/// install palette interceptors, and broadcast locale/shell refreshes without
/// depending on `anterm-app`.
pub(crate) fn register_app_hooks(cx: &mut App) {
    anterm_ui_kit::app_hooks::register(
        cx,
        crate::ui::workspace::windows::open,
        crate::ui::chrome::palette::init,
        crate::ui::workspace::windows::WindowRegistry::refresh_locale,
        crate::ui::workspace::windows::WindowRegistry::refresh_shells,
    );
}

fn register_bundled_fonts(cx: &mut App) {
    use std::borrow::Cow;
    let fonts = vec![
        Cow::Borrowed(include_bytes!("../../../assets/fonts/hack/Hack-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/hack/Hack-Bold.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/hack/Hack-Italic.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../../../assets/fonts/hack/Hack-BoldItalic.ttf").as_slice()),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        log::warn!("failed to register bundled Hack fonts: {e}");
    }
}

/// Entry point for the desktop workbench binary (`anterm-app`).
pub fn run() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    {
        if args.first().map(std::ffi::OsString::as_os_str)
            == Some(std::ffi::OsStr::new("agent-hook"))
        {
            if let (Some(agent), Some(event)) = (
                args.get(1).and_then(|arg| arg.to_str()),
                args.get(2).and_then(|arg| arg.to_str()),
            ) {
                crate::core::agent_hooks::run_agent_hook(agent, event);
            }
            return;
        }
    }

    apply_config_dir_arg(&args);

    let daemon = args
        .iter()
        .any(|arg| arg == std::ffi::OsStr::new("--daemon"));
    let role = if daemon { "daemon" } else { "gui" };
    crate::core::crash::install(role);
    crate::core::logfile::install(role);

    // The Windows installer owns the Explorer context menu: a task checkbox
    // runs these, and the uninstaller always runs the unregister half. Keeping
    // the registry shape in `explorer_context_menu` rather than in the .iss
    // means the installer and the running app can never disagree about it.
    // Handled after the log file is open, because a GUI-subsystem process has
    // no console to report a failure on and Inno does not surface exit codes:
    // the log is the only place the reason can survive.
    if let Some(register) = explorer_menu_action_from(&args) {
        let result = if register {
            // The verb labels are localized, and this process stops at the
            // `return` below — it never reaches the `set_locale` on the GUI
            // path. Without this read every install would write English
            // entries, whatever language the user runs anterm in.
            anterm_i18n::set_locale(&Config::load().gui_language);
            crate::core::explorer_context_menu::register()
        } else {
            crate::core::explorer_context_menu::unregister()
        };
        if let Err(error) = result {
            log::error!("the Explorer context-menu update failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    if daemon {
        if crate::daemon::server::run_daemon().is_err() {
            std::process::exit(1);
        }
        return;
    }

    if args
        .iter()
        .any(|arg| arg == std::ffi::OsStr::new("--stop-daemon"))
    {
        // An installer about to replace `dir` says so, and gets more than a
        // stop: orphaned ConPTY hosts and anything else still running from
        // that directory are terminated, and the call does not return until
        // the images there are actually replaceable (or says why they are
        // not). Invoked by the Inno PrepareToInstall step and the updater.
        #[cfg(windows)]
        if let Some(dir) = update_install_dir_from(args.iter().cloned()) {
            // Held in the *parent's* name: this helper returns in seconds,
            // but the Setup (or uninstaller) that invoked it keeps replacing
            // files in `dir` until it exits — and a daemon spawned in that
            // window would relock them. The guard needs no clearing; it goes
            // stale the moment that parent is gone.
            anterm_core::daemon::update_guard::hold_for_parent();
            if let Err(error) = crate::daemon::spawn::stop_for_update(&dir) {
                log::error!(
                    "preparing {} for replacement failed: {error}",
                    dir.display()
                );
                std::process::exit(1);
            }
            return;
        }
        crate::daemon::spawn::stop();
        return;
    }

    let open_path = open_path_from(args.into_iter());
    if forward_open_path(open_path.as_deref()) {
        return;
    }

    // After the forward: a launch that only hands a path to the running
    // window has no use for the PATH, and running the user's rc costs time.
    #[cfg(unix)]
    enrich_path_from_login_shell();

    // A package the user asked to have applied at the next launch. Deliberately
    // here: after the forward above, so a second launch that is really a
    // request to an existing window never replaces the bundle out from under
    // it, and before everything below, so there is no daemon to strand and no
    // window to flash. On success the updater takes over and this process is
    // done; on failure it has already discarded the plan and we start normally.
    if crate::core::update::apply_pending_at_launch() {
        return;
    }
    // If this launch *is* the relaunch an updater just performed, its outcome
    // file is waiting; fold it into the update state before the first window
    // reads it. Also covers the recovery relaunch after a failed install.
    crate::core::update::absorb_update_outcome_at_launch();

    let (config, config_outcome) = crate::core::config::Config::load_with_outcome();
    let gui_language = config.gui_language.clone();
    #[cfg(target_os = "macos")]
    apply_font_thicken(config.font_thicken);

    // Give desktop toasts the anterm icon and name instead of notify-rust's
    // PowerShell fallback. Best-effort; no-op off Windows.
    #[cfg(target_os = "windows")]
    crate::core::aumid::init();

    let restore_session = config.restore_session;
    let daemon_result = if restore_session {
        crate::daemon::spawn::ensure_running()
    } else {
        crate::daemon::spawn::restart()
    };
    // A pathless launch that found a GUI already registered hands the request
    // to it — the GUI may be sitting in the tray with no window — and exits.
    // The forward above cannot do this: it runs before the daemon exists, and
    // routing through a daemon that is not up yet would fail on every cold
    // start. Here the daemon is known to be running, so the probe is cheap and
    // a `Bool(true)` means a GUI actually received the request.
    if open_path.is_none() && daemon_result.is_ok() && forward_open_path(None) {
        return;
    }
    match &daemon_result {
        Ok(()) => {
            if let Err(e) = crate::daemon::spawn::adopt_this_build() {
                log::warn!("the daemon stays on its old build: {e}");
            }
        }
        Err(e) => log::error!("failed to ensure daemon is running: {e}"),
    }

    // `Application`, not the in-loop `App`, owns the native delegate. Register
    // before `run` so macOS can deliver Finder and URL events from launch.
    let (external_open_tx, external_open_rx) = smol::channel::unbounded();
    let application = gpui_kit::application()
        .with_assets(Assets)
        // The window-close path decides whether this process survives: with
        // the tray icon on, closing the last window retires to the tray, and
        // without it the close handler quits explicitly. The platform default
        // would quit on the last window unconditionally, which is exactly the
        // orphaned-daemon trap the tray is meant to prevent.
        .with_quit_mode(QuitMode::Explicit);
    application.on_open_urls(move |urls| {
        let _ = external_open_tx.try_send(urls);
    });
    // macOS relaunching an app that is already running — the Dock icon, a
    // double-click on the bundle, `open -a anterm` — only reaches this process
    // as `applicationShouldHandleReopen:`, and gpui's delegate does nothing
    // with it unless a handler is registered. That is the one entrance a
    // tray-resident anterm has: with `show_tray_icon` on, closing the last
    // window keeps the process and its Dock icon alive with nothing on
    // screen, and without this the icon's click was a no-op (the only ways
    // back in were ⌘N, the tray's "Show anterm", or quitting and relaunching).
    // Like `on_open_urls`, the hook lives on `Application`, so it cannot go
    // beside the other app-level handlers in `keymap::init`; and like that
    // path it defers to the loop rather than opening windows on AppKit's
    // delegate stack, which is also how Zed's own reopen handler runs.
    application.on_reopen(|cx| {
        cx.spawn(async move |cx| {
            let _ = cx.update(crate::ui::workspace::windows::reopen);
        })
        .detach();
    });
    application.run(move |cx| {
        // gpui invokes this callback without an `App` context. Bridge it
        // back onto the application loop instead of touching UI state on
        // AppKit's delegate call stack.
        cx.spawn(async move |cx| {
            while let Ok(urls) = external_open_rx.recv().await {
                let _ = cx.update(|cx| handle_external_opens(urls, cx));
            }
        })
        .detach();
        gpui_kit::init(cx);
        register_bundled_fonts(cx);
        crate::ui::chrome::prompt::install(cx);
        cx.activate(true);
        #[cfg(target_os = "macos")]
        set_dock_icon_for_bare_binary();
        anterm_i18n::set_locale(&gui_language);
        // The load above is reused rather than re-read: reading the same
        // file twice at launch would report the same failure twice.
        ConfigStore::init(config, cx);
        crate::ui::chrome::theme::refresh_system_appearance(cx);
        crate::ui::workspace::windows::WindowRegistry::init(cx);
        crate::platform::notify::register_activation_handler(|activation| {
            let action = match activation {
                crate::platform::notify::NotificationActivation::RevealPane { leaf_id } => {
                    crate::ui::chrome::tray::TrayAction::RevealPane { leaf_id }
                }
                crate::platform::notify::NotificationActivation::ShowWindow => {
                    crate::ui::chrome::tray::TrayAction::ShowWindow
                }
            };
            crate::ui::chrome::tray::send_or_queue(action);
        });
        crate::ui::chrome::presets::load_registry(cx);
        crate::ui::chrome::theme::apply_cursor_hide_mode(cx);
        spawn_config_watcher(cx);
        crate::core::update::spawn_check(cx);
        cx.background_executor()
            .spawn(async {
                crate::core::agent_hooks::refresh_hooks_at_launch();
            })
            .detach();
        register_app_hooks(cx);
        keymap::init(cx);
        crate::ui::remote::local_link::LocalLink::install(cx);
        anterm_gateway::GatewayRuntime::global().sync_default_background();

        crate::ui::workspace::windows::open_at(cx, open_path);
        if config_outcome.failed() {
            notify_config_load_failed(cx, config_outcome, true);
        }
    });
}
