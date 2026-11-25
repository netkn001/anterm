//! Live config & theme file watcher and hot-reload application.

use gpui::*;

use crate::core::config::Config;

/// Keeps the OS filesystem watcher alive for the lifetime of the GPUI `App`
/// without leaking memory via `Box::leak`.
struct ConfigWatcherGuard {
    _watcher: notify::RecommendedWatcher,
}
impl Global for ConfigWatcherGuard {}

pub(crate) fn spawn_config_watcher(cx: &mut App) {
    use notify::{RecursiveMode, Watcher};

    let Some(config_file) = crate::core::config::config_path("config.json") else {
        return;
    };
    let Some(dir) = crate::core::config::config_dir_path() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    // FSEvents reports the resolved path, and a watch registered through a
    // symlink (`/tmp` is `/private/tmp` on macOS) never matches it: every
    // edit to config.json went unseen.
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);

    const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

    let (tx, rx) = smol::channel::unbounded::<()>();
    let watched_file = config_file.clone();
    let handler = move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        // A reload re-reads the file, and on Linux reading it is itself an
        // event — see `anterm_core::host::is_content_change`.
        if !anterm_core::host::is_content_change(&event.kind) {
            return;
        }
        // The saved SSH hosts live beside config.json in a file of their own
        // (#911), and a hand edit there has to land the same way.
        let hit = event.paths.iter().any(|p| {
            p.file_name() == watched_file.file_name()
                || p.file_name() == Some(std::ffi::OsStr::new(crate::core::config::SERVERS_FILE))
                || is_theme_file(p)
        });
        if hit {
            let _ = tx.try_send(());
        }
    };

    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("config hot-reload disabled: failed to create watcher: {e}");
            return;
        }
    };
    if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
        log::warn!(
            "config hot-reload disabled: failed to watch {}: {e}",
            dir.display()
        );
        return;
    }
    cx.set_global(ConfigWatcherGuard { _watcher: watcher });

    cx.spawn(async move |cx| {
        // One toast per breakage rather than one per write: this watcher also
        // fires for every theme file, so a config.json left broken would
        // otherwise re-announce itself on each of them. Cleared by the load
        // that parses, so a second breakage speaks up again.
        let mut announced = false;
        while rx.recv().await.is_ok() {
            cx.background_executor().timer(DEBOUNCE).await;
            while rx.try_recv().is_ok() {}

            cx.update(|cx| {
                apply_reloaded_config(cx, Config::load_with_outcome(), &mut announced);
            });
        }
    })
    .detach();
}

/// One watcher tick, once the debounce is out: what a reload does to the
/// running app.
///
/// `announced` is the one-toast-per-breakage latch, which lives across ticks
/// in the watcher. Returns whether the keymap was rebuilt — the watcher has no
/// use for that; it is how a test asks which branch ran.
fn apply_reloaded_config(
    cx: &mut App,
    load: (Config, crate::core::config::LoadOutcome),
    announced: &mut bool,
) -> bool {
    let (config, outcome) = load;
    if outcome.failed() {
        // Keep the settings the app is running on: swapping the stand-in
        // defaults in would flash the whole UI onto defaults, and the load
        // already parked the broken file beside the original. It reloads
        // itself the moment the file parses again.
        if !*announced {
            *announced = true;
            notify_config_load_failed(cx, outcome, false);
        }
        // The theme files this same watcher covers must keep hot-reloading: a
        // typo in config.json is no reason for theme editing to go dead until
        // the app restarts. They read the global config, which is deliberately
        // still the one the app is running on.
        reload_themes(cx);
        cx.refresh_windows();
        return false;
    }
    *announced = false;
    // The keymap is rebuilt only when a binding actually moved. This watcher
    // fires for every write under the config dir — including the app's own
    // `save()`, which a sidebar drag or a palette open triggers — so reloading
    // bindings on each tick would rebuild the whole keymap for nothing, and
    // (before `rebind` cleared first) leak a full table per tick (#548).
    // Read past the early return above: a load that failed leaves the global
    // alone, so there is nothing to compare and the user's keys stay in the
    // keymap the app is dispatching on.
    let keymap_before = crate::ui::chrome::keymap::keybinding_config(cx);
    // Explorer's verbs live in the registry rather than in this process, so a
    // hand-edited `gui_language` leaves them behind unless something restates
    // them. Gated on the language actually moving, for the same reason the
    // keymap below is: this watcher fires on every config write, and a sidebar
    // drag has no business touching the registry.
    let language_changed = cx.global::<Config>().gui_language != config.gui_language;
    anterm_i18n::set_locale(&config.gui_language);
    crate::core::config_store::ConfigStore::replace_from_disk(config, cx);
    reload_themes(cx);
    crate::ui::chrome::theme::apply_cursor_hide_mode(cx);
    // The menu bar is built once from the current locale, so editing
    // gui_language by hand has to rebuild it the same way the in-app language
    // picker does.
    crate::ui::chrome::theme::set_menus(cx);
    if language_changed {
        crate::core::explorer_context_menu::refresh_labels();
    }
    crate::ui::workspace::windows::WindowRegistry::refresh_locale(cx, None);
    // `custom_shells` is only ever hand-edited, so this file is the one place
    // it can change from — and the inventory that carries it to the new-tab
    // menu is cached per window.
    crate::ui::workspace::windows::WindowRegistry::refresh_shells(cx);
    // A hand-edited keybinding shows up in the settings list off the live
    // global immediately; without this it never reaches the keymap gpui
    // actually dispatches against, so the key looks bound and does nothing
    // until restart. Gated on the triple so an unrelated save does not churn
    // it.
    let rebound = crate::ui::chrome::keymap::keybinding_config(cx) != keymap_before;
    if rebound {
        crate::ui::chrome::keymap::rebind(cx);
    }
    cx.refresh_windows();
    rebound
}

fn is_theme_file(p: &std::path::Path) -> bool {
    p.parent().and_then(|d| d.file_name()) == Some(std::ffi::OsStr::new("themes"))
        && p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            e.eq_ignore_ascii_case("yaml")
                || e.eq_ignore_ascii_case("yml")
                || e.eq_ignore_ascii_case("itermcolors")
        })
}

/// Re-reads what the theme files on disk say. The config watcher covers the
/// themes directory too, so this has to run even when config.json itself did
/// not load — editing a theme cannot go dead because of a typo elsewhere.
fn reload_themes(cx: &mut App) {
    crate::ui::chrome::presets::load_registry(cx);
    crate::ui::chrome::theme::apply_theme(None, cx);
}

/// Says out loud that config.json did not load and, when there is one, where
/// its contents were parked. Without this the symptom is "my settings are
/// gone" (startup) or "my edit did nothing" (reload) — both read as data loss,
/// and neither points at the file that needs fixing.
pub(crate) fn notify_config_load_failed(
    cx: &mut App,
    outcome: crate::core::config::LoadOutcome,
    startup: bool,
) {
    use crate::core::config::LoadOutcome;
    use anterm_i18n::L10nKey;
    use gpui_kit::component::WindowExt as _;

    // Only an unparseable file leaves a copy behind; an unreadable one had
    // nothing to copy, so it must not send the user after a `.corrupt` file
    // that was never written.
    let key = match (outcome, startup) {
        (LoadOutcome::Unreadable, true) => L10nKey::ConfigUnreadableStartup,
        (LoadOutcome::Unreadable, false) => L10nKey::ConfigUnreadableReload,
        (_, true) => L10nKey::ConfigQuarantinedStartup,
        (_, false) => L10nKey::ConfigQuarantinedReload,
    };
    let Some((handle, _)) = crate::ui::workspace::windows::WindowRegistry::primary(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.push_notification(anterm_i18n::t(key), cx);
    });
}


#[cfg(test)]
mod config_reload_tests {
    use super::{apply_reloaded_config, is_theme_file};
    use crate::core::actions::SplitRight;
    use crate::core::config::{Config, LoadOutcome};
    use gpui::{Action as _, App, KeyContext, Keystroke, TestAppContext};

    /// What the live keymap — the one gpui dispatches against — does with
    /// `keys` typed in a terminal.
    fn dispatched(cx: &App, keys: &str) -> Vec<&'static str> {
        let typed = [Keystroke::parse_with_os(keys, crate::platform::current_os()).expect("the typed keystroke parses")];
        let context = [KeyContext::parse("Terminal").expect("the context parses")];
        cx.key_bindings()
            .borrow()
            .bindings_for_input(&typed, &context)
            .0
            .iter()
            .map(|b| b.action().name())
            .collect()
    }

    /// An app running on `config`, as far as the config watcher can tell:
    /// the globals its tick reads, and a keymap built from that config.
    fn running_on(cx: &mut App, config: Config) {
        let dir = std::env::temp_dir().join(format!("anterm-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        crate::core::config::set_config_dir(dir);

        gpui_kit::init(cx);
        cx.set_global(config);
        crate::ui::workspace::windows::WindowRegistry::init(cx);
        crate::ui::chrome::presets::load_registry(cx);
        crate::ui::chrome::keymap::init(cx);
    }

    fn bound_to_split_right(config: &mut Config, key: &str) {
        config.keybindings.insert(
            "SplitRight".to_string(),
            crate::core::config::KeybindingOverride::Add(key.to_string()),
        );
    }

    #[gpui::test]
    fn a_hand_edited_binding_fires_without_a_restart(cx: &mut TestAppContext) {
        cx.update(|cx| {
            running_on(cx, Config::default());
            assert!(
                dispatched(cx, "ctrl-alt-9").is_empty(),
                "nothing is on the chord before the edit"
            );

            let mut edited = Config::default();
            bound_to_split_right(&mut edited, "ctrl-alt-9");
            let mut announced = false;
            assert!(
                apply_reloaded_config(cx, (edited, LoadOutcome::Parsed), &mut announced),
                "a moved binding rebuilds the keymap"
            );

            assert_eq!(
                dispatched(cx, "ctrl-alt-9"),
                vec![SplitRight::name_for_type()],
                "the hand-edited chord reaches the keymap gpui dispatches on"
            );
        });
    }

    #[gpui::test]
    fn a_save_that_moves_no_binding_leaves_the_keymap_alone(cx: &mut TestAppContext) {
        cx.update(|cx| {
            running_on(cx, Config::default());

            // The app's own `save()` fires this watcher on every sidebar drag.
            let mut unrelated = Config::default();
            unrelated.dim_inactive_panes = !unrelated.dim_inactive_panes;
            let mut announced = false;
            assert!(
                !apply_reloaded_config(
                    cx,
                    (unrelated.clone(), LoadOutcome::Parsed),
                    &mut announced
                ),
                "an unrelated save must not rebuild the keymap"
            );
            assert_eq!(
                cx.global::<Config>().dim_inactive_panes,
                unrelated.dim_inactive_panes,
                "the reload still took effect; only the rebind was skipped"
            );
        });
    }

    #[gpui::test]
    fn a_broken_config_does_not_take_the_users_keys_away(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut user = Config::default();
            bound_to_split_right(&mut user, "ctrl-alt-9");
            running_on(cx, user);
            assert_eq!(
                dispatched(cx, "ctrl-alt-9"),
                vec![SplitRight::name_for_type()]
            );

            // A quarantined load hands back stand-in defaults, and the global
            // is deliberately left alone — so nothing may rebind off them.
            let mut announced = false;
            assert!(
                !apply_reloaded_config(
                    cx,
                    (Config::default(), LoadOutcome::Quarantined),
                    &mut announced
                ),
                "a load that failed has nothing to compare and must not rebind"
            );
            assert!(announced, "the breakage is announced");
            assert_eq!(
                cx.global::<Config>().keybindings.get("SplitRight"),
                Some(&crate::core::config::KeybindingOverride::Add(
                    "ctrl-alt-9".to_string()
                )),
                "the running config survives the broken file"
            );
            assert_eq!(
                dispatched(cx, "ctrl-alt-9"),
                vec![SplitRight::name_for_type()],
                "and so do the keys the app is dispatching on"
            );

            // The file parses again: the latch clears, so a second breakage
            // can speak up.
            let mut fixed = Config::default();
            bound_to_split_right(&mut fixed, "ctrl-alt-8");
            assert!(apply_reloaded_config(
                cx,
                (fixed, LoadOutcome::Parsed),
                &mut announced
            ));
            assert!(!announced);
            assert_eq!(
                dispatched(cx, "ctrl-alt-8"),
                vec![SplitRight::name_for_type()]
            );
            assert!(
                dispatched(cx, "ctrl-alt-9").is_empty(),
                "the chord the fixed file dropped is retired"
            );
        });
    }

    /// The watcher covers the whole config directory, and the themes half of
    /// it has to keep reloading even when config.json does not parse.
    #[test]
    fn only_theme_files_beside_the_config_count_as_themes() {
        use std::path::Path;
        assert!(is_theme_file(Path::new("/cfg/themes/solar.yaml")));
        assert!(is_theme_file(Path::new("/cfg/themes/solar.YML")));
        assert!(is_theme_file(Path::new("/cfg/themes/solar.itermcolors")));
        assert!(!is_theme_file(Path::new("/cfg/themes/notes.txt")));
        assert!(!is_theme_file(Path::new("/cfg/config.json")));
        assert!(!is_theme_file(Path::new("/cfg/solar.yaml")));
    }
}

