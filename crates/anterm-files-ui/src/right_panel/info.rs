use std::path::PathBuf;

use anterm_i18n::{L10nKey, t, t_fmt};
use gpui::{AnyElement, Context, Window, div, prelude::*, px, rems};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, WindowExt as _, h_flex, v_flex};

use anterm_ui_kit::config::RightPanelTab;
use anterm_core::daemon::protocol::{ManagedForward, PaneProcs, PortProbe};
use anterm_ui_kit::window_chrome::{CONTENT_INSET, TILE_GLYPH_XS, TILE_SIZE_XS};

use super::{
    HEADING, INFO_ROW_H, META, META_MONO, PROC_ROW_H, ROW_FILL_RADIUS, ROW_INSET, RightPanelEvent,
    RightPanelView, SECTION_GAP, TEXT, TEXT_INSET, TEXT_MONO, action_strip, info_chip,
    panel_empty_view, reveal_label,
};

const PROC_INDENT: f32 = 14.;
const INFO_LABEL_MIN: f32 = 76.;
const INFO_LABEL_GAP: f32 = 9.;
const HEADING_TILE_OUTDENT: f32 = 5.;
const HEADING_TILE: f32 = 22.;
const HEADING_TILE_GLYPH: f32 = 12.;

const PROCS_POLL: std::time::Duration = std::time::Duration::from_millis(2000);
const PORT_WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(5000);
const AUTO_FORWARD_BURST: usize = 4;
const EMPTY: &str = "—";

pub fn forwards_port(m: &ManagedForward, port: u16) -> bool {
    m.enabled
        && m.kind == anterm_core::daemon::protocol::SshForwardKind::Local
        && m.target_port == port
        && anterm_core::daemon::protocol::PortEntry::reaches_loopback(&m.target_host)
}

#[derive(Clone)]
pub struct PaneForwardCtx {
    pub pane_id: u64,
    pub route: Option<anterm_ui_kit::daemon_rpc::ForwardRoute>,
    pub port_route: anterm_ui_kit::daemon_rpc::PortRoute,
    pub host: Option<anterm_ui_kit::host_ops::SharedHost>,
}

pub(super) enum InfoValue {
    Text(String),
    Path(String),
    Diff {
        added: u32,
        removed: u32,
        open: Option<(anterm_ui_kit::host_ops::HostId, PathBuf)>,
    },
}

pub(super) fn format_rtt(rtt: std::time::Duration) -> String {
    let ms = rtt.as_secs_f64() * 1000.;
    if ms < 1. {
        return "<1 ms".to_string();
    }
    let rounded = ms.round() as u64;
    if rounded < 1000 {
        return format!("{rounded} ms");
    }
    format!("{:.1} s", rtt.as_secs_f64())
}

pub(super) struct InfoRow {
    pub(super) label: &'static str,
    pub(super) value: InfoValue,
    pub(super) copy: Option<String>,
    pub(super) reveal: Option<PathBuf>,
}

impl InfoRow {
    pub(super) fn text(label: &'static str, value: String) -> Self {
        Self {
            label,
            value: InfoValue::Text(value),
            copy: None,
            reveal: None,
        }
    }

    pub(super) fn copyable(mut self) -> Self {
        self.copy = match &self.value {
            InfoValue::Text(v) | InfoValue::Path(v) => Some(v.clone()),
            _ => None,
        };
        self
    }

    pub(super) fn interactive(&self) -> bool {
        self.copy.is_some()
            || self.reveal.is_some()
            || matches!(self.value, InfoValue::Diff { open: Some(_), .. })
    }
}

fn info_label_column(rows: &[InfoRow], window: &mut Window, cx: &gpui::App) -> gpui::Pixels {
    let rem = window.rem_size().as_f32();
    let label_px = TEXT * rem;
    let min = INFO_LABEL_MIN / 16. * rem - INFO_LABEL_GAP - ROW_INSET;
    let max = 108. / 12. * label_px;
    let font = gpui::Font {
        family: cx.theme().font_family.clone(),
        features: Default::default(),
        fallbacks: None,
        weight: Default::default(),
        style: Default::default(),
    };
    let widest = rows
        .iter()
        .map(|row| {
            let k = row.label;
            window
                .text_system()
                .shape_line(
                    gpui::SharedString::from(k),
                    px(label_px),
                    &[gpui::TextRun {
                        len: k.len(),
                        font: font.clone(),
                        color: gpui::Hsla::default(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
                .width
                .as_f32()
        })
        .fold(min, f32::max);
    px(widest.clamp(min, max).ceil())
}

fn compact_path(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    anterm_ui_kit::path_display::abbreviate_home(&path.to_string_lossy(), home).into_owned()
}

impl RightPanelView {
    pub(super) fn render_panel_info(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = self.panel_title(t(L10nKey::PanelInfoTitle), None, None, window, cx);
        let mut rows: Vec<InfoRow> = Vec::new();
        let snap = self.ctx.info_snapshot.clone();
        let ctx = snap.as_ref().map(|s| s.forward_ctx.clone());
        let pane_id = snap.as_ref().map(|s| s.pane_id);

        if let Some(snap) = snap.as_ref() {
            if let Some(cwd) = &snap.cwd {
                rows.push(InfoRow {
                    label: t(L10nKey::PanelCwd),
                    value: InfoValue::Path(compact_path(cwd, snap.display_home.as_deref())),
                    copy: Some(match snap.is_local {
                        true => anterm_ui_kit::path_display::native_separators(cwd)
                            .display()
                            .to_string(),
                        false => cwd.display().to_string(),
                    }),
                    reveal: snap.is_local.then(|| cwd.clone()),
                });
            }
            let root = self
                .procs(pane_id)
                .and_then(|p| p.procs.first())
                .filter(|p| p.depth == 0)
                .map(|p| p.name.trim_start_matches('-').to_string())
                .filter(|name| !name.is_empty());
            let shell = match &snap.shell_program {
                Some(program) => anterm_core::shells::default_shell_name(Some(program)),
                None => root.unwrap_or_else(|| snap.default_shell_label.clone()),
            };
            rows.push(InfoRow::text(t(L10nKey::PanelShell), shell));
            if let Some(ssh_host) = &snap.ssh_host {
                rows.push(InfoRow::text(t(L10nKey::PanelSsh), ssh_host.clone()).copyable());
            }
            if self.link_host.is_some() {
                rows.push(InfoRow::text(
                    t(L10nKey::PanelLatency),
                    self.link_rtt
                        .map(format_rtt)
                        .unwrap_or_else(|| EMPTY.to_string()),
                ));
            }
            if let Some(git) = &snap.git_status {
                rows.push(InfoRow::text(t(L10nKey::PanelBranch), git.branch.clone()).copyable());
                rows.push(InfoRow {
                    label: t(L10nKey::PanelChangesRow),
                    value: InfoValue::Diff {
                        added: git.added,
                        removed: git.removed,
                        open: (git.added > 0 || git.removed > 0)
                            .then_some(snap.diff_target.clone())
                            .flatten(),
                    },
                    copy: None,
                    reveal: None,
                });
            }
        }

        if rows.is_empty() {
            return self.panel_scroll(
                panel_empty_view(
                    t(L10nKey::PanelNoSession),
                    Some(t(L10nKey::PanelNoSessionHint)),
                    cx,
                ),
                title,
            );
        }

        let label_w = info_label_column(&rows, window, cx);
        let mut list = v_flex().px(px(CONTENT_INSET));
        for (i, row) in rows.into_iter().enumerate() {
            list = list.child(self.info_row(i, row, label_w, cx));
        }

        let inner = v_flex()
            .pb(px(SECTION_GAP))
            .gap(px(SECTION_GAP))
            .child(
                v_flex()
                    .child(self.panel_subtitle(t(L10nKey::PanelSessionSubtitle), None, cx))
                    .child(list),
            )
            .children(self.procs_section(pane_id, cx))
            .children(self.ports_section(ctx.as_ref(), cx))
            .into_any_element();
        self.panel_scroll(inner, title)
    }

    fn info_row(
        &self,
        i: usize,
        row: InfoRow,
        label_w: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let mono = cx.theme().mono_font_family.clone();
        let id = gpui::SharedString::from(format!("panel-info-row-{i}"));
        let interactive = row.interactive();
        let tiles_wide = usize::from(row.reveal.is_some()) + usize::from(row.copy.is_some());
        let copy_label = match row.value {
            InfoValue::Path(_) => t(L10nKey::FileTreeContextCopyPath),
            _ => t(L10nKey::CmdCopy),
        };

        let a11y_name = format!(
            "{}, {}",
            row.label,
            match &row.value {
                InfoValue::Text(v) | InfoValue::Path(v) => v.clone(),
                InfoValue::Diff { added, removed, .. } => format!("+{added} −{removed}"),
            }
        );
        let value = match row.value {
            InfoValue::Path(v) => {
                let (head, leaf) = anterm_ui_kit::path_display::split_path_leaf(&v);
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(TEXT))
                    .text_color(cx.theme().foreground)
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(999.)
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(head),
                    )
                    .child(div().min_w_0().flex_shrink(1.).truncate().child(leaf))
                    .into_any_element()
            }
            InfoValue::Text(v) => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT))
                .text_color(cx.theme().foreground)
                .child(v)
                .into_any_element(),
            InfoValue::Diff {
                added,
                removed,
                open,
            } => {
                let clean = added == 0 && removed == 0;
                let counts = h_flex()
                    .flex_none()
                    .items_baseline()
                    .gap_1p5()
                    .text_size(rems(TEXT_MONO))
                    .font_family(mono.clone())
                    .when(clean, |this| {
                        this.child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(EMPTY.to_string()),
                        )
                    })
                    .when(added > 0, |this| {
                        this.child(
                            div()
                                .text_color(cx.theme().success)
                                .child(format!("+{added}")),
                        )
                    })
                    .when(removed > 0, |this| {
                        this.child(
                            div()
                                .text_color(cx.theme().danger)
                                .child(format!("−{removed}")),
                        )
                    });
                match open {
                    Some((host, cwd)) => counts
                        .id(("panel-info-diff", i))
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(move |_this, _, _window, cx| {
                            cx.stop_propagation();
                            cx.emit(RightPanelEvent::ToggleDiffOverlay {
                                host,
                                cwd: cwd.clone(),
                            });
                        }))
                        .into_any_element(),
                    None => counts.into_any_element(),
                }
            }
        };

        let value = h_flex()
            .flex_1()
            .min_w_0()
            .items_baseline()
            .when(tiles_wide > 0, |this| {
                this.pr(px(tiles_wide as f32 * (TILE_SIZE_XS + 1.) + 4.))
            })
            .child(value);

        let mut tiles = action_strip(&id, sf.hover);
        let mut has_tiles = false;
        if let Some(cwd) = row.reveal {
            has_tiles = true;
            tiles = tiles.child(
                self.info_tile(
                    "panel-info-reveal",
                    IconName::FolderOpen,
                    reveal_label(),
                    cx,
                )
                .on_click(move |_, _window, cx| {
                    cx.reveal_path(&anterm_ui_kit::path_display::native_separators(&cwd))
                }),
            );
        }
        if let Some(text) = row.copy {
            has_tiles = true;
            tiles = tiles.child(
                self.info_tile(("panel-info-copy", i), IconName::Copy, copy_label, cx)
                    .on_click(move |_, _window, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    }),
            );
        }
        let actions = has_tiles.then_some(tiles);

        h_flex()
            .id(id.clone())
            .role(gpui::Role::Label)
            .aria_label(a11y_name)
            .group(id)
            .relative()
            .items_center()
            .gap(px(INFO_LABEL_GAP))
            .h(px(INFO_ROW_H))
            .px(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .text_size(rems(TEXT))
            .when(interactive, |this| {
                this.hover(|s| s.bg(gpui::rgb(sf.hover)))
            })
            .child(
                div()
                    .flex_none()
                    .w(label_w)
                    .text_size(rems(TEXT))
                    .whitespace_nowrap()
                    .text_color(cx.theme().muted_foreground)
                    .child(row.label),
            )
            .child(value)
            .children(actions)
            .into_any_element()
    }

    fn info_tile(
        &self,
        id: impl Into<gpui::ElementId>,
        icon: IconName,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> Button {
        anterm_ui_kit::window_chrome::chrome_tile_sized(
            Button::new(id).icon(Icon::new(icon)),
            TILE_SIZE_XS,
            TILE_GLYPH_XS,
            false,
            cx,
        )
        .rounded_sm()
        .tooltip(tooltip)
    }

    fn panel_subtitle(
        &self,
        text: &str,
        trailing: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .flex_none()
            .h(px(INFO_ROW_H))
            .items_center()
            .justify_between()
            .pl(px(TEXT_INSET))
            .pr(px(if trailing.is_some() {
                TEXT_INSET - HEADING_TILE_OUTDENT
            } else {
                TEXT_INSET
            }))
            .child(
                div()
                    .text_size(rems(HEADING))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child(text.to_string()),
            )
            .when_some(trailing, |this, t| this.child(t))
            .into_any_element()
    }

    fn procs_section(&self, pane_id: Option<u64>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let procs = &self.procs(pane_id)?.procs;
        if procs.len() < 2 {
            return None;
        }
        let mono = cx.theme().mono_font_family.clone();
        let hover = gpui::rgb(
            cx.global::<anterm_ui_kit::presets::Surfaces>()
                .sidebar
                .hover,
        );
        let mut list = v_flex().px(px(CONTENT_INSET));
        for p in procs {
            let depth = f32::from(p.depth);
            list = list.child(
                h_flex()
                    .h(px(PROC_ROW_H))
                    .items_center()
                    .gap_1p5()
                    .px(px(ROW_INSET))
                    .rounded(ROW_FILL_RADIUS)
                    .hover(move |s| s.bg(hover))
                    .when(p.depth > 0, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .w(px(depth * PROC_INDENT))
                                .flex()
                                .justify_end()
                                .pr_0p5()
                                .text_size(rems(META))
                                .text_color(cx.theme().muted_foreground)
                                .child("└"),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rems(TEXT))
                            .text_color(cx.theme().foreground)
                            .when(p.foreground, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                            .child(p.name.clone()),
                    )
                    .child(crate::proc_usage::row_cells(
                        p,
                        &self.cpu,
                        mono.clone(),
                        cx,
                    )),
            );
        }
        let total = crate::proc_usage::total_row(procs, &self.cpu, mono, cx);
        let list = list.children(total);
        Some(
            v_flex()
                .child(self.panel_subtitle(t(L10nKey::PanelProcessesSubtitle), None, cx))
                .child(list)
                .into_any_element(),
        )
    }

    fn ports_section(
        &self,
        ctx: Option<&PaneForwardCtx>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ctx = ctx?;
        let pane_id = ctx.pane_id;
        let (ports, probe) = self
            .procs(Some(pane_id))
            .map(|p| (p.ports.clone(), p.probe.clone()))
            .unwrap_or_default();
        let forwards: Vec<ManagedForward> = self.forwards_panel.read(cx).managed_for(pane_id);
        let form_open = self.forwards_panel.read(cx).is_form_open_for(pane_id);
        if ports.is_empty() && forwards.is_empty() && ctx.route.is_none() && probe.is_ok() {
            return None;
        }

        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let mono = cx.theme().mono_font_family.clone();
        let openable = ctx.port_route != anterm_ui_kit::daemon_rpc::PortRoute::Blocked;
        let mut list = v_flex().px(px(CONTENT_INSET)).py(px(1.));
        let mut paired: Vec<u64> = Vec::new();

        for (i, p) in ports.iter().enumerate() {
            let forward = forwards.iter().find(|m| forwards_port(m, p.port));
            if let Some(f) = forward {
                paired.push(f.id);
            }
            let here = forward.map(|f| f.bind_port);
            let authority = here
                .map(|local| format!("127.0.0.1:{local}"))
                .unwrap_or_else(|| p.authority());
            let id = gpui::SharedString::from(format!("panel-port-{}-{}", p.port, p.pid));
            let mut tiles_wide = 1;
            let mut actions = action_strip(&id, sf.hover);
            if openable {
                tiles_wide += 1;
                let port = p.port;
                let direct = p.authority();
                actions = actions.child(
                    self.info_tile(
                        ("panel-port-open", i),
                        IconName::Globe,
                        t(L10nKey::PanelOpenInBrowser),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.open_pane_port(port, direct.clone(), cx)
                    })),
                );
            }
            actions = actions.child(
                self.info_tile(
                    ("panel-port-copy", i),
                    IconName::Copy,
                    t(L10nKey::CmdCopy),
                    cx,
                )
                .on_click({
                    let authority = authority.clone();
                    move |_, _window, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(authority.clone()));
                    }
                }),
            );
            if let Some(f) = forward {
                tiles_wide += 2;
                let forward_id = f.id;
                actions = actions.child(
                    anterm_ui_kit::window_chrome::chrome_tile_sized(
                        Button::new(("panel-port-switch-off", i))
                            .icon(Icon::empty().path("icons/power.svg")),
                        TILE_SIZE_XS,
                        TILE_GLYPH_XS,
                        false,
                        cx,
                    )
                    .rounded_sm()
                    .tooltip(t(L10nKey::ForwardTooltipTurnOff))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.forwards_panel.update(cx, |p, cx| {
                            p.set_forward_enabled(pane_id, forward_id, false, window, cx)
                        });
                    })),
                );
                actions = actions.child(
                    self.info_tile(
                        ("panel-port-unforward", i),
                        IconName::Close,
                        t(L10nKey::ForwardTooltipRemove),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.forwards_panel
                            .update(cx, |p, cx| p.remove_forward(pane_id, forward_id, cx));
                    })),
                );
            }
            list = list.child(
                h_flex()
                    .id(id.clone())
                    .group(id)
                    .relative()
                    .items_center()
                    .gap_2()
                    .h(px(PROC_ROW_H))
                    .px(px(ROW_INSET))
                    .rounded(ROW_FILL_RADIUS)
                    .hover(|s| s.bg(gpui::rgb(sf.hover)))
                    .child(info_chip(
                        &p.port.to_string(),
                        cx.theme().accent,
                        cx.theme().foreground,
                        &mono,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .pr(px(tiles_wide as f32 * (TILE_SIZE_XS + 1.) + 4.))
                            .text_size(rems(TEXT_MONO))
                            .font_family(mono.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(p.name.clone()),
                    )
                    .children(here.map(|local| {
                        div()
                            .flex_none()
                            .text_size(rems(META_MONO))
                            .font_family(mono.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("→ :{local}"))
                    }))
                    .child(actions),
            );
        }

        let extra_rows: Vec<_> = self.forwards_panel.update(cx, |panel, cx| {
            forwards
                .iter()
                .filter(|m| !paired.contains(&m.id))
                .map(|forward| panel.render_forward_row(forward, &mono, cx))
                .collect()
        });
        for row in extra_rows {
            list = list.child(row);
        }

        let add = ctx.route.is_some().then(|| {
            anterm_ui_kit::window_chrome::chrome_tile_sized(
                Button::new(("ssh-forward-add-toggle", pane_id))
                    .icon(Icon::empty().path("icons/plus.svg")),
                HEADING_TILE,
                HEADING_TILE_GLYPH,
                form_open,
                cx,
            )
            .rounded_sm()
            .tooltip(if form_open {
                t(L10nKey::Cancel)
            } else {
                t(L10nKey::ForwardTooltipAdd)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.forwards_panel
                    .update(cx, |p, cx| p.toggle_form(pane_id, window, cx));
            }))
            .into_any_element()
        });

        Some(
            v_flex()
                .child(self.panel_subtitle(t(L10nKey::PanelPortsSubtitle), add, cx))
                .when(
                    ports.is_empty() && forwards.is_empty() && !form_open,
                    |this| {
                        let key = match (self.procs_unsupported, &probe) {
                            (true, _) => L10nKey::PanelPortsUnsupported,
                            (false, PortProbe::Unavailable(_)) => L10nKey::PanelPortsProbeFailed,
                            (false, PortProbe::Restricted) => L10nKey::PanelPortsRestricted,
                            (false, PortProbe::Ok) => L10nKey::PanelPortsEmpty,
                        };
                        this.child(
                            div()
                                .px(px(TEXT_INSET))
                                .min_h(px(PROC_ROW_H))
                                .flex()
                                .items_center()
                                .text_size(rems(TEXT))
                                .text_color(cx.theme().muted_foreground)
                                .child(t(key)),
                        )
                    },
                )
                .child(list)
                .when(form_open, |this| this.child(self.forwards_panel.clone()))
                .into_any_element(),
        )
    }

    pub fn show_ssh_forwards(
        &mut self,
        ctx: Option<&PaneForwardCtx>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ctx) = ctx else {
            return;
        };
        if ctx.route.is_none() {
            return;
        }
        self.set_tab(RightPanelTab::Info, cx);
        if !self.forwards_panel.read(cx).is_form_open_for(ctx.pane_id) {
            self.forwards_panel
                .update(cx, |p, cx| p.toggle_form(ctx.pane_id, window, cx));
        }
    }

    pub fn open_pane_port(
        &mut self,
        port: u16,
        direct_authority: String,
        cx: &mut Context<Self>,
    ) {
        use anterm_ui_kit::daemon_rpc::PortRoute;
        match self.port_route {
            PortRoute::Blocked => {}
            PortRoute::Direct => cx.open_url(&format!("http://{direct_authority}")),
            PortRoute::Forward => {
                if let Some(f) = self
                    .forwards_panel
                    .read(cx)
                    .managed()
                    .iter()
                    .find(|m| forwards_port(m, port))
                {
                    cx.open_url(&format!("http://127.0.0.1:{}", f.bind_port));
                    return;
                }
                let Some(route) = self.procs_forwards.clone() else {
                    return;
                };
                let owner = route.owner_key();
                cx.spawn(async move |this, cx| {
                    let built = cx
                        .background_executor()
                        .spawn(async move { route.ensure_loopback("127.0.0.1", port) })
                        .await;
                    let _ = this.update_in(cx, |panel, window, cx| match built {
                        Ok(f) => {
                            panel
                                .auto_forwarded
                                .entry(owner)
                                .or_default()
                                .insert(port);
                            cx.open_url(&format!("http://127.0.0.1:{}", f.local_port));
                            if let Some(pane_id) = panel.procs_pane {
                                panel
                                    .forwards_panel
                                    .update(cx, |p, cx| p.refresh_managed(pane_id, cx));
                            }
                        }
                        Err(e) => window.push_notification(
                            anterm_ui_kit::host_ops::failure(
                                t_fmt(
                                    L10nKey::LoopbackForwardFailed,
                                    &[("port", &port.to_string()), ("error", &e.to_string())],
                                ),
                                &e,
                            ),
                            cx,
                        ),
                    });
                })
                .detach();
            }
        }
    }

    fn procs(&self, pane_id: Option<u64>) -> Option<&PaneProcs> {
        (pane_id.is_some() && self.procs_pane == pane_id).then_some(self.procs.as_ref())?
    }

    pub fn sync_port_watch(
        &mut self,
        ctx: Option<PaneForwardCtx>,
        cx: &mut Context<Self>,
    ) {
        self.port_route = ctx
            .as_ref()
            .map_or(anterm_ui_kit::daemon_rpc::PortRoute::Blocked, |c| c.port_route);
        let pane_id = ctx.as_ref().map(|c| c.pane_id);
        let host = ctx.as_ref().and_then(|c| c.host.clone());
        let route = ctx.and_then(|c| c.route);
        self.sync_procs(pane_id, route, host, cx);
    }

    fn procs_wanted(&self) -> bool {
        (self.visible && self.tab == RightPanelTab::Info) || self.watching_ports()
    }

    fn watching_ports(&self) -> bool {
        self.port_route == anterm_ui_kit::daemon_rpc::PortRoute::Forward
            && self.procs_forwards.is_some()
    }

    fn auto_forward_ports(&mut self, cx: &mut Context<Self>) {
        if !self.watching_ports() {
            return;
        }
        let Some(route) = self.procs_forwards.clone() else {
            return;
        };
        let listening: Vec<u16> = match self.procs.as_ref() {
            Some(procs) => procs
                .ports
                .iter()
                .filter(|p| anterm_core::daemon::protocol::PortEntry::reaches_loopback(&p.addr))
                .map(|p| p.port)
                .collect(),
            None => return,
        };
        let owner = route.owner_key();
        let seen = self.auto_forwarded.entry(owner).or_default();
        let mut fresh: Vec<u16> = Vec::new();
        for port in listening {
            if fresh.len() >= AUTO_FORWARD_BURST {
                break;
            }
            if seen.contains(&port) || fresh.contains(&port) {
                continue;
            }
            fresh.push(port);
        }
        if fresh.is_empty() {
            return;
        }
        seen.extend(fresh.iter().copied());
        cx.spawn(async move |this, cx| {
            let built = cx
                .background_executor()
                .spawn(async move {
                    fresh
                        .into_iter()
                        .map(|port| {
                            let local = route
                                .ensure_loopback("127.0.0.1", port)
                                .map(|f| f.local_port);
                            (port, local)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update_in(cx, |panel, window, cx| {
                for (port, local) in built {
                    match local {
                        Ok(local) => window.push_notification(
                            t_fmt(
                                L10nKey::PortAutoForwarded,
                                &[("port", &port.to_string()), ("local", &local.to_string())],
                            ),
                            cx,
                        ),
                        Err(e) => {
                            log::debug!("could not auto-forward :{port}: {e}");
                            if let Some(seen) = panel.auto_forwarded.get_mut(&owner) {
                                seen.remove(&port);
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_procs(
        &mut self,
        pane_id: Option<u64>,
        forwards: Option<anterm_ui_kit::daemon_rpc::ForwardRoute>,
        host: Option<anterm_ui_kit::host_ops::SharedHost>,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = pane_id else { return };
        self.procs_forwards = forwards.clone();
        self.procs_host = host.clone();
        let link_host = host.as_ref().map(|h| h.id());
        if self.link_host != link_host {
            self.link_host = link_host;
            self.link_rtt = None;
        }
        if self.procs_pane != Some(pane_id) {
            self.procs_pane = Some(pane_id);
            self.procs = None;
            self.forwards_panel.update(cx, |p, cx| p.clear_managed(cx));
            self.procs_gen += 1;
            self.procs_loading = false;
            self.procs_unsupported = false;
        }
        if !self.procs_loading && self.procs_wanted() {
            self.procs_loading = true;
            self.cpu = Default::default();
            let generation = self.procs_gen;
            self.spawn_procs_query(pane_id, generation, forwards, host, cx);
        }
    }

    fn spawn_procs_query(
        &mut self,
        pane_id: u64,
        generation: u64,
        forwards: Option<anterm_ui_kit::daemon_rpc::ForwardRoute>,
        host: Option<anterm_ui_kit::host_ops::SharedHost>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let route = forwards.clone();
            let want_link = this
                .read_with(cx, |panel, _| {
                    panel.visible && panel.tab == RightPanelTab::Info
                })
                .unwrap_or(false);
            let (procs, managed, link) = cx
                .background_executor()
                .spawn(async move {
                    let procs = match &host {
                        Some(host) => host.pane_procs(pane_id),
                        None => Some(anterm_ui_kit::daemon_rpc::query_procs(pane_id)),
                    };
                    let managed = route.map(|r| r.list()).unwrap_or_default();
                    let link = match (want_link, &host) {
                        (true, Some(host)) => host.link_rtt(),
                        _ => None,
                    };
                    (procs, managed, link)
                })
                .await;
            let keep_polling = this
                .update(cx, |panel, cx| {
                    if panel.procs_gen != generation {
                        return false;
                    }
                    panel.procs_unsupported = procs.is_none();
                    if let Some(procs) = procs {
                        panel.cpu.sample(&procs.procs, std::time::Instant::now());
                        panel.procs = Some(procs);
                    }
                    if forwards.is_some() {
                        panel
                            .forwards_panel
                            .update(cx, |p, cx| p.set_managed(managed, cx));
                    }
                    if want_link {
                        panel.link_rtt = link;
                    }
                    cx.notify();
                    let wanted = panel.procs_wanted();
                    if !wanted {
                        panel.procs_loading = false;
                    }
                    wanted
                })
                .unwrap_or(false);
            let _ = this.update(cx, |panel, cx| panel.auto_forward_ports(cx));
            if !keep_polling {
                return;
            }
            let gap = this
                .read_with(cx, |panel, _| {
                    match panel.visible && panel.tab == RightPanelTab::Info {
                        true => PROCS_POLL,
                        false => PORT_WATCH_POLL,
                    }
                })
                .unwrap_or(PORT_WATCH_POLL);
            cx.background_executor().timer(gap).await;
            let _ = this.update(cx, |panel, cx| {
                if panel.procs_gen != generation {
                    return;
                }
                if panel.procs_wanted() {
                    let forwards = panel.procs_forwards.clone();
                    let host = panel.procs_host.clone();
                    panel.spawn_procs_query(pane_id, generation, forwards, host, cx);
                } else {
                    panel.procs_loading = false;
                }
            });
        })
        .detach();
    }
}
