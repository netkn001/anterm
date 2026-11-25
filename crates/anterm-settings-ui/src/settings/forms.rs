use gpui::{App, Context, Entity, Subscription, Window, prelude::*};
use gpui_kit::component::input::{InputState, TextareaState};
use uuid::Uuid;

use anterm_ui_kit::keychain::{
    CredentialRef, CredentialStore as _, OsCredentialStore, key_account_from_contents,
};
use anterm_core::ssh_profile::{
    Algorithms, AuthMode, ForwardKind, ForwardRule, HostPort, SshProfile,
};
use super::SettingsWindow;
use anterm_i18n::{L10nKey, t, t_fmt};

pub struct SshProfileForm {
    pub(super) editing: Uuid,
    pub(super) carry_group: Option<String>,
    pub(super) carry_credential_ref: Option<CredentialRef>,

    pub(super) name: Entity<InputState>,
    pub(super) host: Entity<InputState>,
    pub(super) port: Entity<InputState>,
    pub(super) user: Entity<InputState>,
    pub(super) auth: AuthMode,

    /// The secret half of a connection. Neither of these is part of the
    /// profile — they live in the system keychain, and the config file holds
    /// no copy — so the form carries what the keychain had when it opened and
    /// compares against it on the way out. A form that was only read writes
    /// nothing back, and one that cleared a field says so.
    pub(super) password: Entity<InputState>,
    pub(super) passphrase: Entity<InputState>,
    pub(super) loaded_password: String,
    pub(super) loaded_passphrase: String,
    /// The endpoint the password above was read for, and the key file the
    /// passphrase belongs to. An edit to the address moves the entry, and
    /// without these there is nothing left pointing at the one to remove.
    pub(super) loaded_endpoint: (String, String, u16),
    pub(super) loaded_key: Option<String>,

    pub(super) jump: Entity<InputState>,

    pub(super) forwards: Vec<ForwardRuleForm>,

    pub(super) identity_files: Entity<InputState>,
    pub(super) proxy_command: Entity<InputState>,
    pub(super) socks: Entity<InputState>,
    pub(super) http: Entity<InputState>,
    pub(super) kex: Entity<InputState>,
    pub(super) cipher: Entity<InputState>,
    pub(super) mac: Entity<InputState>,
    pub(super) hostkey: Entity<InputState>,
    pub(super) compression: Entity<InputState>,
    pub(super) keepalive_interval: Entity<InputState>,
    pub(super) keepalive_count: Entity<InputState>,
    pub(super) connect_timeout: Entity<InputState>,
    pub(super) login_scripts: Entity<TextareaState>,

    pub(super) agent_forward: bool,
    pub(super) x11: bool,
    pub(super) skip_banner: bool,
    pub(super) shell_integration: bool,
    pub(super) remote_clipboard_write: bool,
    pub(super) verify_host_keys: Option<bool>,
    pub(super) warn_on_close: Option<bool>,

    pub(super) _subs: Vec<Subscription>,
}

impl SshProfileForm {
    /// Whether the group that identifies the host — name, host, port, user —
    /// is still untouched. Every field notifies on change, so the form
    /// re-renders on each keystroke; without this a new host would be told it
    /// needs a host before anyone had the chance to type one. Same deal the
    /// forward rows strike with `ForwardRuleForm::is_blank`.
    pub(super) fn core_is_blank(&self, cx: &App) -> bool {
        // The port is not in the list: it opens on 22 and is never empty, so
        // counting it meant a brand-new host was never "untouched" and the
        // form opened with "Needs a host" already in red under an empty box
        // nobody had reached yet.
        [&self.name, &self.host, &self.user]
            .iter()
            .all(|e| e.read(cx).value().trim().is_empty())
    }

    /// Whether either secret differs from what the keychain handed over.
    ///
    /// Nothing about a password reaches the profile, so the dirty check that
    /// compares profiles cannot see one being typed — without this, Save stays
    /// greyed out over a password the user just entered.
    ///
    /// Untrimmed on purpose: a trailing space is a character of the secret,
    /// and the server is the one that decides whether it belongs.
    pub(super) fn secrets_changed(&self, cx: &App) -> bool {
        self.password.read(cx).value().as_ref() != self.loaded_password
            || self.passphrase.read(cx).value().as_ref() != self.loaded_passphrase
    }

    /// Which credential boxes the method actually uses — the same split
    /// `ssh_connect::build_spec_inner` makes when it decides what to hand the
    /// daemon. A password box under "Agent" would be a secret that is stored
    /// and then never offered.
    pub(super) fn wants_password(&self) -> bool {
        matches!(self.auth, AuthMode::Auto | AuthMode::Password)
    }

    /// A passphrase is filed against a key's contents, so the box only shows
    /// once the form names a key that is actually on this machine.
    pub(super) fn wants_passphrase(&self) -> bool {
        matches!(self.auth, AuthMode::Auto | AuthMode::PublicKey) && self.loaded_key.is_some()
    }
}

/// What saving does to the keychain for the password field: which entry to
/// drop, and whether to write one.
///
/// A plain function because these are the cases a keychain makes expensive to
/// reach by hand — an address edited out from under a saved password, a field
/// cleared to mean "stop remembering this", a form opened and closed without a
/// keystroke.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct PasswordPlan {
    pub(super) drop_old: bool,
    pub(super) store: bool,
}

pub(super) fn password_plan(was: &str, typed: &str, moved: bool) -> PasswordPlan {
    PasswordPlan {
        // Only what this form read is ours to drop, and only once it is no
        // longer the entry this form would write to.
        drop_old: !was.is_empty() && (moved || typed.is_empty()),
        // A move rewrites even an unchanged secret: the account it is filed
        // under is the address, and the address is what changed.
        store: !typed.is_empty() && (typed != was || moved),
    }
}

pub struct ForwardRuleForm {
    pub kind: ForwardKind,
    pub bind_host: Entity<InputState>,
    pub bind_port: Entity<InputState>,
    pub target_host: Entity<InputState>,
    pub target_port: Entity<InputState>,
    pub description: Entity<InputState>,
    pub enabled: bool,
}

impl ForwardRuleForm {
    pub(super) fn collect(&self, cx: &App) -> Option<ForwardRule> {
        let val = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let bind_port: u16 = val(&self.bind_port).parse().ok().filter(|p| *p > 0)?;
        let bind = HostPort::new(val(&self.bind_host), bind_port);
        let target = if self.kind == ForwardKind::Dynamic {
            HostPort::default()
        } else {
            let port: u16 = val(&self.target_port).parse().ok().filter(|p| *p > 0)?;
            let host = val(&self.target_host);
            if host.is_empty() {
                return None;
            }
            HostPort::new(host, port)
        };
        Some(ForwardRule {
            kind: self.kind,
            bind,
            target,
            description: val(&self.description),
            enabled: self.enabled,
        })
    }
}

/// What a blank port field means. The same number `SshProfile`'s serde default
/// writes for a config that never mentioned a port, which is why leaving the
/// field empty has to stay legal: every host imported from `~/.ssh/config`
/// leaves it empty.
pub(super) const DEFAULT_SSH_PORT: u16 = 22;

/// The port each proxy scheme listens on when the field names only a host.
pub(super) const DEFAULT_SOCKS_PORT: u16 = 1080;
const DEFAULT_HTTP_PROXY_PORT: u16 = 8080;

/// A port as a form field spells it. Nothing here accepts 0: every port in a
/// profile is one something has to connect to, and no listener answers on 0.
fn parse_port(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

/// A proxy address as the form spells it: blank is "no proxy", a bare host
/// takes the scheme's default port, and anything else has to carry a port that
/// exists. This used to be `parse().unwrap_or(0)`, so `proxy.example.com:88O`
/// saved a proxy on port 0 and the failure surfaced far away, in the socket
/// layer. The default port is not a secret either — `host_port_text` writes it
/// back into the field the next time the form opens.
pub(super) fn parse_host_port_checked(s: &str, default_port: u16) -> Result<Option<HostPort>, SshFieldError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.rsplit_once(':') {
        Some((h, p)) => match parse_port(p) {
            Some(port) => Ok(Some(HostPort::new(h.trim(), port))),
            None => Err(SshFieldError::ProxyPortRange),
        },
        None => Ok(Some(HostPort::new(s, default_port))),
    }
}

pub(super) fn host_port_text(hp: &Option<HostPort>) -> String {
    hp.as_ref()
        .map(|h| format!("{}:{}", h.host, h.port))
        .unwrap_or_default()
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn split_lines(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Why one field of the SSH profile form cannot be saved. A value rather than
/// a finished sentence, so the rules stay a plain function a test can call —
/// the wording, and the locale it is written in, belong to the render pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshFieldError {
    /// Nothing to connect to. Saved anyway, the profile used to render as an
    /// empty row in the host list and hand `TcpStream::connect` an empty name.
    HostMissing,
    /// A port field that is neither blank nor a port.
    PortRange,
    /// The same, for the port half of a proxy address.
    ProxyPortRange,
    /// The jump field names a profile no host list has.
    JumpUnknown(String),
    /// The jump field names the profile being edited.
    JumpIsSelf,
}

impl SshFieldError {
    pub(super) fn message(&self) -> String {
        match self {
            Self::HostMissing => t(L10nKey::SettingsHostRequired).to_string(),
            Self::PortRange => t(L10nKey::SettingsPortInvalid).to_string(),
            Self::ProxyPortRange => t(L10nKey::SettingsProxyPortInvalid).to_string(),
            Self::JumpUnknown(name) => {
                t_fmt(L10nKey::SettingsJumpHostUnknown, &[("jump_name", name)])
            }
            Self::JumpIsSelf => t(L10nKey::SettingsJumpHostSelf).to_string(),
        }
    }
}

/// What the form has to fix before it can be saved, one slot per field so each
/// complaint can be printed under the control it is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshFormErrors {
    pub(super) host: Option<SshFieldError>,
    pub(super) port: Option<SshFieldError>,
    pub(super) jump: Option<SshFieldError>,
    pub(super) socks: Option<SshFieldError>,
    pub(super) http: Option<SshFieldError>,
}

impl SshFormErrors {
    pub(super) fn is_empty(&self) -> bool {
        self.host.is_none()
            && self.port.is_none()
            && self.jump.is_none()
            && self.socks.is_none()
            && self.http.is_none()
    }
}

/// The SSH profile form as plain text, lifted out of the `InputState` entities
/// it lives in. The rules that turn it into a profile are the part worth
/// testing, and a GPUI entity is not something a unit test can hand them, so
/// the window layer's job stops at reading the strings out.
#[derive(Debug, Clone, Default)]
pub struct SshFormDraft {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) group: Option<String>,
    pub(super) host: String,
    pub(super) port: String,
    pub(super) user: String,
    pub(super) jump: String,
    pub(super) proxy_command: String,
    pub(super) socks: String,
    pub(super) http: String,
    pub(super) auth: AuthMode,
    pub(super) identity_files: String,
    pub(super) agent_forward: bool,
    pub(super) credential_ref: Option<CredentialRef>,
    pub(super) forwards: Vec<ForwardRule>,
    pub(super) keepalive_interval: String,
    pub(super) keepalive_count: String,
    pub(super) connect_timeout: String,
    pub(super) warn_on_close: Option<bool>,
    pub(super) skip_banner: bool,
    pub(super) shell_integration: bool,
    pub(super) remote_clipboard_write: bool,
    pub(super) login_scripts: String,
    pub(super) x11: bool,
    pub(super) kex: String,
    pub(super) cipher: String,
    pub(super) mac: String,
    pub(super) hostkey: String,
    pub(super) compression: String,
    pub(super) verify_host_keys: Option<bool>,
}

/// The one place that decides what the form would save and what is wrong with
/// it. Both, always — never one or the other: the Escape prompt asks whether
/// the form differs from what is on disk, and a form that is merely invalid
/// still holds everything the user typed. Handing back only the errors would
/// make a brand-new invalid profile compare equal to the nothing on disk, and
/// Escape would throw the typing away without asking.
///
/// A missing `name` is deliberately not an error: the host list already falls
/// back to the host for a nameless profile, and requiring one would refuse
/// every host imported from `~/.ssh/config`.
pub(super) fn validate_ssh_draft(draft: SshFormDraft, profiles: &[SshProfile]) -> (SshProfile, SshFormErrors) {
    let mut errors = SshFormErrors::default();

    let host = draft.host.trim().to_string();
    if host.is_empty() {
        errors.host = Some(SshFieldError::HostMissing);
    }

    let port_text = draft.port.trim();
    let port = match port_text.is_empty() {
        true => DEFAULT_SSH_PORT,
        false => parse_port(port_text).unwrap_or_else(|| {
            errors.port = Some(SshFieldError::PortRange);
            DEFAULT_SSH_PORT
        }),
    };

    // The field is a name but the profile stores an id, so a jump host already
    // survives its target being renamed. What it never survived was a name
    // nobody has: the lookup returned `None`, the profile saved as a direct
    // connection, and reopening the form showed an empty field.
    let jump_name = draft.jump.trim();
    let jump_host = if jump_name.is_empty() {
        None
    } else {
        let named = |p: &&SshProfile| p.name == jump_name;
        // Duplicate names resolve to whichever profile comes first, as they
        // always have. The one profile that can never be the answer is the one
        // being edited, and typing its own name is worth saying out loud
        // rather than quietly connecting direct.
        match profiles.iter().filter(named).find(|p| p.id != draft.id) {
            Some(p) => Some(p.id),
            None => {
                errors.jump = Some(match profiles.iter().any(|p| p.name == jump_name) {
                    true => SshFieldError::JumpIsSelf,
                    false => SshFieldError::JumpUnknown(jump_name.to_string()),
                });
                None
            }
        }
    };

    let proxy = |text: &str, default_port: u16, slot: &mut Option<SshFieldError>| {
        match parse_host_port_checked(text, default_port) {
            Ok(hp) => hp,
            Err(e) => {
                *slot = Some(e);
                None
            }
        }
    };
    let socks_proxy = proxy(&draft.socks, DEFAULT_SOCKS_PORT, &mut errors.socks);
    let http_proxy = proxy(&draft.http, DEFAULT_HTTP_PROXY_PORT, &mut errors.http);

    let proxy_command = draft.proxy_command.trim();
    let profile = SshProfile {
        id: draft.id,
        name: draft.name.trim().to_string(),
        group: draft.group,
        host,
        port,
        user: draft.user.trim().to_string(),
        jump_host,
        proxy_command: (!proxy_command.is_empty()).then(|| proxy_command.to_string()),
        socks_proxy,
        http_proxy,
        auth: draft.auth,
        identity_files: split_lines(&draft.identity_files),
        agent_forward: draft.agent_forward,
        credential_ref: draft.credential_ref,
        forwards: draft.forwards,
        keepalive_interval_s: draft.keepalive_interval.trim().parse().ok(),
        keepalive_count_max: draft.keepalive_count.trim().parse().ok(),
        connect_timeout_s: draft.connect_timeout.trim().parse().ok(),
        warn_on_close: draft.warn_on_close,
        skip_banner: draft.skip_banner,
        shell_integration: draft.shell_integration,
        remote_clipboard_write: draft.remote_clipboard_write,
        login_scripts: split_lines(&draft.login_scripts),
        x11: draft.x11,
        algorithms: Algorithms {
            kex: split_list(&draft.kex),
            cipher: split_list(&draft.cipher),
            mac: split_list(&draft.mac),
            hostkey: split_list(&draft.hostkey),
            compression: split_list(&draft.compression),
        },
        verify_host_keys: draft.verify_host_keys,
    };
    (profile, errors)
}

pub(super) fn forward_row_inputs(row: &ForwardRuleForm) -> [&Entity<InputState>; 5] {
    [
        &row.bind_host,
        &row.bind_port,
        &row.target_host,
        &row.target_port,
        &row.description,
    ]
}

pub(super) fn seed_forward_row(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    rule: &ForwardRule,
) -> ForwardRuleForm {
    let port = |p: u16| if p == 0 { String::new() } else { p.to_string() };
    ForwardRuleForm {
        kind: rule.kind,
        bind_host: seed_hinted(window, cx, &rule.bind.host, "localhost"),
        bind_port: seed_hinted(window, cx, &port(rule.bind.port), "8080"),
        target_host: seed_hinted(window, cx, &rule.target.host, "127.0.0.1"),
        target_port: seed_hinted(window, cx, &port(rule.target.port), "80"),
        description: seed_hinted(
            window,
            cx,
            &rule.description,
            t(L10nKey::ForwardDescriptionPlaceholder),
        ),
        enabled: rule.enabled,
    }
}

pub(super) fn seed_hinted(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    value: &str,
    placeholder: &'static str,
) -> Entity<InputState> {
    let value = value.to_string();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

pub(super) fn seed_input(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    value: &str,
    _multi_line: bool,
) -> Entity<InputState> {
    let value = value.to_string();
    cx.new(|cx| InputState::new(window, cx).default_value(value))
}

pub(super) fn seed_textarea(
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
    value: &str,
) -> Entity<TextareaState> {
    let value = value.to_string();
    cx.new(|cx| TextareaState::new(window, cx).default_value(value))
}

/// A picked path written the way a config file spells it. `~/.ssh/id_ed25519`
/// keeps meaning the right file on another machine, or after the account is
/// renamed; the absolute path the system picker hands back does not.
pub(super) fn tildify(path: &str) -> String {
    #[cfg(windows)]
    let home = std::env::var("USERPROFILE").ok();
    #[cfg(not(windows))]
    let home = std::env::var("HOME").ok();
    tildify_with(path, home.as_deref().filter(|h| !h.is_empty()))
}

pub(super) fn tildify_with(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    let home = home.trim_end_matches(['/', '\\']);
    match path.strip_prefix(home) {
        // A separator has to follow, or `/Users/adalovelace` would come back
        // as a file inside `/Users/ada`.
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => format!(
            "~/{}",
            rest.trim_start_matches(['/', '\\']).replace('\\', "/")
        ),
        _ => path.to_string(),
    }
}

/// What the key field shows while it is empty: the file ssh would reach for on
/// its own. A hint, not a value — an empty field still means "try the usual
/// `~/.ssh` keys", which is exactly what `default_identity_candidates` does.
pub(super) const DEFAULT_KEY_HINT: &str = "~/.ssh/id_ed25519";

/// The password the keychain holds for this profile's endpoint, or nothing.
///
/// Read once, when a host is opened for editing — not per render, and not per
/// keystroke. A profile with no host yet has no endpoint to ask about: the
/// account would come out as `@:22`, which belongs to no server.
pub(super) fn stored_password(profile: &SshProfile) -> String {
    if profile.host.trim().is_empty() {
        return String::new();
    }
    OsCredentialStore
        .password_for(&profile.user, &profile.host, profile.port)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// The first key file a profile would offer that is actually there.
///
/// A passphrase is accounted by the key's *contents*, not by its path, so a
/// file that cannot be read is a key nothing can be stored against.
pub(super) fn first_readable_key(profile: &SshProfile) -> Option<String> {
    first_readable_key_in(&profile.identity_files, &profile.host, &profile.user)
}

/// The same answer for a form that has not been collected into a profile yet:
/// the key field as typed, with the host and user beside it filling in `%h`
/// and `%r`.
pub(super) fn first_readable_key_in(files: &[String], host: &str, user: &str) -> Option<String> {
    first_readable_key_or(
        files,
        host,
        user,
        anterm_core::ssh_profile::default_identity_candidates,
    )
}

/// An empty key field is not "no key": `build_spec_inner` offers the `~/.ssh`
/// defaults then, and looks their passphrases up by those exact strings. The
/// box has to follow the same list, or the most common setup — no key named,
/// an encrypted `id_ed25519` — could never be given a passphrase here.
pub(super) fn first_readable_key_or(
    files: &[String],
    host: &str,
    user: &str,
    defaults: impl FnOnce() -> Vec<String>,
) -> Option<String> {
    let candidates = if files.is_empty() {
        defaults()
    } else {
        files
            .iter()
            .map(|f| anterm_core::ssh_profile::expand_identity_placeholders(f, host, user))
            .collect()
    };
    candidates
        .into_iter()
        .find(|p| std::fs::metadata(p).is_ok())
}

pub(super) fn stored_passphrase(key_path: &str) -> String {
    let path = anterm_core::ssh_profile::expand_tilde(key_path);
    let Ok(bytes) = std::fs::read(&path) else {
        return String::new();
    };
    OsCredentialStore
        .passphrase_for_key(&key_account_from_contents(&bytes))
        .ok()
        .flatten()
        .unwrap_or_default()
}
