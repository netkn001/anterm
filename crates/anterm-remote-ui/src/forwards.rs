use gpui::{
    AnyElement, App, Context, Div, Entity, EventEmitter, FontWeight, Render, Stateful,
    Subscription, Window, div, prelude::*, px, rems,
};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex, v_flex};

use anterm_core::ssh_profile::ForwardRule;
use anterm_core::daemon::protocol::{ForwardStatus, ManagedForward, SshForwardKind, SshForwardRule};
use anterm_ui_kit::panel_tokens::{META, TEXT_MONO};
use anterm_ui_kit::daemon_rpc::{ForwardRoute, PlaceOutcome};
use anterm_ui_kit::intent::WorkspaceIntent;
use anterm_i18n::{L10nKey, t, t_fmt};

/// How far a forward's target endpoint fades when the rule is Dynamic and has
/// no target to name. The rules editor in Settings and the live Forwards panel
/// draw the same row, so they fade it by the same amount.
pub const NO_TARGET_FADE: f32 = 0.4;

/// The managed-forward form's five text fields, read out of their inputs.
///
/// Split out so the question "do these make a rule?" can be asked without a
/// `Window` and answered the same way twice: `add_managed_forward` needs the
/// rule, and `forward_form` needs to know whether there is one yet — that is
/// what decides whether Add is live and whether the form says what is missing.
pub struct ForwardFields {
    /// Whether the whole `ssh -L` grammar is on screen. With it off the form
    /// is one number and the rule is derived from it.
    pub advanced: bool,
    pub kind: SshForwardKind,
    pub bind_host: String,
    pub bind_port: String,
    pub target_host: String,
    pub target_port: String,
    pub description: String,
}

impl ForwardFields {
    /// The rule these fields describe, or `None` while they do not describe
    /// one yet.
    ///
    /// The same conditions the settings sheet's `ForwardRuleForm::collect`
    /// applies, so a rule typed here and a rule typed there are accepted or
    /// refused alike — including port 0, which parses as a `u16` but asks the
    /// OS to pick the port, and there is nowhere in either form to say which
    /// one it picked.
    pub fn collect(&self) -> Option<SshForwardRule> {
        if !self.advanced {
            return self.simple_rule();
        }
        let bind_port: u16 = self.bind_port.trim().parse().ok().filter(|p| *p > 0)?;
        let (target_host, target_port) = if self.kind == SshForwardKind::Dynamic {
            (String::new(), 0)
        } else {
            let port: u16 = self.target_port.trim().parse().ok().filter(|p| *p > 0)?;
            let host = self.target_host.trim();
            if host.is_empty() {
                return None;
            }
            (host.to_string(), port)
        };
        let bind_host = match self.bind_host.trim() {
            // The panel's own default, and the one the strip's tooltip
            // promises: an empty bind host is loopback, not every interface.
            "" => "127.0.0.1".to_string(),
            host => host.to_string(),
        };
        let description = self.description.trim();
        Some(SshForwardRule {
            kind: self.kind,
            bind_host,
            bind_port,
            target_host,
            target_port,
            description: (!description.is_empty()).then(|| description.to_string()),
            enabled: true,
        })
    }

    /// The rule the short form's single number describes: bring the far
    /// side's own `:port` over to the same number here.
    ///
    /// The bind port is that same number rather than 0 so the address is one
    /// anybody can predict — remote :3000 is localhost:3000. When it is taken
    /// on this machine `add_managed_forward` retries with 0 and the OS picks,
    /// which is a better answer than handing the collision back to be solved
    /// by hand.
    fn simple_rule(&self) -> Option<SshForwardRule> {
        let port: u16 = self.target_port.trim().parse().ok().filter(|p| *p > 0)?;
        Some(SshForwardRule {
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".to_string(),
            bind_port: port,
            target_host: "localhost".to_string(),
            target_port: port,
            // No field for it on screen, so nothing to carry: a description
            // left over from a trip through the advanced form is not something
            // this rule was given.
            description: None,
            enabled: true,
        })
    }

    /// Whether the form is still empty enough that saying what is missing
    /// would be nagging rather than helping — the same restraint the settings
    /// sheet shows through `ForwardRuleForm::is_blank`.
    pub fn is_blank(&self) -> bool {
        if !self.advanced {
            return self.target_port.trim().is_empty();
        }
        [
            &self.bind_host,
            &self.bind_port,
            &self.target_host,
            &self.target_port,
            &self.description,
        ]
        .iter()
        .all(|v| v.trim().is_empty())
    }
}

/// The entry a forward request just appended: the one the panel did not have
/// before it asked.
///
/// Ids come from a counter that only goes up, so "none of the ids from before"
/// names the new entry exactly — and it is the new entry that says whether the
/// rule is listening or why it is not.
pub fn added_forward<'a>(
    before: &[u64],
    list: &'a [ManagedForward],
) -> Option<&'a ManagedForward> {
    list.iter().find(|m| !before.contains(&m.id))
}

/// The rule a live forward was made from.
///
/// An edit removes the old forward before adding the new one, so when the new
/// one will not come up this is what puts the old one back.
pub fn rule_of(forward: &ManagedForward) -> SshForwardRule {
    SshForwardRule {
        kind: forward.kind,
        bind_host: forward.bind_host.clone(),
        bind_port: forward.bind_port,
        target_host: forward.target_host.clone(),
        target_port: forward.target_port,
        description: forward.description.clone(),
        enabled: forward.enabled,
    }
}

/// Which of a saved host's rules a live forward was opened from, among
/// those not already switched to `enabled` — the one a switch in the panel
/// has to be written back to.
///
/// Matched on the whole mapping, since a live forward carries no reference to
/// the rule it came from. A saved rule always names its port, so the port the
/// forward bound is the one the rule asked for. A forward added in the panel
/// matches nothing and stays as temporary as it always was.
pub fn saved_rule_index(
    rules: &[ForwardRule],
    forward: &ManagedForward,
    enabled: bool,
) -> Option<usize> {
    rules.iter().position(|r| {
        let saved = anterm_ui_kit::ssh_helpers::map_forward(r);
        saved.enabled != enabled
            && saved.kind == forward.kind
            && saved.bind_host == forward.bind_host
            && saved.bind_port == forward.bind_port
            && saved.target_host == forward.target_host
            && saved.target_port == forward.target_port
    })
}

/// Value snapshot passed to [`render_ssh_status_strip`] for a disconnected SSH pane.
#[derive(Clone, Debug)]
pub struct SshStatusStripProps {
    pub host: String,
    pub reason: Option<String>,
    pub profile_id: Option<uuid::Uuid>,
}

/// Floating disconnect notice pill shown over a disconnected SSH terminal pane.
pub fn render_ssh_status_strip(
    props: SshStatusStripProps,
    on_intent: impl Fn(WorkspaceIntent, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let SshStatusStripProps {
        host,
        reason,
        profile_id: profile,
    } = props;

    let theme = cx.theme();
    let (danger, foreground) = (theme.danger, theme.foreground);
    let theme_popover = theme.popover;
    let on_reconnect = on_intent.clone();

    let bar = anterm_ui_kit::notice::pill(danger, cx)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(foreground)
                .child(if host.is_empty() {
                    t(L10nKey::ForwardDisconnected).to_string()
                } else {
                    t_fmt(L10nKey::ForwardDisconnectedFrom, &[("host", &host)])
                }),
        )
        .children(reason.map(|reason| {
            let full = reason.clone();
            div()
                .id("ssh-strip-reason")
                .max_w(px(360.))
                .truncate()
                .text_color(danger)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                })
                .child(reason)
        }))
        .children(
            anterm_ui_kit::intent::key_hint("RestartSshSession", cx)
                .map(|keys| div().child(format!("· {keys}"))),
        )
        .child(
            Button::new("ssh-reconnect")
                .label(anterm_i18n::t(anterm_i18n::L10nKey::Reconnect))
                .custom(anterm_ui_kit::theme::inverted_button(theme_popover, cx))
                .small()
                .on_click(move |_, window, cx| {
                    on_reconnect(WorkspaceIntent::RestartActiveSshSession, window, cx);
                }),
        )
        .children(profile.map(|id| {
            let on_edit = on_intent.clone();
            Button::new("ssh-edit-profile")
                .label(anterm_i18n::t(anterm_i18n::L10nKey::SshEditProfile))
                .ghost()
                .small()
                .on_click(move |_, window, cx| {
                    on_edit(WorkspaceIntent::OpenSshProfile(id), window, cx);
                })
        }));
    bar.into_any_element()
}

/// Semantic events emitted by [`ForwardsPanel`] back to its workspace host.
#[derive(Clone, Debug)]
pub enum ForwardsEvent {
    Intent(WorkspaceIntent),
    RememberRuleEnabled {
        pane_id: u64,
        forward: ManagedForward,
        enabled: bool,
    },
}

/// Detached GPUI entity owning the live port-forward list, inline add/edit
/// form, and keyboard focus/return lifecycle.
pub struct ForwardsPanel {
    form_pane_id: Option<u64>,
    managed: Vec<ManagedForward>,
    kind: SshForwardKind,
    bind_host: Entity<InputState>,
    bind_port: Entity<InputState>,
    target_host: Entity<InputState>,
    target_port: Entity<InputState>,
    description: Entity<InputState>,
    editing: Option<ManagedForward>,
    error: Option<String>,
    subs: Vec<Subscription>,
    advanced: bool,
}

impl EventEmitter<ForwardsEvent> for ForwardsPanel {}

impl ForwardsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let bind_host = cx.new(|cx| InputState::new(window, cx).default_value("127.0.0.1"));
        let bind_port = cx.new(|cx| InputState::new(window, cx).placeholder("8080"));
        let target_host = cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1"));
        let target_port = cx.new(|cx| InputState::new(window, cx).placeholder("80"));
        let description = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(L10nKey::AppPlaceholderDescription))
        });
        Self {
            form_pane_id: None,
            managed: Vec::new(),
            kind: SshForwardKind::Local,
            bind_host,
            bind_port,
            target_host,
            target_port,
            description,
            editing: None,
            error: None,
            subs: Vec::new(),
            advanced: false,
        }
    }

    pub fn managed_for(&self, pane_id: u64) -> Vec<ManagedForward> {
        self.managed
            .iter()
            .filter(|m| m.pane_id == pane_id)
            .cloned()
            .collect()
    }

    pub fn managed(&self) -> &[ManagedForward] {
        &self.managed
    }

    pub fn set_managed(&mut self, managed: Vec<ManagedForward>, cx: &mut Context<Self>) {
        self.managed = managed;
        cx.notify();
    }

    pub fn clear_managed(&mut self, cx: &mut Context<Self>) {
        if !self.managed.is_empty() {
            self.managed.clear();
            cx.notify();
        }
    }

    pub fn is_form_open_for(&self, pane_id: u64) -> bool {
        self.form_pane_id == Some(pane_id)
    }

    fn route(pane_id: u64) -> ForwardRoute {
        ForwardRoute { pane_id }
    }

    pub fn refresh_managed(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        self.managed = Self::route(pane_id).list();
        cx.notify();
    }

    fn set_kind(&mut self, kind: SshForwardKind, cx: &mut Context<Self>) {
        self.kind = kind;
        cx.notify();
    }

    pub fn fields(&self, cx: &App) -> ForwardFields {
        let val = |input: &Entity<InputState>| input.read(cx).value().to_string();
        ForwardFields {
            advanced: self.advanced,
            kind: self.kind,
            bind_host: val(&self.bind_host),
            bind_port: val(&self.bind_port),
            target_host: val(&self.target_host),
            target_port: val(&self.target_port),
            description: val(&self.description),
        }
    }

    pub fn add_forward(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fields = self.fields(cx);
        let Some(mut rule) = fields.collect() else {
            return;
        };
        let route = Self::route(pane_id);
        let previous = self.editing.clone();
        if let Some(old) = &previous {
            rule.enabled = old.enabled;
        }
        if let Some(old) = &previous {
            let Some(list) = route.remove(old.id) else {
                self.error = Some(t(L10nKey::ForwardRequestFailed).to_string());
                cx.notify();
                return;
            };
            self.managed = list;
        }

        let retry_free_port = !fields.advanced && rule.bind_port != 0;
        let failure = match self.place_forward(&route, rule.clone()) {
            PlaceOutcome::Placed => None,
            PlaceOutcome::Unreachable(msg) => Some(msg),
            PlaceOutcome::Rejected(msg) if !retry_free_port => Some(msg),
            PlaceOutcome::Rejected(_) => {
                match self.place_forward(
                    &route,
                    SshForwardRule {
                        bind_port: 0,
                        ..rule.clone()
                    },
                ) {
                    PlaceOutcome::Placed => None,
                    PlaceOutcome::Rejected(msg) | PlaceOutcome::Unreachable(msg) => Some(msg),
                }
            }
        };

        if let Some(msg) = failure {
            if let Some(old) = &previous {
                let before: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
                if let Some(list) = route.add(rule_of(old)) {
                    if let Some(restored) = added_forward(&before, &list) {
                        self.editing = Some(restored.clone());
                    }
                    self.managed = list;
                }
            }
            self.error = Some(msg);
            cx.notify();
            return;
        }

        self.editing = None;
        self.error = None;
        self.form_pane_id = None;
        self.subs.clear();
        for input in [
            &self.bind_port,
            &self.target_host,
            &self.target_port,
            &self.description,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn place_forward(&mut self, route: &ForwardRoute, rule: SshForwardRule) -> PlaceOutcome {
        let before: Vec<u64> = self.managed.iter().map(|m| m.id).collect();
        let Some(list) = route.add(rule) else {
            return PlaceOutcome::Unreachable(t(L10nKey::ForwardRequestFailed).to_string());
        };
        let broken = added_forward(&before, &list)
            .filter(|added| added.enabled)
            .and_then(|added| match &added.status {
                ForwardStatus::Error(msg) => Some((added.id, msg.clone())),
                ForwardStatus::Listening => None,
            });
        self.managed = list;
        let Some((id, msg)) = broken else {
            return PlaceOutcome::Placed;
        };
        if let Some(list) = route.remove(id) {
            self.managed = list;
        }
        PlaceOutcome::Rejected(msg)
    }

    pub fn edit_forward(
        &mut self,
        forward: ManagedForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kind = forward.kind;
        self.form_pane_id = Some(forward.pane_id);
        self.error = None;
        self.advanced = true;
        let target_port = if forward.target_port == 0 {
            String::new()
        } else {
            forward.target_port.to_string()
        };
        let fields: [(&Entity<InputState>, String); 5] = [
            (&self.bind_host, forward.bind_host.clone()),
            (&self.bind_port, forward.bind_port.to_string()),
            (&self.target_host, forward.target_host.clone()),
            (&self.target_port, target_port),
            (
                &self.description,
                forward.description.clone().unwrap_or_default(),
            ),
        ];
        for (input, value) in fields {
            input.update(cx, |input, cx| input.set_value(&value, window, cx));
        }
        let pane_id = forward.pane_id;
        self.editing = Some(forward);
        self.arm_form(pane_id, window, cx);
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.error = None;
        for input in [
            &self.bind_port,
            &self.target_host,
            &self.target_port,
            &self.description,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.bind_host
            .update(cx, |input, cx| input.set_value("127.0.0.1", window, cx));
        cx.notify();
    }

    pub fn remove_forward(
        &mut self,
        pane_id: u64,
        forward_id: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(list) = Self::route(pane_id).remove(forward_id) {
            self.managed = list;
        }
        cx.notify();
    }

    pub fn set_forward_enabled(
        &mut self,
        pane_id: u64,
        forward_id: u64,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let route = Self::route(pane_id);
        let forward = self.managed.iter().find(|m| m.id == forward_id).cloned();
        match route.set_enabled(forward_id, enabled) {
            Ok(list) => {
                self.managed = list;
                if let Some(forward) = forward {
                    cx.emit(ForwardsEvent::RememberRuleEnabled {
                        pane_id,
                        forward,
                        enabled,
                    });
                }
            }
            Err(e) => {
                window.push_notification(
                    anterm_ui_kit::host_ops::failure(
                        t_fmt(L10nKey::ForwardSwitchFailed, &[("error", &e.to_string())]),
                        &e,
                    ),
                    cx,
                );
                self.managed = route.list();
            }
        }
        cx.notify();
    }

    pub fn toggle_form(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.form_pane_id == Some(pane_id) {
            self.close_form(window, cx);
            return;
        }
        self.form_pane_id = Some(pane_id);
        self.advanced = false;
        self.kind = SshForwardKind::Local;
        self.cancel_edit(window, cx);
        self.refresh_managed(pane_id, cx);
        self.arm_form(pane_id, window, cx);
    }

    fn arm_form(&mut self, pane_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let inputs = [
            self.bind_host.clone(),
            self.bind_port.clone(),
            self.target_host.clone(),
            self.target_port.clone(),
            self.description.clone(),
        ];
        self.subs = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    move |this, _input, ev: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter { .. } = ev {
                            this.add_forward(pane_id, window, cx);
                        }
                    },
                )
            })
            .collect();
        inputs[0].update(cx, |s, cx| s.focus(window, cx));
    }

    fn toggle_advanced(&mut self, cx: &mut Context<Self>) {
        self.advanced = !self.advanced;
        self.error = None;
        cx.notify();
    }

    pub fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_open = self.form_pane_id.take().is_some();
        self.subs.clear();
        self.cancel_edit(window, cx);
        if was_open {
            cx.emit(ForwardsEvent::Intent(WorkspaceIntent::FocusActiveTerminal));
        }
    }

    pub fn render_forward_row(
        &self,
        forward: &ManagedForward,
        mono: &gpui::SharedString,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let letter = match forward.kind {
            SshForwardKind::Local => "L",
            SshForwardKind::Remote => "R",
            SshForwardKind::Dynamic => "D",
        };
        let off = !forward.enabled;
        let errored = !off && matches!(forward.status, ForwardStatus::Error(_));
        let bind = if matches!(forward.bind_host.as_str(), "127.0.0.1" | "localhost" | "") {
            forward.bind_port.to_string()
        } else {
            format!("{}:{}", forward.bind_host, forward.bind_port)
        };
        let mapping = match forward.kind {
            SshForwardKind::Dynamic => "SOCKS".to_string(),
            _ => format!("→ {}:{}", forward.target_host, forward.target_port),
        };
        let tail = match &forward.status {
            _ if off => mapping,
            ForwardStatus::Error(msg) => msg.clone(),
            ForwardStatus::Listening => mapping,
        };
        let pane_id = forward.pane_id;
        let forward_id = forward.id;
        let forward_for_edit = forward.clone();
        let group = gpui::SharedString::from(format!("panel-forward-{forward_id}"));

        h_flex()
            .id(("panel-forward", forward_id as usize))
            .group(group.clone())
            .items_center()
            .gap_2()
            .min_h(px(26.))
            .py(gpui::rems(3. / 16.))
            .px(px(anterm_ui_kit::panel_tokens::ROW_INSET))
            .rounded(anterm_ui_kit::panel_tokens::ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.edit_forward(forward_for_edit.clone(), window, cx)
            }))
            .child(anterm_ui_kit::panel_tokens::git_badge(
                letter,
                if errored { theme.danger } else { muted },
                mono,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .when(off, |col| col.opacity(0.5))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(anterm_ui_kit::panel_tokens::info_chip(
                                &bind,
                                theme.accent,
                                theme.foreground,
                                mono,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rems(TEXT_MONO))
                                    .font_family(mono.clone())
                                    .text_color(if errored { theme.danger } else { muted })
                                    .child(tail),
                            ),
                    )
                    .when_some(forward.description.clone(), |this, desc| {
                        this.child(
                            div()
                                .truncate()
                                .text_size(rems(META))
                                .text_color(muted)
                                .child(desc),
                        )
                    }),
            )
            .child({
                let tip = if off {
                    t(L10nKey::ForwardTooltipTurnOn)
                } else {
                    t(L10nKey::ForwardTooltipTurnOff)
                };
                div()
                    .id(("panel-forward-enabled-tip", forward_id as usize))
                    .flex_shrink_0()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .child(
                        anterm_ui_kit::kit::switch((
                            "panel-forward-enabled",
                            forward_id as usize,
                        ))
                        .checked(forward.enabled)
                        .small()
                        .on_click(cx.listener(
                            move |this, on: &bool, window, cx| {
                                this.set_forward_enabled(pane_id, forward_id, *on, window, cx)
                            },
                        )),
                    )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        anterm_ui_kit::window_chrome::chrome_tile(
                            Button::new(("panel-forward-del", forward_id as usize))
                                .icon(IconName::Close)
                                .xsmall(),
                            false,
                            cx,
                        )
                        .w(px(anterm_ui_kit::window_chrome::MIN_TARGET))
                        .h(px(anterm_ui_kit::window_chrome::MIN_TARGET))
                        .rounded(px(anterm_ui_kit::window_chrome::RAIL_TILE_RADIUS))
                        .tooltip(t(L10nKey::ForwardTooltipRemove))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.remove_forward(pane_id, forward_id, cx)
                        })),
                    ),
            )
    }

    fn render_form(&self, pane_id: u64, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let kind = self.kind;
        let editing = self.editing.is_some();
        let advanced = self.advanced;
        let fields = self.fields(cx);
        let rule = fields.collect();
        let complete = rule.is_some();
        let incomplete = !complete && !fields.is_blank();
        let selected = match kind {
            SshForwardKind::Local => 0,
            SshForwardKind::Remote => 1,
            SshForwardKind::Dynamic => 2,
        };
        let needs_target = kind != SshForwardKind::Dynamic;

        let pair = |label: &'static str, host: &Entity<InputState>, port: &Entity<InputState>| {
            h_flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_none()
                        .w(px(30.))
                        .text_size(rems(META))
                        .text_color(muted)
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(Input::new(host).xsmall()))
                .child(div().text_size(rems(META)).text_color(muted).child(":"))
                .child(div().w(px(52.)).child(Input::new(port).xsmall()))
        };

        v_flex()
            .px(px(anterm_ui_kit::panel_tokens::TEXT_INSET))
            .pt_1p5()
            .pb_0p5()
            .gap(gpui::rems(5. / 16.))
            .on_key_down(
                cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.close_form(window, cx);
                    }
                }),
            )
            .when(!advanced, |form| {
                form.child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .flex_none()
                                .text_size(rems(META))
                                .text_color(muted)
                                .child(t(L10nKey::ForwardPortLabel)),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .child(Input::new(&self.target_port).xsmall()),
                        )
                        .children(rule.as_ref().map(|r| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(rems(META))
                                .text_color(muted)
                                .child(t_fmt(
                                    L10nKey::ForwardPortHere,
                                    &[("port", &r.bind_port.to_string())],
                                ))
                        })),
                )
            })
            .when(advanced, |form| {
                form.child(anterm_ui_kit::kit::segmented_control(
                    "ssh-managed-forward-kind",
                    &[
                        t(L10nKey::ForwardLocal),
                        t(L10nKey::ForwardRemote),
                        t(L10nKey::ForwardDynamic),
                    ],
                    selected,
                    cx,
                    move |this, ix, _window, cx| {
                        let kind = match ix {
                            1 => SshForwardKind::Remote,
                            2 => SshForwardKind::Dynamic,
                            _ => SshForwardKind::Local,
                        };
                        this.set_kind(kind, cx);
                    },
                ))
                .child(pair(
                    t(L10nKey::ForwardBindLabel),
                    &self.bind_host,
                    &self.bind_port,
                ))
                .child(
                    div()
                        .opacity(if needs_target { 1.0 } else { NO_TARGET_FADE })
                        .child(pair(
                            if needs_target {
                                t(L10nKey::ForwardToLabel)
                            } else {
                                t(L10nKey::ForwardSocksLabel)
                            },
                            &self.target_host,
                            &self.target_port,
                        )),
                )
                .child(Input::new(&self.description).xsmall())
            })
            .when(incomplete, |form| {
                form.child(div().text_size(rems(META)).text_color(danger).child(
                    match (advanced, needs_target) {
                        (false, _) => t(L10nKey::ForwardNeedsPort),
                        (true, true) => t(L10nKey::SettingsFwdNeedsBoth),
                        (true, false) => t(L10nKey::SettingsFwdNeedsListen),
                    },
                ))
            })
            .when_some(self.error.clone(), |form, msg| {
                form.child(div().text_size(rems(META)).text_color(danger).child(msg))
            })
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .pt(px(1.))
                    .child(div().when(!editing, |slot| {
                        slot.child(
                            Button::new(("ssh-managed-forward-advanced", pane_id))
                                .label(if advanced {
                                    t(L10nKey::ForwardSimpleToggle)
                                } else {
                                    t(L10nKey::ForwardAdvancedToggle)
                                })
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.toggle_advanced(cx)
                                })),
                        )
                    }))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("ssh-managed-forward-cancel", pane_id))
                                    .label(t(L10nKey::Cancel))
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.close_form(window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("ssh-managed-forward-add", pane_id))
                                    .label(if editing {
                                        t(L10nKey::Save)
                                    } else {
                                        t(L10nKey::ForwardAdd)
                                    })
                                    .custom(anterm_ui_kit::theme::inverted_button(
                                        gpui::rgb(sf.base).into(),
                                        cx,
                                    ))
                                    .xsmall()
                                    .disabled(!complete)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.add_forward(pane_id, window, cx)
                                    })),
                            ),
                    ),
            )
    }
}

impl Render for ForwardsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.form_pane_id {
            Some(pane_id) => self.render_form(pane_id, cx).into_any_element(),
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;
    use anterm_ui_kit::config::Config;
    use gpui::{Focusable as _, TestAppContext, VisualTestContext};
    use std::cell::Cell;
    use std::rc::Rc;

    fn listening(id: u64) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 1,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".to_string(),
            bind_port: 8080,
            target_host: "10.0.0.5".to_string(),
            target_port: 80,
            description: None,
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    #[gpui::test]
    fn standalone_forwards_panel_lifecycle_and_failed_add_preserves_state(
        cx: &mut TestAppContext,
    ) {
        anterm_ui_kit::config::pin_test_config_dir();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::presets::load_registry(cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
        });
        let focus_restored = Rc::new(Cell::new(0usize));
        let focus_clone = focus_restored.clone();
        let mut panel_slot = None;
        let window = cx.add_window(|window, cx| {
            let panel = cx.new(|cx| ForwardsPanel::new(window, cx));
            cx.subscribe(&panel, move |_, _, ev: &ForwardsEvent, _| {
                if matches!(ev, ForwardsEvent::Intent(WorkspaceIntent::FocusActiveTerminal)) {
                    focus_clone.set(focus_clone.get() + 1);
                }
            })
            .detach();
            panel_slot = Some(panel.clone());
            gpui_kit::component::Root::new(panel, window, cx)
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        let panel = panel_slot.unwrap();

        panel.update_in(&mut vcx, |p, window, cx| {
            assert!(p.subs.is_empty());
            p.toggle_form(1, window, cx);
            assert!(p.is_form_open_for(1));
            assert_eq!(p.subs.len(), 5);
            assert!(p.bind_host.read(cx).focus_handle(cx).is_focused(window));

            // Failed add (no daemon in test dir) preserves existing managed list & reports error.
            p.set_managed(vec![listening(1)], cx);
            p.advanced = true;
            p.bind_port.update(cx, |i, cx| i.set_value("9000", window, cx));
            p.target_host.update(cx, |i, cx| i.set_value("127.0.0.1", window, cx));
            p.target_port.update(cx, |i, cx| i.set_value("22", window, cx));
            p.add_forward(1, window, cx);
            assert_eq!(p.managed().len(), 1);
            assert!(p.error.is_some());
            assert!(p.is_form_open_for(1));

            p.close_form(window, cx);
            assert!(!p.is_form_open_for(1));
            assert!(p.subs.is_empty());
        });
        vcx.run_until_parked();
        assert_eq!(focus_restored.get(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(kind: SshForwardKind, bind_port: &str, host: &str, port: &str) -> ForwardFields {
        ForwardFields {
            advanced: true,
            kind,
            bind_host: "127.0.0.1".to_string(),
            bind_port: bind_port.to_string(),
            target_host: host.to_string(),
            target_port: port.to_string(),
            description: String::new(),
        }
    }

    fn managed(id: u64, bind_port: u16) -> ManagedForward {
        ManagedForward {
            id,
            pane_id: 7,
            kind: SshForwardKind::Local,
            bind_host: "127.0.0.1".to_string(),
            bind_port,
            target_host: "10.0.0.5".to_string(),
            target_port: 80,
            description: Some("the staging box".to_string()),
            status: ForwardStatus::Listening,
            enabled: true,
        }
    }

    #[test]
    fn a_complete_local_rule_is_collected() {
        let rule = fields(SshForwardKind::Local, "8080", "10.0.0.5", "80")
            .collect()
            .expect("a bind port and a target make a rule");
        assert_eq!(rule.bind_port, 8080);
        assert_eq!(rule.target_host, "10.0.0.5");
        assert_eq!(rule.target_port, 80);
        assert_eq!(rule.description, None);
    }

    #[test]
    fn a_half_typed_rule_is_not_a_rule() {
        assert!(
            fields(SshForwardKind::Local, "", "10.0.0.5", "80")
                .collect()
                .is_none()
        );
        assert!(
            fields(SshForwardKind::Local, "8080", "", "80")
                .collect()
                .is_none()
        );
        assert!(
            fields(SshForwardKind::Local, "8080", "10.0.0.5", "")
                .collect()
                .is_none()
        );
        assert!(
            fields(SshForwardKind::Local, "http", "10.0.0.5", "80")
                .collect()
                .is_none(),
            "a service name is not a port"
        );
    }

    #[test]
    fn port_zero_is_refused_rather_than_quietly_ephemeral() {
        assert!(
            fields(SshForwardKind::Local, "0", "10.0.0.5", "80")
                .collect()
                .is_none(),
            "there is nowhere in this form to say which port the OS picked"
        );
        assert!(
            fields(SshForwardKind::Local, "8080", "10.0.0.5", "0")
                .collect()
                .is_none()
        );
    }

    /// The short form: one number, and the rule it makes reaches the far
    /// side's own loopback and comes out here under the same number.
    #[test]
    fn one_number_is_a_whole_rule_in_the_short_form() {
        let mut form = fields(SshForwardKind::Local, "", "", "3000");
        form.advanced = false;
        // Whatever a trip through the advanced form left behind is not part of
        // what the short form was asked for.
        form.bind_host = "0.0.0.0".to_string();
        form.description = "left over".to_string();
        let rule = form.collect().expect("a port is enough");
        assert_eq!(rule.kind, SshForwardKind::Local);
        assert_eq!(rule.bind_host, "127.0.0.1");
        assert_eq!(rule.bind_port, 3000, "the address has to be predictable");
        assert_eq!(rule.target_host, "localhost");
        assert_eq!(rule.target_port, 3000);
        assert_eq!(rule.description, None);
    }

    #[test]
    fn the_short_form_is_blank_until_the_port_is_typed() {
        let mut form = fields(SshForwardKind::Local, "8080", "10.0.0.5", "");
        form.advanced = false;
        assert!(
            form.is_blank(),
            "fields the short form does not show cannot make it dirty"
        );
        assert!(form.collect().is_none());
        form.target_port = "http".to_string();
        assert!(
            !form.is_blank(),
            "a typed port that is not one still counts"
        );
        assert!(form.collect().is_none());
    }

    #[test]
    fn a_socks_proxy_needs_nothing_but_a_port_to_listen_on() {
        let rule = fields(SshForwardKind::Dynamic, "1080", "", "")
            .collect()
            .expect("a dynamic forward has no target");
        assert_eq!(rule.bind_port, 1080);
        assert_eq!(rule.target_host, "");
        assert_eq!(rule.target_port, 0);
    }

    #[test]
    fn an_untouched_form_is_blank_and_a_touched_one_is_not() {
        let mut form = fields(SshForwardKind::Local, "", "", "");
        form.bind_host = String::new();
        assert!(form.is_blank());
        form.description = "  ".to_string();
        assert!(form.is_blank(), "whitespace is not typing");
        form.bind_port = "8".to_string();
        assert!(!form.is_blank());
    }

    #[test]
    fn a_rule_survives_the_round_trip_through_a_live_forward() {
        let rule = rule_of(&managed(3, 8080));
        assert_eq!(rule.kind, SshForwardKind::Local);
        assert_eq!(rule.bind_host, "127.0.0.1");
        assert_eq!(rule.bind_port, 8080);
        assert_eq!(rule.target_host, "10.0.0.5");
        assert_eq!(rule.target_port, 80);
        assert_eq!(rule.description.as_deref(), Some("the staging box"));
    }

    #[test]
    fn the_entry_an_add_appended_is_the_one_that_was_not_there_before() {
        let list = vec![managed(1, 8080), managed(4, 9090)];
        let added = added_forward(&[1], &list).expect("the new entry");
        assert_eq!(added.id, 4);
        assert!(
            added_forward(&[1, 4], &list).is_none(),
            "nothing was added, so there is nothing to point at"
        );
    }

    #[test]
    fn a_switched_off_forward_keeps_its_switch_through_an_edit() {
        let mut off = managed(3, 8080);
        off.enabled = false;
        assert!(!rule_of(&off).enabled);
        assert!(rule_of(&managed(3, 8080)).enabled);
    }

    /// The panel's switch is written back to the saved host only for the rule
    /// the forward was opened from, and a rule added in the panel — which no
    /// saved rule describes — is left as temporary as it always was.
    #[test]
    fn a_switch_is_written_back_to_the_saved_rule_it_came_from() {
        use anterm_core::ssh_profile::{ForwardKind, HostPort};
        let saved = |target: &str, enabled: bool| ForwardRule {
            kind: ForwardKind::Local,
            bind: HostPort::new("127.0.0.1", 8080),
            target: HostPort::new(target, 80),
            description: "the staging box".to_string(),
            enabled,
        };
        let rules = vec![saved("10.0.0.9", true), saved("10.0.0.5", true)];
        let live = managed(3, 8080);
        assert_eq!(saved_rule_index(&rules, &live, false), Some(1));
        assert_eq!(
            saved_rule_index(&rules, &live, true),
            None,
            "already on: nothing to write"
        );

        let mut elsewhere = managed(3, 8081);
        elsewhere.description = None;
        assert_eq!(saved_rule_index(&rules, &elsewhere, false), None);

        let mut remote = managed(3, 8080);
        remote.kind = SshForwardKind::Remote;
        assert_eq!(saved_rule_index(&rules, &remote, false), None);
    }
}
