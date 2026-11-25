//! Pure SSH profile/spec resolution, frecency sorting, forward mapping, and
//! `ssh ...` command-line parsing shared across settings, search, forwards,
//! and workspace session spawning.

use std::collections::{HashMap, HashSet};

use anterm_core::daemon::protocol::{
    NativeSshSpec, SshAlgorithms, SshAuthMode, SshForwardKind, SshForwardRule, SshProxy,
    ssh_option_takes_value,
};
use anterm_core::keychain::key_account_from_contents;
use anterm_core::ssh_profile::{
    Algorithms, AuthMode, ForwardKind, ForwardRule, HostPort, SshProfile,
};
use anterm_i18n::{L10nKey, t, t_fmt};
use uuid::Uuid;

use crate::config::Config;
use crate::keychain::{CredentialStore, OsCredentialStore};

/// Saved hosts, most likely first: whatever has been connected to often and
/// recently, then alphabetically for everything nobody has used yet. Shared by
/// the palette and the new-tab menu so the same host leads both lists.
pub fn ssh_profiles_by_frecency(cx: &gpui::App) -> Vec<SshProfile> {
    let cfg = cx.global::<Config>();
    let now = crate::config::unix_now();
    let mut profiles = cfg.ssh_profiles.clone();
    profiles.sort_by(|a, b| {
        let score = |p: &SshProfile| {
            cfg.ssh_profile_frecency
                .get(&p.id)
                .map(|u| u.score(now))
                .unwrap_or(0.0)
        };
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    profiles
}

pub fn resolve_persisted_ssh_spec(
    spec: Box<anterm_core::daemon::protocol::NativeSshSpec>,
    cx: &gpui::App,
) -> Box<anterm_core::daemon::protocol::NativeSshSpec> {
    let cfg = cx.global::<Config>();
    let profile = spec
        .profile_id
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .and_then(|id| cfg.ssh_profiles.iter().find(|p| p.id == id).cloned());
    match profile {
        Some(p) => Box::new(build_native_ssh_spec(
            &p,
            &cfg.ssh_profiles,
            &OsCredentialStore,
            cfg.verify_host_keys,
        )),
        None => spec,
    }
}

pub fn build_native_ssh_spec(
    profile: &SshProfile,
    profiles: &[SshProfile],
    store: &dyn CredentialStore,
    global_verify_host_keys: bool,
) -> NativeSshSpec {
    let mut visited = HashSet::new();
    visited.insert(profile.id);
    build_spec_inner(
        profile,
        profiles,
        store,
        global_verify_host_keys,
        &mut visited,
    )
}

fn build_spec_inner(
    profile: &SshProfile,
    profiles: &[SshProfile],
    store: &dyn CredentialStore,
    global_verify_host_keys: bool,
    visited: &mut HashSet<Uuid>,
) -> NativeSshSpec {
    let identity_files = profile.expanded_identity_files();

    let password = if matches!(profile.auth, AuthMode::Auto | AuthMode::Password) {
        store
            .password_for(&profile.user, &profile.host, profile.port)
            .ok()
            .flatten()
    } else {
        None
    };

    let mut key_passphrases: HashMap<String, String> = HashMap::new();
    if matches!(profile.auth, AuthMode::Auto | AuthMode::PublicKey) {
        // Whichever list the daemon will actually offer (#484, #513): the
        // profile's own files, or — only when it names none — the same
        // `~/.ssh` defaults, from the one shared candidate list. The daemon
        // looks passphrases up by the candidate string, so both sides must
        // spell them identically.
        let owned = identity_files.clone();
        let probed = if owned.is_empty() {
            anterm_core::ssh_profile::default_identity_candidates()
        } else {
            owned
        };
        for path in &probed {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let account = key_account_from_contents(&bytes);
            if let Ok(Some(passphrase)) = store.passphrase_for_key(&account) {
                key_passphrases.insert(path.clone(), passphrase);
            }
        }
    }

    let jump = profile
        .jump_host
        .and_then(|id| {
            if visited.contains(&id) {
                return None;
            }
            profiles.iter().find(|p| p.id == id)
        })
        .map(|jp| {
            visited.insert(jp.id);
            Box::new(build_spec_inner(
                jp,
                profiles,
                store,
                global_verify_host_keys,
                visited,
            ))
        });

    NativeSshSpec {
        host: profile.host.clone(),
        port: profile.port,
        user: anterm_core::ssh_profile::login_user(&profile.user),
        auth_mode: map_auth_mode(profile.auth),
        identity_files,
        agent_forward: profile.agent_forward,
        password,
        key_passphrases: (!key_passphrases.is_empty()).then_some(key_passphrases),
        proxy: map_proxy(profile),
        jump,
        forwards: profile.forwards.iter().map(map_forward).collect(),
        keepalive_interval_s: profile.keepalive_interval_s,
        keepalive_count_max: profile.keepalive_count_max,
        connect_timeout_s: profile.connect_timeout_s,
        algorithms: map_algorithms(&profile.algorithms),
        x11: profile.x11,
        term: "xterm-256color".to_string(),
        verify_host_keys: profile.verify_host_keys.unwrap_or(global_verify_host_keys),
        skip_banner: profile.skip_banner,
        shell_integration: profile.shell_integration,
        remote_clipboard_write: profile.remote_clipboard_write,
        login_script: profile.login_scripts.clone(),
        // What the pane calls itself before the remote shell says anything.
        // A nameless profile — every host imported from `~/.ssh/config` is
        // one — falls back to its address rather than to nothing, which is
        // what left ad-hoc connections all sharing the name "anterm" (#438).
        display_name: Some(match profile.name.trim() {
            "" => anterm_core::ssh_profile::to_connect_string(profile),
            name => name.to_string(),
        }),
        profile_id: Some(profile.id.to_string()),
    }
}

/// What a tab's host row opens when it is taken.
///
/// The two halves are not the same form. A saved host is already a record, so
/// it is addressed by the target that names it and nothing about the live
/// session is needed. An unsaved one is only ever the session, and it goes to
/// the form whole: an address dialled by hand carries a proxy, a jump host,
/// identity files and forwards, and a draft built from `user@host:port` alone
/// would save fine and then not connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabHostForm {
    Saved(Uuid),
    Unsaved(Box<NativeSshSpec>),
}

/// Label for a tab's SSH host form row.
pub fn host_form_label(form: &TabHostForm) -> &'static str {
    match form {
        TabHostForm::Saved(_) => anterm_i18n::t(anterm_i18n::L10nKey::SwitcherEditHost),
        TabHostForm::Unsaved(_) => anterm_i18n::t(anterm_i18n::L10nKey::SwitcherSaveAsHost),
    }
}

/// Returns the saved profile id for `spec` if it still resolves against
/// `profiles`; otherwise `None` (an ad-hoc or deleted profile).
pub fn saved_profile_id_of_spec(
    spec: &NativeSshSpec,
    profiles: &[SshProfile],
) -> Option<Uuid> {
    spec.profile_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .filter(|id| profiles.iter().any(|p| p.id == *id))
}

/// A live connection read back as a profile someone could keep — the return
/// leg of [`build_native_ssh_spec`], for a session that was dialled by hand
/// and turned out to be worth saving.
///
/// Secrets are not among the fields: the spec a pane holds has been through
/// `without_secrets`, and a password belongs in the keychain the form writes
/// to, not in a draft. Neither is `verify_host_keys`, which the spec carries
/// already resolved against the global setting — copying it in would pin a
/// per-host override nobody asked for. The jump host is the one thing that
/// cannot come along: a profile names its jump by the id of another profile,
/// and an ad-hoc `-J` hop is not one. Callers say so out loud.
pub fn profile_from_live_spec(spec: &NativeSshSpec) -> SshProfile {
    let mut profile = SshProfile::new(String::new());
    profile.host = spec.host.clone();
    profile.port = spec.port;
    profile.user = spec.user.clone();
    profile.auth = unmap_auth_mode(spec.auth_mode);
    profile.identity_files = spec.identity_files.clone();
    profile.agent_forward = spec.agent_forward;
    match &spec.proxy {
        SshProxy::None => {}
        SshProxy::Command(cmd) => profile.proxy_command = Some(cmd.clone()),
        SshProxy::Socks { host, port } => {
            profile.socks_proxy = Some(HostPort::new(host.clone(), *port))
        }
        SshProxy::Http { host, port } => {
            profile.http_proxy = Some(HostPort::new(host.clone(), *port))
        }
    }
    profile.forwards = spec.forwards.iter().map(unmap_forward).collect();
    profile.keepalive_interval_s = spec.keepalive_interval_s;
    profile.keepalive_count_max = spec.keepalive_count_max;
    profile.connect_timeout_s = spec.connect_timeout_s;
    profile.skip_banner = spec.skip_banner;
    profile.shell_integration = spec.shell_integration;
    profile.remote_clipboard_write = spec.remote_clipboard_write;
    profile.login_scripts = spec.login_script.clone();
    profile.x11 = spec.x11;
    profile.algorithms = Algorithms {
        kex: spec.algorithms.kex.clone(),
        cipher: spec.algorithms.cipher.clone(),
        mac: spec.algorithms.mac.clone(),
        hostkey: spec.algorithms.host_key.clone(),
        compression: spec.algorithms.compression.clone(),
    };
    profile.name = anterm_core::ssh_profile::to_connect_string(&profile);
    profile
}

fn unmap_auth_mode(auth: SshAuthMode) -> AuthMode {
    match auth {
        SshAuthMode::Auto => AuthMode::Auto,
        SshAuthMode::Gssapi => AuthMode::Gssapi,
        SshAuthMode::Password => AuthMode::Password,
        SshAuthMode::PublicKey => AuthMode::PublicKey,
        SshAuthMode::Agent => AuthMode::Agent,
        SshAuthMode::KeyboardInteractive => AuthMode::KeyboardInteractive,
    }
}

fn unmap_forward(rule: &SshForwardRule) -> ForwardRule {
    ForwardRule {
        kind: match rule.kind {
            SshForwardKind::Local => ForwardKind::Local,
            SshForwardKind::Remote => ForwardKind::Remote,
            SshForwardKind::Dynamic => ForwardKind::Dynamic,
        },
        bind: HostPort::new(rule.bind_host.clone(), rule.bind_port),
        target: HostPort::new(rule.target_host.clone(), rule.target_port),
        description: rule.description.clone().unwrap_or_default(),
        enabled: rule.enabled,
    }
}

fn map_auth_mode(auth: AuthMode) -> SshAuthMode {
    match auth {
        AuthMode::Auto => SshAuthMode::Auto,
        AuthMode::Gssapi => SshAuthMode::Gssapi,
        AuthMode::Password => SshAuthMode::Password,
        AuthMode::PublicKey => SshAuthMode::PublicKey,
        AuthMode::Agent => SshAuthMode::Agent,
        AuthMode::KeyboardInteractive => SshAuthMode::KeyboardInteractive,
    }
}

fn map_proxy(profile: &SshProfile) -> SshProxy {
    if let Some(cmd) = &profile.proxy_command {
        if !cmd.trim().is_empty() {
            return SshProxy::Command(cmd.clone());
        }
    }
    // Port 0 is not somewhere a proxy listens. The settings form used to write
    // it whenever the address had no port or an unparseable one, so configs
    // carrying it are already on disk; connecting direct is the honest reading
    // of an address that names nowhere.
    if let Some(HostPort { host, port }) = &profile.socks_proxy {
        if !host.is_empty() && *port != 0 {
            return SshProxy::Socks {
                host: host.clone(),
                port: *port,
            };
        }
    }
    if let Some(HostPort { host, port }) = &profile.http_proxy {
        if !host.is_empty() && *port != 0 {
            return SshProxy::Http {
                host: host.clone(),
                port: *port,
            };
        }
    }
    SshProxy::None
}

pub fn map_forward(rule: &ForwardRule) -> SshForwardRule {
    SshForwardRule {
        kind: match rule.kind {
            ForwardKind::Local => SshForwardKind::Local,
            ForwardKind::Remote => SshForwardKind::Remote,
            ForwardKind::Dynamic => SshForwardKind::Dynamic,
        },
        bind_host: rule.bind.host.clone(),
        bind_port: rule.bind.port,
        target_host: rule.target.host.clone(),
        target_port: rule.target.port,
        description: (!rule.description.is_empty()).then(|| rule.description.clone()),
        enabled: rule.enabled,
    }
}

fn map_algorithms(a: &Algorithms) -> SshAlgorithms {
    SshAlgorithms {
        kex: a.kex.clone(),
        cipher: a.cipher.clone(),
        mac: a.mac.clone(),
        host_key: a.hostkey.clone(),
        compression: a.compression.clone(),
    }
}

pub type AliasResolver<'a> = dyn Fn(&str) -> Option<(SshProfile, Option<String>)> + 'a;

pub fn config_alias_resolver(alias: &str) -> Option<(SshProfile, Option<String>)> {
    anterm_core::ssh_config::resolve_alias_to_profile(alias).map(|r| (r.profile, r.proxy_jump))
}

pub fn native_spec_from_transient_profile(
    profile: &SshProfile,
    proxy_jump: Option<String>,
    store: &dyn CredentialStore,
    global_verify_host_keys: bool,
    resolve_alias: &AliasResolver<'_>,
) -> NativeSshSpec {
    let mut spec = build_native_ssh_spec(profile, &[], store, global_verify_host_keys);
    if let Some(raw) = proxy_jump {
        let mut visited = HashSet::new();
        visited.insert(profile.name.clone());
        spec.jump = resolve_jump_chain(
            &raw,
            store,
            global_verify_host_keys,
            resolve_alias,
            &mut visited,
        );
    }
    spec
}

fn resolve_jump_chain(
    raw: &str,
    store: &dyn CredentialStore,
    verify: bool,
    resolve_alias: &AliasResolver<'_>,
    visited: &mut HashSet<String>,
) -> Option<Box<NativeSshSpec>> {
    let hops: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    build_jump_from_hops(&hops, store, verify, resolve_alias, visited)
}

fn build_jump_from_hops(
    hops: &[&str],
    store: &dyn CredentialStore,
    verify: bool,
    resolve_alias: &AliasResolver<'_>,
    visited: &mut HashSet<String>,
) -> Option<Box<NativeSshSpec>> {
    let (last, earlier) = hops.split_last()?;
    if !visited.insert((*last).to_string()) {
        return None;
    }
    let (profile, own_jump) = match resolve_alias(last) {
        Some((profile, own_jump)) => (profile, if earlier.is_empty() { own_jump } else { None }),
        None => (transient_profile_from_target(last)?, None),
    };
    let mut spec = build_native_ssh_spec(&profile, &[], store, verify);
    spec.jump = if !earlier.is_empty() {
        build_jump_from_hops(earlier, store, verify, resolve_alias, visited)
    } else if let Some(own_jump) = own_jump {
        resolve_jump_chain(&own_jump, store, verify, resolve_alias, visited)
    } else {
        None
    };
    Some(Box::new(spec))
}

fn transient_profile_from_target(target: &str) -> Option<SshProfile> {
    let qc = anterm_core::ssh_profile::parse_quick_connect(target)?;
    let mut profile = SshProfile::new(qc.host.clone());
    profile.port = qc.port_or_default();
    profile.host = qc.host;
    if let Some(user) = qc.user {
        profile.user = user;
    }
    Some(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::InMemoryCredentialStore;

    fn profile(name: &str, host: &str, user: &str) -> SshProfile {
        let mut p = SshProfile::new(name);
        p.host = host.into();
        p.user = user.into();
        p
    }

    #[test]
    fn resolves_stored_password_for_auto_and_password_modes() {
        let store = InMemoryCredentialStore::new();
        store
            .set_password("deploy", "10.0.0.5", 22, "hunter2")
            .unwrap();
        let mut p = profile("web", "10.0.0.5", "deploy");

        p.auth = AuthMode::Auto;
        let spec = build_native_ssh_spec(&p, &[], &store, true);
        assert_eq!(spec.password.as_deref(), Some("hunter2"));

        p.auth = AuthMode::Password;
        let spec = build_native_ssh_spec(&p, &[], &store, true);
        assert_eq!(spec.password.as_deref(), Some("hunter2"));

        p.auth = AuthMode::PublicKey;
        let spec = build_native_ssh_spec(&p, &[], &store, true);
        assert_eq!(spec.password, None);
    }

    /// A pane's route is built on the UI thread with the keychain left out of
    /// it; that must describe the same connection as the full spec with its
    /// secrets stripped, jump host and all.
    #[test]
    fn a_spec_built_without_credentials_is_the_full_one_stripped() {
        let store = InMemoryCredentialStore::new();
        store
            .set_password("deploy", "10.0.0.5", 22, "hunter2")
            .unwrap();
        store.set_password("ops", "bastion", 22, "s3cret").unwrap();
        let jump = profile("bastion", "bastion", "ops");
        let mut p = profile("web", "10.0.0.5", "deploy");
        p.jump_host = Some(jump.id);
        let profiles = [p.clone(), jump];

        let full = build_native_ssh_spec(&p, &profiles, &store, true);
        assert_eq!(full.password.as_deref(), Some("hunter2"));
        assert_eq!(
            build_native_ssh_spec(&p, &profiles, &crate::keychain::NoCredentials, true),
            full.without_secrets()
        );
    }

    /// The pane wears this until the remote shell titles itself, so a host
    /// nobody bothered to name still says where it went (#438). Every host
    /// imported from `~/.ssh/config` used to arrive nameless, and every one
    /// of them opened a tab called "anterm".
    #[test]
    fn a_spec_names_itself_after_the_profile_or_its_address() {
        let store = InMemoryCredentialStore::new();

        let named = profile("prod-web", "10.0.0.5", "deploy");
        let spec = build_native_ssh_spec(&named, &[], &store, true);
        assert_eq!(spec.display_name.as_deref(), Some("prod-web"));

        let mut nameless = profile("", "10.0.0.5", "deploy");
        let spec = build_native_ssh_spec(&nameless, &[], &store, true);
        assert_eq!(spec.display_name.as_deref(), Some("deploy@10.0.0.5"));

        nameless.port = 2222;
        let spec = build_native_ssh_spec(&nameless, &[], &store, true);
        assert_eq!(spec.display_name.as_deref(), Some("deploy@10.0.0.5:2222"));
    }

    /// "Save Connection as Host" opens the form on this, so anything it drops
    /// is something the user typed once and has to type again. The jump host
    /// is the one exception, and the caller says so out loud.
    #[test]
    fn a_live_connection_reads_back_as_the_profile_it_was_dialled_from() {
        let store = InMemoryCredentialStore::new();
        let mut p = profile("prod-web", "10.0.0.5", "deploy");
        p.port = 2222;
        p.auth = AuthMode::PublicKey;
        p.identity_files = vec!["/keys/id_ed25519".into()];
        p.agent_forward = true;
        p.x11 = true;
        p.skip_banner = true;
        p.remote_clipboard_write = true;
        p.socks_proxy = Some(HostPort::new("127.0.0.1", 1080));
        p.keepalive_interval_s = Some(30);
        p.connect_timeout_s = Some(9);
        p.login_scripts = vec!["tmux attach".into()];
        p.algorithms.cipher = vec!["aes256-gcm@openssh.com".into()];
        p.forwards = vec![ForwardRule {
            kind: ForwardKind::Local,
            bind: HostPort::new("localhost", 8080),
            target: HostPort::new("127.0.0.1", 80),
            description: "web".into(),
            enabled: true,
        }];
        // A switched-off rule survives the trip through a live spec switched
        // off, or reopening a host from its connection would turn it back on.
        p.forwards.push(ForwardRule {
            bind: HostPort::new("localhost", 8080),
            target: HostPort::new("10.0.0.7", 80),
            enabled: false,
            ..ForwardRule::default()
        });

        let spec = build_native_ssh_spec(&p, &[], &store, true);
        let back = profile_from_live_spec(&spec);

        assert_eq!(back.host, p.host);
        assert_eq!(back.port, p.port);
        assert_eq!(back.user, p.user);
        assert_eq!(back.auth, p.auth);
        assert_eq!(back.identity_files, p.identity_files);
        assert!(back.agent_forward && back.x11 && back.skip_banner && back.remote_clipboard_write);
        assert_eq!(back.socks_proxy, p.socks_proxy);
        assert_eq!(back.keepalive_interval_s, p.keepalive_interval_s);
        assert_eq!(back.connect_timeout_s, p.connect_timeout_s);
        assert_eq!(back.login_scripts, p.login_scripts);
        assert_eq!(back.algorithms.cipher, p.algorithms.cipher);
        assert_eq!(back.forwards, p.forwards);
        assert_eq!(
            back.name, "deploy@10.0.0.5:2222",
            "a connection dialled by hand was never named, so the form opens on its address"
        );
        assert_ne!(back.id, p.id, "saving it makes a host of its own");
        assert_eq!(
            back.verify_host_keys, None,
            "the spec carries the global setting resolved; copying it back would pin an override"
        );
    }

    /// Both halves of "remember passphrase" key the keychain entry off the
    /// *contents* of the key file, so the read-back here and the write in
    /// `ui::ssh_prompt` only ever meet if both expand a leading `~/` first.
    /// They do — `expanded_identity_files` runs the path through
    /// `expand_tilde` — and this pins that down from the outside: whichever
    /// way the profile spells the path, the spec lists it expanded and files
    /// the passphrase under that same string, which is the key the daemon's
    /// `spec.key_passphrases` lookup uses.
    #[test]
    fn a_tilde_and_an_absolute_identity_path_resolve_the_same_stored_passphrase() {
        let home = anterm_core::ssh_profile::expand_tilde("~");
        let dir = tempfile::Builder::new()
            .prefix("anterm-key-test")
            .tempdir_in(&home)
            .expect("the home directory is writable");
        let key = dir.path().join("id_ed25519");
        std::fs::write(&key, b"-----BEGIN OPENSSH PRIVATE KEY-----\nencrypted\n")
            .expect("the temp directory is writable");

        let store = InMemoryCredentialStore::new();
        let account = key_account_from_contents(&std::fs::read(&key).unwrap());
        store.set_key_passphrase(&account, "pp").unwrap();

        let leaf = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut p = profile("web", "10.0.0.5", "deploy");
        p.auth = AuthMode::PublicKey;

        for spelling in [
            format!("~/{leaf}/id_ed25519"),
            key.to_string_lossy().to_string(),
        ] {
            p.identity_files = vec![spelling.clone()];
            let spec = build_native_ssh_spec(&p, &[], &store, true);
            let listed = spec
                .identity_files
                .first()
                .expect("the profile lists one key");
            assert!(!listed.starts_with('~'), "{spelling} was left unexpanded");
            assert_eq!(
                spec.key_passphrases
                    .as_ref()
                    .and_then(|m| m.get(listed))
                    .map(String::as_str),
                Some("pp"),
                "{spelling} should resolve its stored passphrase"
            );
        }

        // A mode that will never offer the key does not go looking for its
        // secret either.
        p.auth = AuthMode::Password;
        assert!(
            build_native_ssh_spec(&p, &[], &store, true)
                .key_passphrases
                .is_none()
        );
    }

    #[test]
    fn resolves_jump_chain_into_nested_specs() {
        let bastion = profile("bastion", "bastion.example.com", "jump");
        let mut web = profile("web", "10.0.0.5", "deploy");
        web.jump_host = Some(bastion.id);

        let profiles = vec![bastion.clone(), web.clone()];
        let store = InMemoryCredentialStore::new();
        let spec = build_native_ssh_spec(&web, &profiles, &store, true);

        let jump = spec.jump.expect("jump host should resolve");
        assert_eq!(jump.host, "bastion.example.com");
        assert_eq!(jump.user, "jump");
        assert!(jump.jump.is_none());
    }

    #[test]
    fn jump_cycle_is_broken_not_infinite() {
        let mut a = profile("a", "a.example.com", "u");
        let mut b = profile("b", "b.example.com", "u");
        a.jump_host = Some(b.id);
        b.jump_host = Some(a.id);
        let profiles = vec![a.clone(), b.clone()];
        let store = InMemoryCredentialStore::new();

        let spec = build_native_ssh_spec(&a, &profiles, &store, true);
        let jump = spec.jump.expect("first hop resolves");
        assert_eq!(jump.host, "b.example.com");
        assert!(jump.jump.is_none(), "cycle back to `a` is cut");
    }

    #[test]
    fn global_verify_host_keys_is_the_fallback() {
        let store = InMemoryCredentialStore::new();
        let mut p = profile("web", "h", "u");

        p.verify_host_keys = None;
        assert!(!build_native_ssh_spec(&p, &[], &store, false).verify_host_keys);
        assert!(build_native_ssh_spec(&p, &[], &store, true).verify_host_keys);

        p.verify_host_keys = Some(false);
        assert!(!build_native_ssh_spec(&p, &[], &store, true).verify_host_keys);
    }

    #[test]
    fn transient_profile_maps_and_resolves_alias_jump_chain() {
        let store = InMemoryCredentialStore::new();
        let mut prod = profile("prod", "10.0.0.5", "deploy");
        prod.port = 2222;
        let resolve = |a: &str| -> Option<(SshProfile, Option<String>)> {
            match a {
                "bastion" => Some((profile("bastion", "bastion.example.com", "jump"), None)),
                _ => None,
            }
        };
        let spec = native_spec_from_transient_profile(
            &prod,
            Some("bastion".to_string()),
            &store,
            true,
            &resolve,
        );
        assert_eq!(spec.host, "10.0.0.5");
        assert_eq!(spec.port, 2222);
        let jump = spec.jump.expect("jump resolves from alias");
        assert_eq!(jump.host, "bastion.example.com");
        assert_eq!(jump.user, "jump");
        assert!(jump.jump.is_none());
    }

    #[test]
    fn transient_profile_jump_falls_back_to_user_host_port() {
        let store = InMemoryCredentialStore::new();
        let prod = profile("prod", "10.0.0.5", "deploy");
        let resolve = |_: &str| None;
        let spec = native_spec_from_transient_profile(
            &prod,
            Some("me@jump.example.com:2200".to_string()),
            &store,
            true,
            &resolve,
        );
        let jump = spec.jump.expect("jump parses as target");
        assert_eq!(jump.host, "jump.example.com");
        assert_eq!(jump.user, "me");
        assert_eq!(jump.port, 2200);
    }

    #[test]
    fn transient_profile_jump_cycle_is_broken() {
        let store = InMemoryCredentialStore::new();
        let prod = profile("prod", "10.0.0.5", "deploy");
        let resolve = |a: &str| -> Option<(SshProfile, Option<String>)> {
            match a {
                "bastion" => Some((
                    profile("bastion", "bastion.example.com", "jump"),
                    Some("prod".to_string()),
                )),
                _ => None,
            }
        };
        let spec = native_spec_from_transient_profile(
            &prod,
            Some("bastion".to_string()),
            &store,
            true,
            &resolve,
        );
        let jump = spec.jump.expect("first hop resolves");
        assert_eq!(jump.host, "bastion.example.com");
        assert!(jump.jump.is_none(), "cycle back to prod is cut");
    }

    #[test]
    fn maps_proxy_precedence_command_over_socks_over_http() {
        let store = InMemoryCredentialStore::new();
        let mut p = profile("web", "h", "u");
        p.socks_proxy = Some(HostPort::new("socks", 1080));
        p.http_proxy = Some(HostPort::new("http", 8080));
        assert!(matches!(
            build_native_ssh_spec(&p, &[], &store, true).proxy,
            SshProxy::Socks { .. }
        ));
        p.proxy_command = Some("nc %h %p".into());
        assert!(matches!(
            build_native_ssh_spec(&p, &[], &store, true).proxy,
            SshProxy::Command(_)
        ));
    }

    #[test]
    fn a_proxy_on_port_zero_is_no_proxy() {
        // What the settings form wrote for `proxy.example.com` before it
        // checked the port, and what is still sitting in configs saved then.
        let store = InMemoryCredentialStore::new();
        let mut p = profile("web", "h", "u");
        p.socks_proxy = Some(HostPort::new("socks", 0));
        assert!(matches!(
            build_native_ssh_spec(&p, &[], &store, true).proxy,
            SshProxy::None
        ));
        p.http_proxy = Some(HostPort::new("http", 8080));
        assert!(matches!(
            build_native_ssh_spec(&p, &[], &store, true).proxy,
            SshProxy::Http { .. }
        ));
    }
}


pub fn parse_ssh_option_words(input: &str) -> Result<Vec<String>, ()> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return Err(());
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

pub struct ParsedSshConnect {
    pub profile: anterm_core::ssh_profile::SshProfile,
    pub proxy_jump: Option<String>,
}

pub fn parse_ssh_connect_input(input: &str) -> Result<ParsedSshConnect, String> {
    use anterm_core::ssh_profile::{SshProfile, parse_quick_connect};

    let mut words = parse_ssh_option_words(input)
        .map_err(|_| t(L10nKey::AppSshParseUnbalancedQuotes).to_string())?;
    if words.first().is_some_and(|word| word == "ssh") {
        words.remove(0);
    }

    let mut target: Option<String> = None;
    let mut user: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut identities: Vec<String> = Vec::new();
    let mut jump: Option<String> = None;

    let mut i = 0;
    while i < words.len() {
        let word = words[i].clone();
        if word == "--" {
            return Err(t(L10nKey::AppSshParseNoRemoteCommands).to_string());
        }
        if let Some((flag, attached)) = ssh_short_flag(&word) {
            let value = if ssh_option_takes_value(flag) {
                if !attached.is_empty() {
                    attached
                } else {
                    i += 1;
                    match words.get(i) {
                        Some(v) => v.clone(),
                        None => {
                            return Err(t_fmt(
                                L10nKey::AppSshParseFlagNeedsValue,
                                &[("flag", &flag.to_string())],
                            ));
                        }
                    }
                }
            } else {
                String::new()
            };
            match flag {
                'p' => {
                    port = Some(
                        value
                            .parse::<u16>()
                            .ok()
                            .filter(|&p| p != 0)
                            .ok_or_else(|| {
                                t_fmt(L10nKey::AppSshParseInvalidPort, &[("value", &value)])
                            })?,
                    )
                }
                'l' => user = Some(value),
                'i' => identities.push(value),
                'J' => jump = Some(value),
                'o' => apply_ssh_o_option(&value, &mut user, &mut port, &mut jump)?,
                _ => {}
            }
        } else if word.starts_with('-') {
            return Err(t_fmt(
                L10nKey::AppSshParseUnsupportedOption,
                &[("option", &word)],
            ));
        } else if target.is_none() {
            target = Some(word);
        } else {
            return Err("Remote commands aren't supported here".to_string());
        }
        i += 1;
    }

    let target = target.ok_or_else(|| t(L10nKey::AppSshParseEnterHost).to_string())?;
    let qc = parse_quick_connect(&target)
        .ok_or_else(|| t_fmt(L10nKey::AppSshParseBadHost, &[("host", &target)]))?;

    let mut profile = SshProfile::new(qc.host.clone());
    profile.host = qc.host;
    profile.port = port.or(qc.port).unwrap_or(22);
    if let Some(user) = user.or(qc.user) {
        profile.user = user;
    }
    profile.identity_files = identities;

    Ok(ParsedSshConnect {
        profile,
        proxy_jump: jump,
    })
}

fn ssh_short_flag(word: &str) -> Option<(char, String)> {
    let rest = word.strip_prefix('-')?;
    if rest.is_empty() || rest.starts_with('-') {
        return None;
    }
    let mut chars = rest.chars();
    let flag = chars.next()?;
    Some((flag, chars.as_str().to_string()))
}

fn apply_ssh_o_option(
    value: &str,
    user: &mut Option<String>,
    port: &mut Option<u16>,
    jump: &mut Option<String>,
) -> Result<(), String> {
    let Some((name, val)) = value.split_once('=') else {
        return Ok(());
    };
    match name.to_ascii_lowercase().as_str() {
        "user" => *user = Some(val.to_string()),
        "port" => {
            *port = Some(
                val.parse::<u16>()
                    .ok()
                    .filter(|&p| p != 0)
                    .ok_or_else(|| t_fmt(L10nKey::AppSshParseInvalidPort, &[("value", val)]))?,
            )
        }
        "proxyjump" => *jump = Some(val.to_string()),
        _ => {}
    }
    Ok(())
}
