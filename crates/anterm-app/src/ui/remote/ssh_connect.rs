use crate::core::config::Config;
use crate::core::keychain::OsCredentialStore;
use crate::core::ssh_profile::SshProfile;
use crate::daemon::protocol::NativeSshSpec;

use crate::ui::workspace::app::{SpawnAs, SpawnWhere, AntermApp};

impl AntermApp {
    pub(crate) fn native_ssh_spec_for_profile(
        &self,
        profile: &SshProfile,
        cx: &gpui::App,
    ) -> NativeSshSpec {
        let cfg = cx.global::<Config>();
        build_native_ssh_spec(
            profile,
            &cfg.ssh_profiles,
            &OsCredentialStore,
            cfg.verify_host_keys,
        )
    }

    pub(crate) fn connect_ssh_profile(
        &mut self,
        profile_id: uuid::Uuid,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.connect_ssh_profile_at(profile_id, SpawnWhere::NewTab, window, cx);
    }

    /// A saved host opened where the caller asks for it — a tab of its own, or
    /// beside the pane in front of the user when the new-tab menu's row was
    /// taken with ⌥ held.
    pub(crate) fn connect_ssh_profile_at(
        &mut self,
        profile_id: uuid::Uuid,
        at: SpawnWhere,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(profile) = cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .cloned()
        else {
            return;
        };
        self.bump_ssh_frecency(profile_id, cx);
        let spec = Box::new(self.native_ssh_spec_for_profile(&profile, cx));
        match at {
            SpawnWhere::NewTab => self.open_native_ssh_tab(spec, None, window, cx),
            SpawnWhere::Split => {
                self.split_into(gpui::Axis::Horizontal, Some(SpawnAs::Ssh(spec)), window, cx)
            }
        }
    }

    pub(crate) fn quick_connect(
        &mut self,
        qc: crate::core::ssh_profile::QuickConnect,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(resolved) = crate::core::ssh_config::resolve_alias_to_profile(&qc.host) {
            let mut profile = resolved.profile;
            if let Some(user) = qc.user {
                profile.user = user;
            }
            if let Some(port) = qc.port {
                profile.port = port;
            }
            let spec = native_spec_from_transient_profile(
                &profile,
                resolved.proxy_jump,
                &OsCredentialStore,
                cx.global::<Config>().verify_host_keys,
                &config_alias_resolver,
            );
            self.open_native_ssh_tab(Box::new(spec), None, window, cx);
            return;
        }
        let port = qc.port_or_default();
        let mut profile = SshProfile::new(qc.host.clone());
        profile.host = qc.host;
        profile.port = port;
        if let Some(user) = qc.user {
            profile.user = user;
        }
        let spec = Box::new(self.native_ssh_spec_for_profile(&profile, cx));
        self.open_native_ssh_tab(spec, None, window, cx);
    }

    pub(crate) fn restart_ssh_session(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(view) = self.focused_pane_view(window, cx) else {
            return;
        };
        let dead_spec = {
            let v = view.read(cx);
            if !v.ssh_disconnected() {
                return;
            }
            v.ssh_spec()
        };
        let Some(spec) = dead_spec else {
            return;
        };
        let resolved = self.resolve_restart_spec(spec, cx);
        self.respawn_native_ssh_in_place(&view, resolved, window, cx);
    }

    fn resolve_restart_spec(
        &self,
        spec: Box<crate::daemon::protocol::NativeSshSpec>,
        cx: &gpui::App,
    ) -> Box<crate::daemon::protocol::NativeSshSpec> {
        resolve_persisted_ssh_spec(spec, cx)
    }

    fn focused_pane_view(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<crate::terminal::view::TerminalView>> {
        self.tabs
            .get(self.active)?
            .pane
            .focused_or_first(window, cx)
    }

    /// Open the host form on a blank profile — the "add a host" route for
    /// everywhere outside the settings list, which is where the only `+` used
    /// to live.
    pub(crate) fn open_new_ssh_host(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.open_ssh_profile_form(SshProfile::new(String::new()), window, cx);
    }

    /// The connection in the focused pane, when it was dialled by hand rather
    /// than opened from a saved host. What "Save Connection as Host" acts on,
    /// and what decides whether the command is offered at all.
    pub(crate) fn unsaved_ssh_session(
        &self,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> Option<Box<NativeSshSpec>> {
        let spec = self.focused_pane_view(window, cx)?.read(cx).ssh_spec()?;
        let profiles = &cx.global::<Config>().ssh_profiles;
        // A transient profile is handed a fresh uuid on its way to the daemon,
        // so an id alone does not mean a host was saved — only one that still
        // resolves does.
        let saved = spec
            .profile_id
            .as_deref()
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .is_some_and(|id| profiles.iter().any(|p| p.id == id));
        (!saved).then_some(spec)
    }

    /// Keep the connection in front of you: the form opens on everything the
    /// live session was dialled with, so a host proved by hand becomes a saved
    /// one without retyping it (#438).
    pub(crate) fn save_ssh_session_as_host(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(spec) = self.unsaved_ssh_session(window, cx) else {
            return;
        };
        self.save_ssh_spec_as_host(&spec, window, cx);
    }

    /// The same form, for a connection named by the caller rather than by the
    /// focus — the tab menu's row offers it for the tab it was opened on.
    pub(crate) fn save_ssh_spec_as_host(
        &mut self,
        spec: &NativeSshSpec,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let profile = profile_from_live_spec(spec);
        let jumped = spec.jump.is_some();
        self.open_settings_then(window, cx, move |settings, window, cx| {
            settings.with_settings_edits_resolved(window, cx, move |settings, window, cx| {
                settings.navigate_settings(
                    crate::ui::chrome::settings::SettingsSection::Ssh,
                    None,
                    window,
                    cx,
                );
                settings.ssh_form_load(&profile, window, cx);
                if jumped {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(
                        anterm_i18n::t(anterm_i18n::L10nKey::SshSaveDroppedJumpHost),
                        cx,
                    );
                }
            });
        });
    }

    /// The host form a tab's own context menu offers, and what the row calls
    /// it — read off the tab the menu was opened on rather than off whichever
    /// pane happens to be focused, so right-clicking a background tab reaches
    /// that tab's connection.
    ///
    /// `None` for a tab there is no host form to open (a local shell).
    pub(crate) fn tab_ssh_host_form(
        &self,
        index: usize,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> Option<(TabHostForm, &'static str)> {
        let leaf = self.tabs.get(index)?.pane.focused_or_first(window, cx)?;
        let spec = leaf.read(cx).ssh_spec()?;
        let form = match saved_profile_id_of_spec(&spec, &cx.global::<Config>().ssh_profiles) {
            Some(id) => TabHostForm::Saved(id),
            None => TabHostForm::Unsaved(spec),
        };
        let label = host_form_label(&form);
        Some((form, label))
    }

    /// Open what the row offered. A saved host goes to its own record; an
    /// unsaved one goes through the same "save this connection" path the
    /// command already uses, so the proxy, the identity files and the forwards
    /// the session was dialled with land in the draft rather than being
    /// thrown away with everything that does not fit in `user@host:port`.
    pub(crate) fn open_tab_ssh_host_form(
        &mut self,
        form: &TabHostForm,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match form {
            TabHostForm::Saved(id) => self.open_ssh_profile_in_settings(*id, window, cx),
            TabHostForm::Unsaved(spec) => self.save_ssh_spec_as_host(spec, window, cx),
        }
    }

    fn bump_ssh_frecency(&mut self, profile_id: uuid::Uuid, cx: &mut gpui::Context<Self>) {
        self.update_config(cx, |cfg| {
            let entry = cfg.ssh_profile_frecency.entry(profile_id).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used = crate::core::config::unix_now();
        });
    }
}

pub(crate) use anterm_ui_kit::ssh_helpers::*;
