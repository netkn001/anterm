//! Mobile Companion settings page: E2EE `iroh` QUIC gateway toggles,
//! single-use QR pairing card, and paired device RBAC / revocation list.

use anterm_gateway::{
    GatewayRuntime, GatewayStore, MobileGatewayStoreData, MobileRole, qr_matrix,
};
use gpui::{ClipboardItem, SharedString, div, px, rgb};

use super::kit::{self, BtnKind, Tk, fs};
use super::*;

fn role_label(role: MobileRole) -> &'static str {
    match role {
        MobileRole::Interactive => t(L10nKey::SettingsMobileRoleInteractive),
        MobileRole::ObserveOnly => t(L10nKey::SettingsMobileRoleObserveOnly),
        MobileRole::Full => t(L10nKey::SettingsMobileRoleFull),
    }
}

fn role_index(role: MobileRole) -> usize {
    match role {
        MobileRole::Interactive => 0,
        MobileRole::ObserveOnly => 1,
        MobileRole::Full => 2,
    }
}

fn role_from_index(idx: usize) -> MobileRole {
    match idx {
        1 => MobileRole::ObserveOnly,
        2 => MobileRole::Full,
        _ => MobileRole::Interactive,
    }
}

fn short_node_id(node_id: &str) -> String {
    if node_id.len() <= 16 {
        return node_id.to_string();
    }
    let head = &node_id[..8];
    let tail = &node_id[node_id.len() - 6..];
    format!("{head}...{tail}")
}

impl SettingsWindow {
    pub(super) fn init_mobile_store() -> Option<GatewayStore> {
        GatewayStore::open_default().ok()
    }

    pub(super) fn build_mobile_relay_input(
        store: Option<&GatewayStore>,
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let initial = store
            .and_then(|s| s.snapshot().relay_url)
            .unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://relay.example.com")
                .default_value(initial)
        });
        subs.push(
            cx.subscribe_in(&input, window, |this, input, ev, _w, cx| {
                if matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let raw = input.read(cx).value().trim().to_string();
                    let next = (!raw.is_empty()).then_some(raw);
                    this.update_mobile_config(cx, move |cfg| {
                        cfg.relay_url = next;
                    });
                }
            }),
        );
        input
    }

    pub(super) fn build_mobile_rename_input(
        subs: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx));
        subs.push(
            cx.subscribe_in(&input, window, |this, _input, ev, _w, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.commit_mobile_device_rename(cx);
                }
            }),
        );
        input
    }

    fn start_mobile_device_rename(
        &mut self,
        node_id: String,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.mobile_rename_target = Some(node_id);
        let input = self.state.mobile_rename_input.clone();
        input.update(cx, |state, cx| {
            state.set_value(current_name, window, cx);
        });
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn commit_mobile_device_rename(&mut self, cx: &mut Context<Self>) {
        let Some(node_id) = self.state.mobile_rename_target.take() else {
            return;
        };
        let next = self
            .state
            .mobile_rename_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        if next.is_empty() {
            cx.notify();
            return;
        }
        if let Some(store) = self.state.mobile_store.clone() {
            match store.rename_device(&node_id, &next) {
                Ok(_) => self.state.mobile_status_note = None,
                Err(err) => self.state.mobile_status_note = Some(err.to_string()),
            }
        }
        cx.notify();
    }

    fn mobile_snapshot(&self) -> MobileGatewayStoreData {
        self.state
            .mobile_store
            .as_ref()
            .map(GatewayStore::snapshot)
            .unwrap_or_default()
    }

    fn update_mobile_config(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut MobileGatewayStoreData),
    ) {
        let Some(store) = self.state.mobile_store.clone() else {
            return;
        };
        match store.update_config(f) {
            Ok(_) => {
                let status = GatewayRuntime::global().sync_store(&store);
                self.state.mobile_status_note = status.last_error;
            }
            Err(err) => {
                self.state.mobile_status_note = Some(err.to_string());
            }
        }
        cx.notify();
    }

    fn generate_mobile_pairing_qr(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.state.mobile_store.clone() else {
            return;
        };
        let role = self.state.mobile_pair_role;
        let res: anyhow::Result<(String, usize, Vec<bool>)> = (|| {
            let bundle =
                GatewayRuntime::global().issue_live_pairing_bundle(&store, role, None)?;
            let uri = bundle.to_uri();
            let (width, modules) = qr_matrix(&uri)?;
            Ok((uri, width, modules))
        })();

        match res {
            Ok(qr) => {
                self.state.mobile_pairing_qr = Some(qr);
                self.state.mobile_status_note = None;
            }
            Err(err) => {
                self.state.mobile_status_note = Some(err.to_string());
            }
        }
        cx.notify();
    }

    pub fn render_settings_mobile(&self, cx: &mut Context<Self>) -> AnyElement {
        let snap = self.mobile_snapshot();

        let connection_group = self.render_mobile_connection_group(&snap, cx);
        let pairing_group = self.render_mobile_pairing_group(cx);
        let devices_group = self.render_mobile_devices_group(&snap, cx);

        Self::settings_page([connection_group, pairing_group, devices_group])
    }

    fn render_mobile_connection_group(
        &self,
        snap: &MobileGatewayStoreData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enable = self.settings_switch(
            "mobile-enable",
            snap.enabled,
            cx,
            |this, on, _w, cx| {
                this.update_mobile_config(cx, |cfg| cfg.enabled = on);
            },
        );
        let lan_only = self.settings_switch(
            "mobile-lan-only",
            snap.lan_only,
            cx,
            |this, on, _w, cx| {
                this.update_mobile_config(cx, |cfg| cfg.lan_only = on);
            },
        );
        let relay = self
            .settings_text_input(&self.state.mobile_relay_input, PATH_FIELD_W, false, cx)
            .into_any_element();
        let prevent_sleep = self.settings_switch(
            "mobile-prevent-sleep",
            snap.prevent_sleep_while_active,
            cx,
            |this, on, _w, cx| {
                this.update_mobile_config(cx, |cfg| cfg.prevent_sleep_while_active = on);
            },
        );
        let silence_push = self.settings_switch(
            "mobile-silence-push",
            snap.silence_push_when_desktop_active,
            cx,
            |this, on, _w, cx| {
                this.update_mobile_config(cx, |cfg| cfg.silence_push_when_desktop_active = on);
            },
        );

        let live_status = GatewayRuntime::global().status();
        let enable_desc = if live_status.running {
            let endpoint_hint = live_status
                .lan_addrs
                .first()
                .cloned()
                .or(live_status.relay_url.clone())
                .unwrap_or_else(|| "QUIC".to_string());
            format!(
                "{} · Live ({endpoint_hint})",
                t(L10nKey::SettingsMobileEnableDesc)
            )
        } else {
            t(L10nKey::SettingsMobileEnableDesc).to_string()
        };

        let rows = vec![
            self.settings_row(
                t(L10nKey::SettingsMobileEnable),
                enable_desc,
                enable,
                cx,
            )
            .into_any_element(),
            self.settings_row_gated_when(
                t(L10nKey::SettingsMobileLanOnly),
                t(L10nKey::SettingsMobileLanOnlyDesc),
                lan_only,
                !snap.enabled,
                cx,
            )
            .into_any_element(),
            self.settings_row_gated_when(
                t(L10nKey::SettingsMobileRelayUrl),
                t(L10nKey::SettingsMobileRelayUrlDesc),
                relay,
                !snap.enabled || snap.lan_only,
                cx,
            )
            .into_any_element(),
            self.settings_row_gated_when(
                t(L10nKey::SettingsMobilePreventSleep),
                t(L10nKey::SettingsMobilePreventSleepDesc),
                prevent_sleep,
                !snap.enabled,
                cx,
            )
            .into_any_element(),
            self.settings_row_gated_when(
                t(L10nKey::SettingsMobileSilenceWhenActive),
                t(L10nKey::SettingsMobileSilenceWhenActiveDesc),
                silence_push,
                !snap.enabled,
                cx,
            )
            .into_any_element(),
        ];
        self.settings_group(None, None, rows, cx)
    }

    fn render_mobile_pairing_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = Tk::of(cx);
        let role_options = [
            t(L10nKey::SettingsMobileRoleInteractive),
            t(L10nKey::SettingsMobileRoleObserveOnly),
            t(L10nKey::SettingsMobileRoleFull),
        ];
        let role_picker = self.settings_choice(
            "mobile-pair-role",
            &role_options,
            role_index(self.state.mobile_pair_role),
            cx,
            |this, ix, _w, cx| {
                this.state.mobile_pair_role = role_from_index(ix);
                cx.notify();
            },
        );
        let generate_btn = self
            .settings_button(
                "mobile-generate-qr",
                t(L10nKey::SettingsMobilePairGenerate),
                cx,
                |this, _w, cx| {
                    this.generate_mobile_pairing_qr(cx);
                },
            )
            .into_any_element();

        let mut body = vec![
            self.settings_row(
                t(L10nKey::SettingsMobilePairRole),
                t(L10nKey::SettingsMobilePairRoleDesc),
                role_picker,
                cx,
            )
            .into_any_element(),
            h_flex()
                .pt_2()
                .gap_2()
                .items_center()
                .child(generate_btn)
                .when_some(self.state.mobile_status_note.clone(), |row, note| {
                    row.child(div().text_size(fs(12.)).text_color(tk.danger).child(note))
                })
                .into_any_element(),
        ];

        if let Some((uri, width, modules)) = &self.state.mobile_pairing_qr {
            let uri_for_copy = uri.clone();
            let side = (*width).max(1);
            let module_px = if side > 45 { 3.0 } else { 4.0 };
            let qr_grid = v_flex()
                .p(px(12.))
                .rounded_md()
                .bg(rgb(0xFFFFFF))
                .border_1()
                .border_color(tk.k15)
                .children(modules.chunks(side).map(|row| {
                    h_flex().children(row.iter().map(|&dark| {
                        div()
                            .w(px(module_px))
                            .h(px(module_px))
                            .bg(if dark { rgb(0x111111) } else { rgb(0xFFFFFF) })
                    }))
                }));

            let copy_btn = kit::button(
                "mobile-copy-qr-uri",
                t(L10nKey::SettingsMobilePairCopyUri),
                BtnKind::Secondary,
            )
            .on_click(cx.listener(move |_this, _ev, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(uri_for_copy.clone()));
                window.push_notification(t(L10nKey::SettingsCopied), cx);
            }));

            let close_btn = kit::button(
                "mobile-close-qr",
                t(L10nKey::SettingsMobilePairCancel),
                BtnKind::Link,
            )
            .on_click(cx.listener(|this, _ev, _w, cx| {
                this.state.mobile_pairing_qr = None;
                cx.notify();
            }));

            let card = h_flex()
                .mt_3()
                .p_4()
                .gap_4()
                .items_start()
                .rounded_lg()
                .bg(tk.k04)
                .border_1()
                .border_color(tk.k08)
                .child(qr_grid)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .text_size(fs(12.5))
                                .text_color(tk.fg)
                                .child(t(L10nKey::SettingsMobilePairHint)),
                        )
                        .child(
                            div()
                                .text_size(fs(11.))
                                .text_color(tk.k5)
                                .overflow_hidden()
                                .child(SharedString::from(if uri.len() > 72 {
                                    format!("{}...", &uri[..72])
                                } else {
                                    uri.clone()
                                })),
                        )
                        .child(h_flex().gap_2().pt_1().child(copy_btn).child(close_btn)),
                );
            body.push(card.into_any_element());
        }

        self.settings_group(Some(t(L10nKey::SettingsMobilePairGroup)), None, body, cx)
    }

    fn render_mobile_devices_group(
        &self,
        snap: &MobileGatewayStoreData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = Tk::of(cx);
        let role_options = [
            t(L10nKey::SettingsMobileRoleInteractive),
            t(L10nKey::SettingsMobileRoleObserveOnly),
            t(L10nKey::SettingsMobileRoleFull),
        ];

        let mut rows: Vec<AnyElement> = Vec::new();
        if snap.devices.is_empty() {
            rows.push(
                div()
                    .py_3()
                    .text_size(fs(12.5))
                    .text_color(tk.k5)
                    .child(t(L10nKey::SettingsMobileNoDevices))
                    .into_any_element(),
            );
        } else {
            for (idx, device) in snap.devices.iter().enumerate() {
                let is_renaming =
                    self.state.mobile_rename_target.as_deref() == Some(device.node_id.as_str());
                let node_for_role = device.node_id.clone();
                let node_for_rename = device.node_id.clone();
                let name_for_rename = device.name.clone();
                let node_for_revoke = device.node_id.clone();

                let controls = if is_renaming {
                    let input_el = self
                        .settings_text_input(&self.state.mobile_rename_input, 180., false, cx)
                        .into_any_element();
                    let save_btn = kit::button(
                        SharedString::from(format!("mobile-device-rename-save-{idx}")),
                        t(L10nKey::Save),
                        BtnKind::Primary,
                    )
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        this.commit_mobile_device_rename(cx);
                    }));
                    let cancel_btn = kit::button(
                        SharedString::from(format!("mobile-device-rename-cancel-{idx}")),
                        t(L10nKey::Cancel),
                        BtnKind::Link,
                    )
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        this.state.mobile_rename_target = None;
                        cx.notify();
                    }));
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(input_el)
                        .child(save_btn)
                        .child(cancel_btn)
                        .into_any_element()
                } else {
                    let role_ctrl = self.settings_choice(
                        SharedString::from(format!("mobile-device-role-{idx}")),
                        &role_options,
                        role_index(device.role),
                        cx,
                        move |this, ix, _w, cx| {
                            if let Some(store) = this.state.mobile_store.clone() {
                                let _ = store.set_device_role(&node_for_role, role_from_index(ix));
                                cx.notify();
                            }
                        },
                    );
                    let rename_btn = kit::button(
                        SharedString::from(format!("mobile-device-rename-{idx}")),
                        t(L10nKey::SettingsMobileRename),
                        BtnKind::Secondary,
                    )
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.start_mobile_device_rename(
                            node_for_rename.clone(),
                            name_for_rename.clone(),
                            window,
                            cx,
                        );
                    }));
                    let revoke_btn = kit::button(
                        SharedString::from(format!("mobile-device-revoke-{idx}")),
                        t(L10nKey::SettingsMobileRevoke),
                        BtnKind::Danger,
                    )
                    .on_click(cx.listener(move |this, _ev, _w, cx| {
                        if let Some(store) = this.state.mobile_store.clone() {
                            let _ = store.revoke_device(&node_for_revoke);
                            cx.notify();
                        }
                    }));
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(role_ctrl)
                        .child(rename_btn)
                        .child(revoke_btn)
                        .into_any_element()
                };

                let subtitle = format!(
                    "{} · {}",
                    short_node_id(&device.node_id),
                    role_label(device.role)
                );

                rows.push(
                    self.settings_row(device.name.clone(), subtitle, controls, cx)
                        .into_any_element(),
                );
            }
        }

        self.settings_group(Some(t(L10nKey::SettingsMobilePairedDevices)), None, rows, cx)
    }
}
