//! Reactive application-wide configuration store.
//!
//! Centralizes mutation, persistence, rollback, and save-error tracking for the
//! GPUI global [`Config`] so detached windows and feature entities (such as
//! `SettingsWindow`, `ForwardsView`, and `RightPanelView`) can update settings
//! directly without routing through `AntermApp`.
//!
//! Every write goes through GPUI's global mutation path, which automatically
//! notifies all `cx.observe_global::<Config>(...)` subscribers across every
//! open window.

use gpui::{App, BorrowAppContext as _, Global};

use crate::config::Config;

/// Tracks the last cleanly persisted snapshot and any pending save error.
///
/// Stored as a separate GPUI global so views displaying persistence status can
/// observe save failures independently of high-frequency config reads.
#[derive(Clone, Debug, Default)]
pub struct ConfigSaveState {
    saved_snapshot: Config,
    last_error: Option<String>,
}

impl Global for ConfigSaveState {}

/// Narrow facade for reading, mutating, persisting, and rolling back [`Config`].
pub struct ConfigStore;

impl ConfigStore {
    /// Installs `config` as both the live [`Config`] global and the initial
    /// cleanly persisted snapshot.
    pub fn init(config: Config, cx: &mut App) {
        let snapshot = config.clone();
        cx.set_global(config);
        cx.set_global(ConfigSaveState {
            saved_snapshot: snapshot,
            last_error: None,
        });
    }

    /// Returns the live configuration.
    #[inline]
    pub fn read(cx: &App) -> &Config {
        cx.global::<Config>()
    }

    /// Mutates the live [`Config`] in memory without writing to disk.
    ///
    /// Appropriate for transient previews (for example, hovering a theme preset
    /// in the command palette) that may be rolled back without persisting.
    pub fn update_in_memory<R>(cx: &mut App, mutate: impl FnOnce(&mut Config) -> R) -> R {
        cx.update_global::<Config, _>(|cfg, _| mutate(cfg))
    }

    /// Mutates the live [`Config`], notifies all `observe_global::<Config>`
    /// subscribers, and persists the updated configuration to disk.
    ///
    /// Returns `Ok(())` on successful save, or `Err(message)` if writing to
    /// disk failed (keeping the error in [`ConfigSaveState`] so callers can
    /// offer retry or discard).
    pub fn update(cx: &mut App, mutate: impl FnOnce(&mut Config)) -> Result<(), String> {
        Self::ensure_save_state(cx);
        cx.update_global::<Config, _>(|cfg, _| mutate(cfg));
        Self::persist_current(cx)
    }

    /// Persists the current live [`Config`] global to disk and updates
    /// [`ConfigSaveState`].
    pub fn persist_current(cx: &mut App) -> Result<(), String> {
        Self::ensure_save_state(cx);
        let config = cx.global::<Config>().clone();
        match config.try_save() {
            Ok(()) => {
                cx.update_global::<ConfigSaveState, _>(|state, _| {
                    state.saved_snapshot = config;
                    state.last_error = None;
                });
                Ok(())
            }
            Err(error) => {
                let message = error.to_string();
                log::warn!("failed to save settings: {message}");
                cx.update_global::<ConfigSaveState, _>(|state, _| {
                    state.last_error = Some(message.clone());
                });
                Err(message)
            }
        }
    }

    /// Replaces the live configuration from an external disk reload, clearing
    /// any prior save error and advancing the saved snapshot.
    pub fn replace_from_disk(config: Config, cx: &mut App) {
        let snapshot = config.clone();
        cx.set_global(config);
        cx.set_global(ConfigSaveState {
            saved_snapshot: snapshot,
            last_error: None,
        });
    }

    /// Rolls the live [`Config`] back to the last persisted snapshot when a
    /// save error is active. Returns `true` if a rollback was performed.
    pub fn discard_unsaved(cx: &mut App) -> bool {
        let Some(snapshot) = cx
            .try_global::<ConfigSaveState>()
            .filter(|state| state.last_error.is_some())
            .map(|state| state.saved_snapshot.clone())
        else {
            return false;
        };
        cx.set_global(snapshot);
        cx.update_global::<ConfigSaveState, _>(|state, _| {
            state.last_error = None;
        });
        true
    }

    /// Returns the most recent persistence error, if saving the live config
    /// failed and has not yet been retried or discarded.
    pub fn save_error(cx: &App) -> Option<&str> {
        cx.try_global::<ConfigSaveState>()
            .and_then(|state| state.last_error.as_deref())
    }

    fn ensure_save_state(cx: &mut App) {
        if !cx.has_global::<ConfigSaveState>() {
            let snapshot = cx.global::<Config>().clone();
            cx.set_global(ConfigSaveState {
                saved_snapshot: snapshot,
                last_error: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Context, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    struct ConfigObserver {
        seen_font_size: Rc<Cell<f32>>,
        notifications: Rc<Cell<usize>>,
    }

    impl ConfigObserver {
        fn new(
            seen_font_size: Rc<Cell<f32>>,
            notifications: Rc<Cell<usize>>,
            cx: &mut Context<Self>,
        ) -> Self {
            cx.observe_global::<Config>(|this, cx| {
                this.seen_font_size.set(ConfigStore::read(cx).font_size);
                this.notifications.set(this.notifications.get() + 1);
            })
            .detach();
            Self {
                seen_font_size,
                notifications,
            }
        }
    }

    #[gpui::test]
    fn update_persists_and_notifies_independent_observers(cx: &mut TestAppContext) {
        crate::config::pin_test_config_dir();
        let seen = Rc::new(Cell::new(0.0f32));
        let notifications = Rc::new(Cell::new(0usize));

        // First turn: install config and register observer (GPUI activates
        // `observe_global` subscriptions on defer at the end of the turn).
        let _observer = cx.update(|cx| {
            ConfigStore::init(Config::default(), cx);
            cx.new(|cx| ConfigObserver::new(seen.clone(), notifications.clone(), cx))
        });

        // Second turn: mutate through `ConfigStore::update` and verify notification.
        cx.update(|cx| {
            let res = ConfigStore::update(cx, |cfg| {
                cfg.font_size = 19.0;
            });
            assert!(res.is_ok(), "save into scratch dir should succeed: {res:?}");
            assert!(ConfigStore::save_error(cx).is_none());
        });
        cx.run_until_parked();

        assert_eq!(seen.get(), 19.0);
        assert!(notifications.get() >= 1);
    }

    #[gpui::test]
    fn replace_from_disk_clears_error_and_notifies(cx: &mut TestAppContext) {
        crate::config::pin_test_config_dir();
        let seen = Rc::new(Cell::new(0.0f32));
        let notifications = Rc::new(Cell::new(0usize));

        let _observer = cx.update(|cx| {
            ConfigStore::init(Config::default(), cx);
            cx.update_global::<ConfigSaveState, _>(|state, _| {
                state.last_error = Some("disk full".into());
            });
            cx.new(|cx| ConfigObserver::new(seen.clone(), notifications.clone(), cx))
        });

        cx.update(|cx| {
            let mut reloaded = Config::default();
            reloaded.font_size = 16.5;
            ConfigStore::replace_from_disk(reloaded, cx);
            assert!(ConfigStore::save_error(cx).is_none());
        });
        cx.run_until_parked();

        assert_eq!(seen.get(), 16.5);
        assert!(notifications.get() >= 1);
    }

    #[gpui::test]
    fn discard_unsaved_restores_saved_snapshot_when_save_failed(cx: &mut TestAppContext) {
        crate::config::pin_test_config_dir();
        cx.update(|cx| {
            let mut initial = Config::default();
            initial.font_size = 14.0;
            ConfigStore::init(initial, cx);

            // Simulate an in-memory edit whose disk persistence failed.
            ConfigStore::update_in_memory(cx, |cfg| cfg.font_size = 22.0);
            cx.update_global::<ConfigSaveState, _>(|state, _| {
                state.last_error = Some("permission denied".into());
            });

            assert_eq!(ConfigStore::read(cx).font_size, 22.0);
            assert!(ConfigStore::discard_unsaved(cx));
            assert_eq!(ConfigStore::read(cx).font_size, 14.0);
            assert!(ConfigStore::save_error(cx).is_none());
            // Second discard is a no-op once clean.
            assert!(!ConfigStore::discard_unsaved(cx));
        });
    }
}
