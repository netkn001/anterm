use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, IntoElement, ParentElement as _,
    Render, Styled as _, Subscription, Window, div, prelude::*, px, rems,
};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, FocusTrapElement as _, Sizable as _, h_flex, v_flex};

use anterm_ui_kit::keychain::{CredentialStore as _, OsCredentialStore};
use anterm_core::daemon::protocol::{AuthPromptKind, AuthResponse, SshPhase};

use anterm_ui_kit::dialog::{self, Tone};
use anterm_ui_kit::panel_tokens::{META, META_MONO, TEXT};
use anterm_i18n::{L10nKey, t, t_fmt};

/// The sheet's width, and the width of the banners stacked above it, so the
/// column reads as one.
const SHEET_W: f32 = 440.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiRow {
    pub text: String,
    pub echo: bool,
}

/// The connection a prompt belongs to, which is also the key the keychain
/// files its password under. A password prompt names its own user and host,
/// but nothing on the wire carries the port and a keyboard-interactive prompt
/// names none of the three — so whoever raises the sheet has to supply what it
/// knows about the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEndpoint {
    pub user: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptModel {
    Password {
        user: String,
        host: String,
        port: u16,
        rejected: bool,
    },
    KeyPassphrase {
        key_path: String,
        comment: String,
        rejected: bool,
    },
    KeyboardInteractive {
        name: String,
        instructions: String,
        prompts: Vec<KiRow>,
        /// `None` when nothing told the sheet which connection is asking — a
        /// route the GUI could not resolve to an SSH hop. Without it there is
        /// no keychain entry to name, so a rejected stored password can only
        /// be re-typed, not forgotten.
        endpoint: Option<PromptEndpoint>,
        stored_rejected: bool,
    },
    HostKeyUnknown {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        /// Set when the host is already on file under some other algorithm, so
        /// the sheet can say why a known host is offering an unseen key.
        previously_known_as: Option<String>,
    },
    HostKeyChanged {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        old_fingerprint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainWrite {
    None,
    SetPassword {
        user: String,
        host: String,
        port: u16,
        secret: String,
    },
    DeletePassword {
        user: String,
        host: String,
        port: u16,
    },
    SetKeyPassphrase {
        key_path: String,
        secret: String,
    },
    DeleteKeyPassphrase {
        key_path: String,
    },
}

impl PromptModel {
    pub fn from_prompt(
        kind: AuthPromptKind,
        endpoint: Option<PromptEndpoint>,
        auto_supplied_password: bool,
    ) -> Option<PromptModel> {
        let port = endpoint.as_ref().map(|e| e.port).unwrap_or(22);
        Some(match kind {
            AuthPromptKind::Password { user, host } => PromptModel::Password {
                user,
                host,
                port,
                rejected: auto_supplied_password,
            },
            AuthPromptKind::KeyPassphrase {
                key_path,
                comment,
                rejected,
            } => PromptModel::KeyPassphrase {
                key_path,
                comment,
                rejected,
            },
            AuthPromptKind::KeyboardInteractive {
                name,
                instructions,
                prompts,
                stored_rejected,
            } => PromptModel::KeyboardInteractive {
                name,
                instructions,
                prompts: prompts
                    .into_iter()
                    .map(|p| KiRow {
                        text: p.text,
                        echo: p.echo,
                    })
                    .collect(),
                endpoint,
                stored_rejected,
            },
            AuthPromptKind::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint_sha256,
                previously_known_as,
            } => PromptModel::HostKeyUnknown {
                host,
                port,
                algorithm,
                fingerprint: fingerprint_sha256,
                previously_known_as,
            },
            AuthPromptKind::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint_sha256,
                old_fingerprint_sha256,
            } => PromptModel::HostKeyChanged {
                host,
                port,
                algorithm,
                fingerprint: fingerprint_sha256,
                old_fingerprint: old_fingerprint_sha256,
            },
            AuthPromptKind::Banner { .. } => return None,
        })
    }

    fn input_count(&self) -> usize {
        match self {
            PromptModel::Password { .. } | PromptModel::KeyPassphrase { .. } => 1,
            PromptModel::KeyboardInteractive { prompts, .. } => prompts.len(),
            PromptModel::HostKeyUnknown { .. } => 0,
            PromptModel::HostKeyChanged { .. } => 1,
        }
    }
}

pub fn password_submit(
    user: &str,
    host: &str,
    port: u16,
    secret: String,
    remember: bool,
    rejected: bool,
) -> (AuthResponse, KeychainWrite) {
    let write = if remember {
        KeychainWrite::SetPassword {
            user: user.to_string(),
            host: host.to_string(),
            port,
            secret: secret.clone(),
        }
    } else if rejected {
        KeychainWrite::DeletePassword {
            user: user.to_string(),
            host: host.to_string(),
            port,
        }
    } else {
        KeychainWrite::None
    };
    (AuthResponse::Secret(secret), write)
}

pub fn passphrase_submit(
    key_path: &str,
    secret: String,
    remember: bool,
    rejected: bool,
) -> (AuthResponse, KeychainWrite) {
    let write = if remember {
        KeychainWrite::SetKeyPassphrase {
            key_path: key_path.to_string(),
            secret: secret.clone(),
        }
    } else if rejected {
        // The stored passphrase is the reason this sheet is up, and the user
        // has just declined to save the replacement. Leaving the old one
        // behind would hand the same dead secret to the next connection.
        KeychainWrite::DeleteKeyPassphrase {
            key_path: key_path.to_string(),
        }
    } else {
        KeychainWrite::None
    };
    (AuthResponse::Secret(secret), write)
}

/// Keyboard-interactive answers are never saved — one of them is as likely to
/// be a one-time code as a password — so this side only ever *forgets*: the
/// stored password the daemon says the server just turned down.
pub fn ki_submit(
    endpoint: Option<&PromptEndpoint>,
    answers: Vec<String>,
    stored_rejected: bool,
) -> (AuthResponse, KeychainWrite) {
    let write = match endpoint.filter(|_| stored_rejected) {
        Some(e) => KeychainWrite::DeletePassword {
            user: e.user.clone(),
            host: e.host.clone(),
            port: e.port,
        },
        None => KeychainWrite::None,
    };
    (AuthResponse::Secrets(answers), write)
}

pub fn host_key_unknown_decision(trust: bool) -> AuthResponse {
    AuthResponse::HostKeyDecision {
        accept: trust,
        remember: trust,
    }
}

pub fn changed_confirmed(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case("yes")
}

pub fn host_key_changed_decision(typed: &str) -> AuthResponse {
    if changed_confirmed(typed) {
        AuthResponse::HostKeyDecision {
            accept: true,
            remember: true,
        }
    } else {
        AuthResponse::HostKeyDecision {
            accept: false,
            remember: false,
        }
    }
}

/// Value snapshot drained from an SSH terminal pane and handed to [`SshPromptModal`].
#[derive(Clone, Debug)]
pub struct SshPromptBatch {
    pub pane_id: u64,
    pub endpoint: Option<PromptEndpoint>,
    pub auto_supplied: bool,
    pub phase: Option<SshPhase>,
    pub banners: Vec<String>,
    pub next: Option<(u64, AuthPromptKind)>,
}

/// Semantic events emitted by [`SshPromptModal`] back to its workspace host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshPromptEvent {
    /// Deliver an authentication response back to the originating pane.
    Answered {
        pane_id: u64,
        request_id: u64,
        response: AuthResponse,
    },
    /// The active prompt closed. The workspace should feed another waiting
    /// prompt (from the same pane first, then any other waiting pane) or
    /// restore keyboard focus to the active terminal.
    Dismissed {
        previous_pane_id: Option<u64>,
    },
}

/// Detached modal view owning SSH authentication sheets and connection banners.
pub struct SshPromptModal {
    pane_id: Option<u64>,
    request_id: u64,
    model: Option<PromptModel>,
    banners: Vec<String>,
    inputs: Vec<Entity<InputState>>,
    remember: bool,
    phase: Option<SshPhase>,
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SshPromptEvent> for SshPromptModal {}

impl SshPromptModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            pane_id: None,
            request_id: 0,
            model: None,
            banners: Vec::new(),
            inputs: Vec::new(),
            remember: false,
            phase: None,
            focus_handle: cx.focus_handle(),
            _subs: Vec::new(),
        }
    }

    pub fn has_active_prompt(&self) -> bool {
        self.model.is_some()
    }

    pub fn is_visible_for(&self, focused_pane_id: Option<u64>) -> bool {
        if self.model.is_none() && self.banners.is_empty() {
            return false;
        }
        self.pane_id.is_none() || focused_pane_id == self.pane_id
    }

    fn clear(&mut self) {
        self.pane_id = None;
        self.request_id = 0;
        self.model = None;
        self.inputs.clear();
        self.remember = false;
        self._subs.clear();
    }

    pub fn push_prompt_batch(
        &mut self,
        batch: SshPromptBatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SshPromptBatch {
            pane_id,
            endpoint,
            auto_supplied,
            phase,
            banners,
            next,
        } = batch;

        self.banners.extend(banners);
        if phase.is_some() {
            self.phase = phase;
        }

        if let Some((request_id, kind)) = next {
            if let Some(model) = PromptModel::from_prompt(kind, endpoint, auto_supplied) {
                let inputs = build_inputs(&model, window, cx);
                let mut subs = Vec::new();
                for input in &inputs {
                    subs.push(cx.subscribe_in(
                        input,
                        window,
                        |this, _input, ev: &InputEvent, window, cx| match ev {
                            InputEvent::PressEnter { .. } => this.submit(window, cx),
                            InputEvent::Change => cx.notify(),
                            _ => {}
                        },
                    ));
                }
                if let Some(first) = inputs.first() {
                    first.update(cx, |s, cx| s.focus(window, cx));
                }
                self.pane_id = Some(pane_id);
                self.request_id = request_id;
                self.model = Some(model);
                self.inputs = inputs;
                self.remember = false;
                self._subs = subs;
            }
        }
        cx.notify();
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.clone() else {
            return;
        };
        let remember = self.remember;
        let values: Vec<String> = self
            .inputs
            .iter()
            .map(|i| i.read(cx).value().to_string())
            .collect();

        if let PromptModel::HostKeyChanged { .. } = &model {
            if !changed_confirmed(values.first().map(String::as_str).unwrap_or_default()) {
                return;
            }
        }

        let (response, write) = match &model {
            PromptModel::Password {
                user,
                host,
                port,
                rejected,
            } => {
                let secret = values.first().cloned().unwrap_or_default();
                password_submit(user, host, *port, secret, remember, *rejected)
            }
            PromptModel::KeyPassphrase {
                key_path, rejected, ..
            } => {
                let secret = values.first().cloned().unwrap_or_default();
                passphrase_submit(key_path, secret, remember, *rejected)
            }
            PromptModel::KeyboardInteractive {
                endpoint,
                stored_rejected,
                ..
            } => ki_submit(endpoint.as_ref(), values, *stored_rejected),
            PromptModel::HostKeyUnknown { .. } => {
                (host_key_unknown_decision(true), KeychainWrite::None)
            }
            PromptModel::HostKeyChanged { .. } => {
                let typed = values.first().cloned().unwrap_or_default();
                (host_key_changed_decision(&typed), KeychainWrite::None)
            }
        };

        self.apply_keychain_write(write);
        self.respond_active(response, cx);
        self.dismiss_and_advance(window, cx);
    }

    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model) = self.model.clone() else {
            return;
        };
        let response = match model {
            PromptModel::HostKeyUnknown { .. } => host_key_unknown_decision(false),
            PromptModel::HostKeyChanged { .. } => AuthResponse::HostKeyDecision {
                accept: false,
                remember: false,
            },
            _ => AuthResponse::Cancelled,
        };
        self.respond_active(response, cx);
        self.dismiss_and_advance(window, cx);
    }

    fn trust_host_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.respond_active(host_key_unknown_decision(true), cx);
        self.dismiss_and_advance(window, cx);
    }

    fn toggle_remember(&mut self, cx: &mut Context<Self>) {
        self.remember = !self.remember;
        cx.notify();
    }

    pub fn push_connect_error(&mut self, reason: String, cx: &mut Context<Self>) {
        self.banners.push(reason);
        cx.notify();
    }

    pub fn dismiss_banner(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.banners.len() {
            self.banners.remove(ix);
            cx.notify();
        }
    }

    fn respond_active(&mut self, response: AuthResponse, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.pane_id {
            cx.emit(SshPromptEvent::Answered {
                pane_id,
                request_id: self.request_id,
                response,
            });
        }
    }

    fn dismiss_and_advance(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let previous_pane_id = self.pane_id;
        self.clear();
        cx.emit(SshPromptEvent::Dismissed { previous_pane_id });
        cx.notify();
    }

    fn apply_keychain_write(&self, write: KeychainWrite) {
        let store = OsCredentialStore;
        match write {
            KeychainWrite::None => {}
            KeychainWrite::SetPassword {
                user,
                host,
                port,
                secret,
            } => {
                if let Err(e) = store.set_password(&user, &host, port, &secret) {
                    log::warn!("could not save password to keychain: {e}");
                }
            }
            KeychainWrite::DeletePassword { user, host, port } => {
                if let Err(e) = store.delete_password(&user, &host, port) {
                    log::warn!("could not forget password in keychain: {e}");
                }
            }
            KeychainWrite::SetKeyPassphrase { key_path, secret } => {
                let path = anterm_core::ssh_profile::expand_tilde(&key_path);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let account = anterm_ui_kit::keychain::key_account_from_contents(&bytes);
                        if let Err(e) = store.set_key_passphrase(&account, &secret) {
                            log::warn!("could not save key passphrase to keychain: {e}");
                        }
                    }
                    Err(e) => log::warn!("not remembering passphrase; cannot read {path}: {e}"),
                }
            }
            KeychainWrite::DeleteKeyPassphrase { key_path } => {
                let path = anterm_core::ssh_profile::expand_tilde(&key_path);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let account = anterm_ui_kit::keychain::key_account_from_contents(&bytes);
                        if let Err(e) = store.delete_key_passphrase(&account) {
                            log::warn!("could not forget key passphrase in keychain: {e}");
                        }
                    }
                    Err(e) => log::warn!("not forgetting passphrase; cannot read {path}: {e}"),
                }
            }
        }
    }

    fn render_banner(&self, ix: usize, text: &str, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .occlude()
            .w(dialog::rem_px(SHEET_W))
            .items_center()
            .gap(dialog::rem_px(10.))
            .min_h(dialog::rem_px(dialog::HEADER_H))
            .pl(dialog::rem_px(dialog::INSET))
            .pr(dialog::rem_px(10.))
            .py(dialog::rem_px(6.))
            .map(|panel| anterm_ui_kit::theme::floating_surface(panel, cx))
            .rounded_xl()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(TEXT))
                    .child(text.to_string()),
            )
            .child(dialog::button(
                ("ssh-banner-dismiss", ix),
                t(L10nKey::Dismiss),
                Tone::Secondary,
                true,
                dialog::popover_rungs(cx),
                cx,
                cx.listener(move |this, _, _w, cx| this.dismiss_banner(ix, cx)),
            ))
            .into_any_element()
    }

    fn render_sheet(&self, model: &PromptModel, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (danger, muted, fg) = (theme.danger, theme.muted_foreground, theme.foreground);
        let rungs = dialog::popover_rungs(cx);
        let (title, danger_sheet) = match model {
            PromptModel::Password { user, host, .. } => (
                t_fmt(
                    L10nKey::SshPromptPasswordFor,
                    &[("user", user), ("host", host)],
                ),
                false,
            ),
            PromptModel::KeyPassphrase { key_path, .. } => (
                t_fmt(L10nKey::SshPromptPassphraseFor, &[("key_path", key_path)]),
                false,
            ),
            PromptModel::KeyboardInteractive { name, .. } => {
                let label = if name.is_empty() {
                    t(L10nKey::SshPromptTwoFactor).to_string()
                } else {
                    name.clone()
                };
                (label, false)
            }
            PromptModel::HostKeyUnknown { host, .. } => (
                t_fmt(L10nKey::SshPromptUnknownHost, &[("host", host)]),
                false,
            ),
            PromptModel::HostKeyChanged { .. } => {
                (t(L10nKey::SshPromptHostKeyChanged).to_string(), true)
            }
        };

        let line =
            |text: String, ink: gpui::Hsla| div().text_size(rems(META)).text_color(ink).child(text);
        let mono_family = cx.theme().mono_font_family.clone();
        let mono = |text: String, ink: gpui::Hsla| {
            div()
                .text_size(rems(META_MONO))
                .font_family(mono_family.clone())
                .text_color(ink)
                .child(text)
        };

        let card = dialog::card("ssh-prompt-card", title.clone(), SHEET_W, cx)
            .track_focus(&self.focus_handle)
            .key_context("SshPrompt")
            .when(danger_sheet, |panel| panel.border_color(danger))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.cancel(window, cx);
                }
            }))
            .child(dialog::header(
                div()
                    .when(danger_sheet, |d| d.text_color(danger))
                    .child(title),
                cx,
            ));

        let (body, footer) = match model {
            PromptModel::Password { rejected, .. } => {
                let body = dialog::body()
                    .when(*rejected, |c| {
                        c.child(line(t(L10nKey::StoredPasswordRejected).to_string(), danger))
                    })
                    .child(self.render_input(0, cx))
                    .child(self.render_remember(cx));
                (body, self.render_actions(t(L10nKey::SshPromptConnect), cx))
            }
            PromptModel::KeyPassphrase {
                comment, rejected, ..
            } => {
                let body = dialog::body()
                    .when(*rejected, |c| {
                        c.child(line(
                            t(L10nKey::StoredPassphraseRejected).to_string(),
                            danger,
                        ))
                    })
                    .when(!comment.is_empty(), |c| {
                        c.child(line(comment.clone(), muted))
                    })
                    .child(self.render_input(0, cx))
                    .child(self.render_remember(cx));
                (body, self.render_actions(t(L10nKey::SshPromptUnlock), cx))
            }
            PromptModel::KeyboardInteractive {
                instructions,
                prompts,
                stored_rejected,
                ..
            } => {
                let mut body = dialog::body()
                    .when(*stored_rejected, |c| {
                        c.child(line(t(L10nKey::StoredPasswordRejected).to_string(), danger))
                    })
                    .when(!instructions.is_empty(), |c| {
                        c.child(line(instructions.clone(), fg))
                    });
                for (i, row) in prompts.iter().enumerate() {
                    body = body.child(
                        v_flex()
                            .gap(px(6.))
                            .child(dialog::label(row.text.clone(), cx))
                            .child(self.render_input(i, cx)),
                    );
                }
                (body, self.render_actions(t(L10nKey::SshPromptSubmit), cx))
            }
            PromptModel::HostKeyUnknown {
                algorithm,
                fingerprint,
                port,
                host,
                previously_known_as,
            } => {
                let body = dialog::body()
                    .child(
                        dialog::well(cx)
                            .child(mono(format!("{host}:{port}  {algorithm}"), muted))
                            .child(mono(fingerprint.clone(), fg)),
                    )
                    .when_some(previously_known_as.as_ref(), |c, previous| {
                        c.child(line(
                            t_fmt(
                                L10nKey::SshPromptHostKeyNewAlgorithm,
                                &[("previous_algorithm", previous), ("algorithm", algorithm)],
                            ),
                            muted,
                        ))
                    });
                let footer = dialog::footer(cx)
                    .child(dialog::button(
                        "ssh-hk-abort",
                        t(L10nKey::Abort),
                        Tone::Secondary,
                        true,
                        rungs,
                        cx,
                        cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                    ))
                    .child(dialog::button(
                        "ssh-hk-trust",
                        t(L10nKey::Trust),
                        Tone::Primary,
                        true,
                        rungs,
                        cx,
                        cx.listener(|this, _, window, cx| this.trust_host_key(window, cx)),
                    ))
                    .into_any_element();
                (body, footer)
            }
            PromptModel::HostKeyChanged {
                algorithm,
                fingerprint,
                old_fingerprint,
                port,
                host,
            } => {
                let typed = self
                    .inputs
                    .first()
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                let can_override = changed_confirmed(&typed);
                let body = dialog::body()
                    .child(line(
                        t(L10nKey::SshPromptHostKeyChangedBody).to_string(),
                        danger,
                    ))
                    .child(
                        dialog::well(cx)
                            .child(mono(format!("{host}:{port}  {algorithm}"), muted))
                            .child(mono(
                                t_fmt(L10nKey::SshPromptNewKey, &[("fingerprint", &fingerprint)]),
                                fg,
                            ))
                            .child(mono(
                                t_fmt(
                                    L10nKey::SshPromptOldKey,
                                    &[("old_fingerprint", &old_fingerprint)],
                                ),
                                muted,
                            )),
                    )
                    .child(
                        v_flex()
                            .gap(px(6.))
                            .child(line(t(L10nKey::HostKeyOverrideMessage).to_string(), fg))
                            .child(self.render_input(0, cx))
                            .when(!typed.trim().is_empty() && !can_override, |c| {
                                c.child(line(
                                    t(L10nKey::SshPromptTypeYesToOverride).to_string(),
                                    muted,
                                ))
                            }),
                    );
                let footer = dialog::footer(cx)
                    .child(dialog::button(
                        "ssh-hkc-override",
                        t(L10nKey::Override),
                        Tone::Danger,
                        can_override,
                        rungs,
                        cx,
                        cx.listener(|this, _, window, cx| this.submit(window, cx)),
                    ))
                    .child(dialog::button(
                        "ssh-hkc-abort",
                        t(L10nKey::Abort),
                        Tone::Primary,
                        true,
                        rungs,
                        cx,
                        cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                    ))
                    .into_any_element();
                (body, footer)
            }
        };

        card.child(body)
            .child(footer)
            .focus_trap("ssh-prompt-trap", &self.focus_handle)
            .into_any_element()
    }

    fn render_input(&self, ix: usize, cx: &App) -> AnyElement {
        match self.inputs.get(ix) {
            Some(state) => dialog::field(Input::new(state), cx).into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn render_remember(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .child(
                Checkbox::new("ssh-remember")
                    .label(t(L10nKey::RememberKeychain))
                    .small()
                    .checked(self.remember)
                    .on_click(cx.listener(|this, _, _w, cx| this.toggle_remember(cx))),
            )
            .into_any_element()
    }

    fn render_actions(&self, submit_label: &str, cx: &mut Context<Self>) -> AnyElement {
        let rungs = dialog::popover_rungs(cx);
        dialog::footer(cx)
            .child(dialog::button(
                "ssh-cancel",
                t(L10nKey::Cancel),
                Tone::Secondary,
                true,
                rungs,
                cx,
                cx.listener(|this, _, window, cx| this.cancel(window, cx)),
            ))
            .child(dialog::button(
                "ssh-submit",
                submit_label.to_string(),
                Tone::Primary,
                true,
                rungs,
                cx,
                cx.listener(|this, _, window, cx| this.submit(window, cx)),
            ))
            .into_any_element()
    }
}

impl Render for SshPromptModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut stack = v_flex().gap_2().items_center();

        for (ix, banner) in self.banners.iter().enumerate() {
            stack = stack.child(self.render_banner(ix, banner, cx));
        }

        if let Some(model) = &self.model {
            stack = stack.child(self.render_sheet(model, cx));
        }

        div()
            .absolute()
            .inset_0()
            .bg(anterm_ui_kit::presets::scrim_fill(cx))
            .flex()
            .flex_col()
            .items_center()
            .justify_start()
            .pt(px(anterm_ui_kit::dialog::CARD_TOP)) // viewport drop offset
            .child(stack)
    }
}

fn build_inputs(
    model: &PromptModel,
    window: &mut Window,
    cx: &mut Context<SshPromptModal>,
) -> Vec<Entity<InputState>> {
    let count = model.input_count();
    (0..count)
        .map(|i| {
            let masked = match model {
                PromptModel::Password { .. } | PromptModel::KeyPassphrase { .. } => true,
                PromptModel::KeyboardInteractive { prompts, .. } => {
                    prompts.get(i).map(|p| !p.echo).unwrap_or(true)
                }
                PromptModel::HostKeyUnknown { .. } | PromptModel::HostKeyChanged { .. } => false,
            };
            cx.new(|cx| InputState::new(window, cx).masked(masked))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> PromptEndpoint {
        PromptEndpoint {
            user: "deploy".into(),
            host: "10.0.0.5".into(),
            port: 2222,
        }
    }

    #[test]
    fn password_prompt_carries_port_from_endpoint_and_marks_rejection() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
            },
            Some(endpoint()),
            true,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::Password {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
                port: 2222,
                rejected: true,
            }
        );
    }

    #[test]
    fn banner_is_not_a_blocking_model() {
        assert!(
            PromptModel::from_prompt(AuthPromptKind::Banner { text: "hi".into() }, None, false)
                .is_none()
        );
    }

    #[test]
    fn fr_a6_remember_overwrites() {
        let (resp, write) = password_submit("u", "h", 22, "new".into(), true, true);
        assert!(matches!(resp, AuthResponse::Secret(_)));
        assert_eq!(
            write,
            KeychainWrite::SetPassword {
                user: "u".into(),
                host: "h".into(),
                port: 22,
                secret: "new".into(),
            }
        );
    }

    #[test]
    fn fr_a6_rejected_without_remember_deletes_stale_entry() {
        let (_resp, write) = password_submit("u", "h", 22, "new".into(), false, true);
        assert_eq!(
            write,
            KeychainWrite::DeletePassword {
                user: "u".into(),
                host: "h".into(),
                port: 22,
            }
        );
    }

    #[test]
    fn non_rejection_without_remember_never_touches_keychain() {
        let (_resp, write) = password_submit("u", "h", 22, "pw".into(), false, false);
        assert_eq!(write, KeychainWrite::None);
    }

    #[test]
    fn passphrase_remember_stores_by_key_path() {
        let (_resp, write) = passphrase_submit("/home/u/.ssh/id_ed25519", "pp".into(), true, false);
        assert_eq!(
            write,
            KeychainWrite::SetKeyPassphrase {
                key_path: "/home/u/.ssh/id_ed25519".into(),
                secret: "pp".into(),
            }
        );
        let (_r, w) = passphrase_submit("/k", "pp".into(), false, false);
        assert_eq!(w, KeychainWrite::None);
    }

    /// The passphrase side of `fr_a6_rejected_without_remember_deletes_stale_entry`
    /// above. It matters more here than it does for a password: a key
    /// passphrase the daemon cannot use is not one wrong login, it is a key
    /// that never opens again, and before this the sheet had no way at all to
    /// let go of one.
    #[test]
    fn passphrase_rejected_without_remember_deletes_stale_entry() {
        let (resp, write) = passphrase_submit("~/.ssh/id_ed25519", "right".into(), false, true);
        assert!(matches!(resp, AuthResponse::Secret(_)));
        assert_eq!(
            write,
            KeychainWrite::DeleteKeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
            }
        );

        // Remembering still wins: the new secret replaces the old one, so
        // there is nothing left to delete.
        let (_r, w) = passphrase_submit("~/.ssh/id_ed25519", "right".into(), true, true);
        assert_eq!(
            w,
            KeychainWrite::SetKeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                secret: "right".into(),
            }
        );
    }

    #[test]
    fn ki_submit_bundles_all_answers() {
        let (resp, write) = ki_submit(Some(&endpoint()), vec!["a".into(), "b".into()], false);
        assert_eq!(resp, AuthResponse::Secrets(vec!["a".into(), "b".into()]));
        assert_eq!(write, KeychainWrite::None);
    }

    /// Keyboard-interactive has no "remember" of its own — an answer may well
    /// be a one-time code — so the only keychain move it makes is letting go
    /// of the stored password the server just turned down. Without it that
    /// password was replayed into every later connection, and the prompt the
    /// user answered here never became the one the next attempt sent.
    #[test]
    fn ki_answer_forgets_the_stored_password_the_server_rejected() {
        let (resp, write) = ki_submit(Some(&endpoint()), vec!["typed".into()], true);
        assert_eq!(resp, AuthResponse::Secrets(vec!["typed".into()]));
        assert_eq!(
            write,
            KeychainWrite::DeletePassword {
                user: "deploy".into(),
                host: "10.0.0.5".into(),
                port: 2222,
            }
        );

        // A prompt nobody could tie to an endpoint has no entry to name, so it
        // must not guess one — deleting the wrong account is worse than
        // leaving the right one in place.
        let (_r, w) = ki_submit(None, vec!["typed".into()], true);
        assert_eq!(w, KeychainWrite::None);
    }

    /// The port has to come from the connection, not from the default: a
    /// prompt for `:2222` that files its keychain entry under `:22` writes a
    /// secret nothing ever reads back.
    #[test]
    fn keyboard_interactive_carries_the_endpoint_and_the_rejection_through() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::KeyboardInteractive {
                name: "2FA".into(),
                instructions: "code please".into(),
                prompts: vec![],
                stored_rejected: true,
            },
            Some(endpoint()),
            false,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::KeyboardInteractive {
                name: "2FA".into(),
                instructions: "code please".into(),
                prompts: vec![],
                endpoint: Some(endpoint()),
                stored_rejected: true,
            }
        );
    }

    #[test]
    fn passphrase_prompt_carries_the_daemons_rejection() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::KeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                comment: String::new(),
                rejected: true,
            },
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::KeyPassphrase {
                key_path: "~/.ssh/id_ed25519".into(),
                comment: String::new(),
                rejected: true,
            }
        );
    }

    #[test]
    fn unknown_host_trust_accepts_and_remembers_abort_rejects() {
        assert_eq!(
            host_key_unknown_decision(true),
            AuthResponse::HostKeyDecision {
                accept: true,
                remember: true
            }
        );
        assert_eq!(
            host_key_unknown_decision(false),
            AuthResponse::HostKeyDecision {
                accept: false,
                remember: false
            }
        );
    }

    #[test]
    fn changed_host_never_auto_accepts() {
        assert!(changed_confirmed("yes"));
        assert!(changed_confirmed("  YES "));
        assert!(!changed_confirmed(""));
        assert!(!changed_confirmed("y"));
        assert!(!changed_confirmed("no"));
        assert_eq!(
            host_key_changed_decision(""),
            AuthResponse::HostKeyDecision {
                accept: false,
                remember: false
            }
        );
        assert_eq!(
            host_key_changed_decision("yes"),
            AuthResponse::HostKeyDecision {
                accept: true,
                remember: true
            }
        );
    }

    /// `changed_confirmed` is also what enables the Override button, so the
    /// button and the decision can never disagree about what "yes" means — the
    /// bug was a button that offered a way through and sent the rejection.
    #[test]
    fn override_is_enabled_by_exactly_what_accepts() {
        for typed in ["", " ", "y", "no", "yesss"] {
            assert!(
                !changed_confirmed(typed),
                "{typed:?} must leave Override disabled"
            );
            assert_eq!(
                host_key_changed_decision(typed),
                AuthResponse::HostKeyDecision {
                    accept: false,
                    remember: false
                }
            );
        }
        for typed in ["yes", "YES", " yes "] {
            assert!(changed_confirmed(typed), "{typed:?} must enable Override");
        }
    }

    /// The host is known, just not by this algorithm — the mild confirmation,
    /// carrying the algorithm it *is* known by, and never the danger sheet.
    #[test]
    fn a_new_algorithm_raises_the_unknown_host_sheet_not_the_changed_one() {
        let m = PromptModel::from_prompt(
            AuthPromptKind::HostKeyUnknown {
                host: "example.com".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint_sha256: "SHA256:new".into(),
                previously_known_as: Some("ssh-rsa".into()),
            },
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            m,
            PromptModel::HostKeyUnknown {
                host: "example.com".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint: "SHA256:new".into(),
                previously_known_as: Some("ssh-rsa".into()),
            }
        );
        assert_eq!(m.input_count(), 0);
    }
}

#[cfg(test)]
mod modal_tests {
    use super::*;
    use anterm_ui_kit::config::Config;
    use gpui::{TestAppContext, VisualTestContext};
    use std::cell::Cell;
    use std::rc::Rc;

    #[gpui::test]
    fn cancelling_prompt_clears_visibility_and_emits_dismissed(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::presets::load_registry(cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
        });
        let dismissed = Rc::new(Cell::new(0usize));
        let dismissed_clone = dismissed.clone();
        let mut modal_handle: Option<Entity<SshPromptModal>> = None;
        let window = cx.add_window(|window, cx| {
            let modal = cx.new(SshPromptModal::new);
            cx.subscribe(&modal, move |_, _, ev: &SshPromptEvent, _| {
                if matches!(ev, SshPromptEvent::Dismissed { .. }) {
                    dismissed_clone.set(dismissed_clone.get() + 1);
                }
            })
            .detach();
            modal_handle = Some(modal.clone());
            gpui_kit::component::Root::new(modal, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        let modal = modal_handle.unwrap();

        assert!(!modal.read_with(&vcx, |m, _| m.is_visible_for(None)));

        modal.update(&mut vcx, |m, cx| {
            m.model = Some(PromptModel::HostKeyUnknown {
                host: "example".into(),
                port: 22,
                algorithm: "ssh-ed25519".into(),
                fingerprint: "SHA256:zzz".into(),
                previously_known_as: None,
            });
            m.push_connect_error("banner warning".into(), cx);
            cx.notify();
        });
        vcx.run_until_parked();

        assert!(modal.read_with(&vcx, |m, _| m.has_active_prompt()));
        assert!(modal.read_with(&vcx, |m, _| m.is_visible_for(None)));

        modal.update_in(&mut vcx, |m, window, cx| {
            m.cancel(window, cx);
        });
        vcx.run_until_parked();

        assert!(!modal.read_with(&vcx, |m, _| m.has_active_prompt()));
        assert_eq!(dismissed.get(), 1);
        // Banner stays visible until dismissed explicitly.
        assert!(modal.read_with(&vcx, |m, _| m.is_visible_for(None)));
        modal.update(&mut vcx, |m, cx| m.dismiss_banner(0, cx));
        assert!(!modal.read_with(&vcx, |m, _| m.is_visible_for(None)));
    }
}
