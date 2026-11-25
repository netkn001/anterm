//! Drawing the message composer: the overlay/docked container, attachment
//! chips, toolbar controls and pickers, `/` and `@` menu popup, Helix overlays,
//! and mouse/keyboard/action handlers.

use alacritty_terminal::vte::ansi::Rgb;
use gpui::{
    Context, ExternalPaths, Focusable as _, MouseButton, MouseDownEvent, SharedString, Window, div,
    prelude::*, px,
};
use gpui_kit::component::input::{self, Textarea};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, Size, h_flex};

use super::agents::{
    CLAUDE_MODELS, EFFORT_LEVELS, MenuItem, Trigger, alias_label, effort_label, mode_label,
    model_alias, model_label,
};
use super::{BOX_INSET, Composer, Picker, Presence};
use anterm_core::cli_agent::{AgentReadout, CLIAgent};
use crate::view::{GRID_PAD_X, GRID_PAD_Y, TerminalView};
use anterm_i18n::{L10nKey, t, t_fmt};

/// The design's type: 13.5px text in the box. gpui-component sizes an input's
/// text at 7/8 of a custom size, and pads it 8px across and 2px down.
const TEXT_PX: f32 = 13.5;
const INPUT_PAD_X: f32 = 8.;
const INPUT_PAD_Y: f32 = 2.;

/// The key the toolbar sends: Shift+Tab cycles the permission mode in every
/// agent that has one.
const BACK_TAB: &[u8] = b"\x1b[Z";

fn rgb_to_hsla(c: Rgb) -> gpui::Hsla {
    gpui::Rgba {
        r: c.r as f32 / 255.,
        g: c.g as f32 / 255.,
        b: c.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// A toolbar button: 28px tall, the box's own hover tint.
fn composer_tool_button(id: &'static str, ink: gpui::Hsla) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(28.))
        .gap(px(6.))
        .px(px(9.))
        .rounded(px(7.))
        .text_size(px(12.))
        .text_color(ink.opacity(0.5))
        .hover(move |s| s.bg(ink.opacity(0.05)))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
}

struct PickerButtonSpec {
    picker: Picker,
    id: &'static str,
    label: gpui::AnyElement,
    tip: L10nKey,
    title: L10nKey,
    rows: Vec<(&'static str, String, bool)>,
}

impl TerminalView {
    pub(crate) fn composer_presence(&self) -> Presence {
        self.presence()
    }

    pub(crate) fn composer_has_popup(&self, cx: &gpui::App) -> bool {
        self.composer_state
            .active
            .as_ref()
            .is_some_and(|c| c.picker.is_some())
            || self.composer_menu(cx).is_some()
    }

    /// The box, and whether it is docked (laid out under the grid) rather
    /// than laid over it.
    pub(crate) fn render_composer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(gpui::AnyElement, bool)> {
        let presence = self.presence();
        let covering = match presence {
            Presence::Covering(rows) => Some(rows),
            Presence::Docked => None,
            Presence::Hidden | Presence::SteppedAside => return None,
        };
        let frame = self.render_composer_frame(window, cx)?;
        // Measured while empty: the least the box needs of the pane, for
        // [`Self::composer_reserved_rows`].
        let measure = self.composer_state.active.as_ref().and_then(|c| {
            (covering.is_some() && c.attached.is_empty() && c.input.read(cx).value().is_empty())
                .then(|| c.frame_height.clone())
        });
        let frame = div()
            .w_full()
            .when_some(measure, |d, cell| {
                d.on_children_prepainted(move |bounds, window, _| {
                    let Some(b) = bounds.first() else { return };
                    if cell.get() != Some(b.size.height) {
                        cell.set(Some(b.size.height));
                        window.refresh();
                    }
                })
            })
            .child(frame);
        let reserved = self.composer_reserved_rows();
        let element = match covering {
            Some(rows) => div()
                .relative()
                .w_full()
                .min_h(
                    self.render.line_height * (rows + reserved) as f32
                        + self.render.grid_slack
                        + px(GRID_PAD_Y),
                )
                .flex()
                .flex_col()
                .justify_end()
                .px(px(BOX_INSET))
                .pb(px(BOX_INSET))
                .bg(cx.theme().background)
                // Over the grid's own cells, the colour the agent paints
                // them in; the pane's padding around them keeps the theme's.
                .when_some(
                    self.composer_state.active.as_ref().and_then(|c| c.backdrop),
                    |d, (backdrop, width)| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left(px(GRID_PAD_X))
                                .w(width)
                                .bottom(
                                    self.render.line_height * reserved as f32
                                        + self.render.grid_slack
                                        + px(GRID_PAD_Y),
                                )
                                .bg(rgb_to_hsla(backdrop)),
                        )
                    },
                )
                .occlude()
                // Around the box is the terminal as far as a click goes: it
                // takes the keyboard out of the box, as a click on the rows
                // above does. The layer only hides what is under it.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus_handle, cx);
                    }),
                )
                .child(frame)
                .into_any_element(),
            None => div()
                .flex_none()
                .w_full()
                .pt(px(8.))
                // The pane's own padding is already between it and the edges.
                .px(px(BOX_INSET - GRID_PAD_X))
                .pb(px(BOX_INSET - GRID_PAD_Y))
                .child(frame)
                .into_any_element(),
        };
        Some((element, covering.is_none()))
    }

    fn render_composer_frame(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let c = self.composer_state.active.as_ref()?;
        let agent = self.agent()?;
        let theme = cx.theme();
        let focused = c.input.read(cx).focus_handle(cx).is_focused(window);
        let ink = theme.foreground;
        let background = theme.background;
        let hairline = anterm_ui_kit::theme::hairline(window);
        let menu = self.composer_menu(cx);

        let chips = self.render_composer_chips(&c.attached, cx);
        let toolbar = self.render_composer_toolbar(c, agent, window, cx);
        let popup = menu
            .as_ref()
            .map(|(trigger, items)| self.render_composer_menu_popup(c, trigger, items, cx));
        let helix_block_cursor = self.render_composer_helix_block_cursor(c, focused, cx);
        let container = self.attach_composer_handlers(
            div().id("composer").relative().w_full(),
            c.picker.is_some(),
            menu.is_some(),
            cx,
        );

        Some(
            container
                .children(popup.map(|p| gpui::deferred(p).with_priority(1)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        // Opaque: under it can be whatever colour the agent
                        // paints its screen in.
                        .bg(background.blend(ink.opacity(0.035)))
                        .relative()
                        .border_t(hairline)
                        .border_b(hairline)
                        .border_l(hairline)
                        .border_r(hairline)
                        .border_color(ink.opacity(0.15))
                        .children(chips)
                        .child(
                            div()
                                .min_h(px(44.))
                                .px(px(14. - INPUT_PAD_X))
                                .pt(px(12. - INPUT_PAD_Y))
                                .pb(px(4. - INPUT_PAD_Y))
                                .child(
                                    Textarea::new(&c.input)
                                        .appearance(false)
                                        .with_size(Size::Size(px(TEXT_PX / 0.875))),
                                )
                                .children(helix_block_cursor),
                        )
                        .child(toolbar)
                        // Focus and a drag over the box thicken the ring. Drawn
                        // as a layer over the hairline, not as a wider border,
                        // so the box's contents stay where they are.
                        .child(
                            // Laid out inside the hairline, so pulled out over it.
                            div()
                                .absolute()
                                .top(-hairline)
                                .bottom(-hairline)
                                .left(-hairline)
                                .right(-hairline)
                                .rounded(px(12.))
                                .border_1()
                                .border_color(match focused {
                                    true => ink.opacity(0.22),
                                    false => gpui::transparent_black(),
                                })
                                .drag_over::<ExternalPaths>(move |s, _, _, _| {
                                    s.border_color(ink.opacity(0.48))
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_composer_chips(&self, attached: &[String], cx: &Context<Self>) -> Option<gpui::Div> {
        if attached.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let ink = theme.foreground;
        let muted = theme.muted_foreground;
        Some(
            h_flex()
                .flex_wrap()
                .gap(px(6.))
                .px(px(10.))
                .pt(px(10.))
                .children(attached.iter().enumerate().map(|(i, path)| {
                    let name = path
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(path)
                        .trim_matches('\'')
                        .to_string();
                    let image = ["png", "jpg", "jpeg", "gif", "webp"]
                        .iter()
                        .any(|ext| name.to_lowercase().ends_with(&format!(".{ext}")));
                    let glyph = match image {
                        true => div()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .bg(ink.opacity(0.08))
                            .child(
                                gpui::svg()
                                    .path("icons/image.svg")
                                    .size(px(11.))
                                    .text_color(muted),
                            )
                            .into_any_element(),
                        false => gpui::svg()
                            .path("icons/file.svg")
                            .mx(px(2.))
                            .size(px(12.))
                            .text_color(muted)
                            .into_any_element(),
                    };
                    h_flex()
                        .id(("composer-chip", i))
                        .h(px(28.))
                        .max_w(px(260.))
                        .gap(px(7.))
                        .pl(px(6.))
                        .pr(px(4.))
                        .rounded(px(7.))
                        .bg(ink.opacity(0.05))
                        .text_size(px(12.))
                        .tooltip({
                            let path: SharedString = path.clone().into();
                            move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                        })
                        .child(glyph)
                        .child(div().min_w_0().truncate().child(name))
                        .child(
                            div()
                                .id(("composer-chip-remove", i))
                                .size(px(18.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .hover(|s| s.bg(ink.opacity(0.07)))
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, _w, cx| this.detach(i, cx)))
                                .child(Icon::new(IconName::Close).size(px(9.)).text_color(muted)),
                        )
                })),
        )
    }

    /// A toolbar list, opened over its own button.
    fn render_composer_picker_popup(
        &self,
        picker: Picker,
        title: L10nKey,
        rows: Vec<(&'static str, String, bool)>,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let ink = cx.theme().foreground;
        anterm_ui_kit::theme::floating_surface(div(), cx)
            .rounded(px(10.))
            .absolute()
            .left_0()
            .bottom_full()
            .mb(px(8.))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .min_w(px(180.))
            .p(px(5.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .text_size(px(13.))
            .text_color(ink)
            .occlude()
            .child(
                div()
                    .h(px(24.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(ink.opacity(0.4))
                    .child(t(title)),
            )
            .children(rows.into_iter().enumerate().map(|(i, (value, label, on))| {
                h_flex()
                    .id(("composer-picker-row", i))
                    .h(px(30.))
                    .px(px(9.))
                    .gap(px(16.))
                    .rounded(px(6.))
                    .hover(move |s| s.bg(ink.opacity(0.05)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.pick(picker, value, window, cx);
                        }),
                    )
                    .child(div().flex_1().whitespace_nowrap().child(label))
                    .child(div().w(px(12.)).when(on, |s| {
                        s.child(
                            Icon::new(IconName::Check)
                                .size(px(12.))
                                .text_color(ink.opacity(0.6)),
                        )
                    }))
            }))
    }

    /// A toolbar button that opens a list over itself.
    fn render_composer_picker_button(
        &self,
        c: &Composer,
        spec: PickerButtonSpec,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let ink = cx.theme().foreground;
        let picker = spec.picker;
        let tip = spec.tip;
        let open = c.picker == Some(picker);
        div()
            .relative()
            .flex_none()
            // A click anywhere else closes the list, as a menu's would.
            // The button and the list stop their own clicks short of this.
            .when(open, |s| {
                s.on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                    if let Some(c) = this.composer_state.active.as_mut() {
                        c.picker = None;
                    }
                    cx.notify();
                }))
            })
            .child(
                composer_tool_button(spec.id, ink)
                    .gap(px(5.))
                    .when(open, |s| s.bg(ink.opacity(0.05)))
                    .when(!open, |s| {
                        s.tooltip(move |window, cx| Tooltip::new(t(tip)).build(window, cx))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_picker(picker, window, cx)
                    }))
                    .child(spec.label),
            )
            // Painted after everything else, so the box's ring and whatever
            // else is under the list stays under it.
            .when(open, |s| {
                s.child(
                    gpui::deferred(
                        self.render_composer_picker_popup(picker, spec.title, spec.rows, cx),
                    )
                    .with_priority(1),
                )
            })
    }

    fn render_composer_model_picker(
        &self,
        c: &Composer,
        readout: &AgentReadout,
        turns: u64,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let ink = cx.theme().foreground;
        let picked = c
            .model
            .as_ref()
            .filter(|(_, then)| *then == turns)
            .map(|(alias, _)| *alias);
        let current = picked.or_else(|| readout.model.as_deref().and_then(model_alias));
        let name = match (picked, readout.model.as_deref()) {
            (Some(alias), _) => alias_label(alias),
            (None, Some(model)) => model_label(model),
            (None, None) => t(L10nKey::ComposerModel).to_string(),
        };
        let label = h_flex()
            .gap(px(5.))
            .child(name)
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(px(8.))
                    .text_color(ink.opacity(0.4)),
            )
            .into_any_element();
        let rows = CLAUDE_MODELS
            .iter()
            .map(|&alias| (alias, alias_label(alias), current == Some(alias)))
            .collect();
        self.render_composer_picker_button(
            c,
            PickerButtonSpec {
                picker: Picker::Model,
                id: "composer-model",
                label,
                tip: L10nKey::ComposerModelTip,
                title: L10nKey::ComposerModel,
                rows,
            },
            cx,
        )
    }

    fn render_composer_effort_picker(
        &self,
        c: &Composer,
        readout: &AgentReadout,
        turns: u64,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let ink = cx.theme().foreground;
        let current_effort = c
            .effort
            .as_ref()
            .filter(|(_, then)| *then == turns)
            .map(|(level, _)| level.to_string())
            .or_else(|| readout.effort.clone())
            // A model that takes no effort level has none to show.
            .filter(|level| !level.is_empty());
        // The level alone, as the model button shows the model alone;
        // the tooltip and the list's title say what it is.
        let name = match current_effort.as_deref() {
            Some(level) => effort_label(level),
            None => t(L10nKey::ComposerEffort).to_string(),
        };
        let label = h_flex()
            .gap(px(5.))
            .child(name)
            .child(
                Icon::new(IconName::ChevronDown)
                    .size(px(8.))
                    .text_color(ink.opacity(0.4)),
            )
            .into_any_element();
        let rows = EFFORT_LEVELS
            .iter()
            .map(|&level| {
                let on = current_effort.as_deref() == Some(level);
                (level, effort_label(level), on)
            })
            .collect();
        self.render_composer_picker_button(
            c,
            PickerButtonSpec {
                picker: Picker::Effort,
                id: "composer-effort",
                label,
                tip: L10nKey::ComposerEffortTip,
                title: L10nKey::ComposerEffort,
                rows,
            },
            cx,
        )
    }

    fn render_composer_toolbar(
        &self,
        c: &Composer,
        agent: CLIAgent,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let (readout, turns) = self
            .agent_session()
            .map(|s| (s.readout, s.turns))
            .unwrap_or_default();
        let theme = cx.theme();
        let ink = theme.foreground;
        let amber = theme.warning;
        let can_send = self.composer_can_send(cx);

        let attach = composer_tool_button("composer-attach", ink)
            .w(px(28.))
            .px_0()
            .justify_center()
            .tooltip(|window, cx| Tooltip::new(t(L10nKey::ComposerAttach)).build(window, cx))
            .on_click(cx.listener(|this, _, window, cx| this.pick_attachments(window, cx)))
            .child(
                Icon::new(IconName::Plus)
                    .size(px(12.))
                    .text_color(ink.opacity(0.5)),
            );

        let claude = agent == CLIAgent::Claude;
        let mode = claude.then(|| {
            // The status row under the input says it as it changes; the
            // hooks only say it at the next event.
            let mode = c
                .area
                .as_ref()
                .and_then(|a| a.mode)
                .map(str::to_string)
                .or_else(|| readout.permission_mode.clone())
                .unwrap_or_else(|| "default".into());
            let bypass = mode == "bypassPermissions";
            composer_tool_button("composer-mode", ink)
                .when(bypass, |s| s.text_color(amber))
                .tooltip(|window, cx| Tooltip::new(t(L10nKey::ComposerModeTip)).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.toolbar_key(BACK_TAB, window, cx)))
                .when(bypass, |s| {
                    s.child(div().size(px(5.)).rounded_full().bg(amber))
                })
                .child(mode_label(&mode))
        });
        let divider = claude.then(|| {
            div()
                .flex_none()
                .w(anterm_ui_kit::theme::hairline(window))
                .h(px(14.))
                .mx(px(4.))
                .bg(ink.opacity(0.15))
        });
        let model = claude.then(|| self.render_composer_model_picker(c, &readout, turns, cx));
        let effort = claude.then(|| self.render_composer_effort_picker(c, &readout, turns, cx));

        let send = div()
            .id("composer-send")
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(match can_send {
                true => ink,
                false => ink.opacity(0.07),
            })
            .tooltip(|window, cx| Tooltip::new(t(L10nKey::ComposerSendTip)).build(window, cx))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, window, cx| this.submit_composer(window, cx)))
            .child(
                Icon::new(IconName::ArrowUp)
                    .size(px(12.))
                    .text_color(match can_send {
                        true => theme.background,
                        false => ink.opacity(0.35),
                    }),
            );

        let asking = self.agent_is_asking().then(|| {
            div()
                .min_w_0()
                .truncate()
                .px(px(6.))
                .text_size(px(12.))
                .text_color(amber)
                .child(t_fmt(
                    L10nKey::ComposerAgentAsking,
                    &[("agent", agent.display_name())],
                ))
        });

        let helix_badge = self.render_composer_helix_badge(c, cx);

        h_flex()
            .h(px(40.))
            .px(px(6.))
            .gap(px(2.))
            .child(attach)
            .children(mode)
            .children(divider)
            .children(model)
            .children(effort)
            .children(asking)
            .child(div().flex_1())
            .children(helix_badge)
            .child(send)
    }

    fn render_composer_menu_popup(
        &self,
        c: &Composer,
        trigger: &Trigger,
        items: &[MenuItem],
        cx: &Context<Self>,
    ) -> gpui::Div {
        let ink = cx.theme().foreground;
        let highlighted = c.highlighted.min(items.len() - 1);
        let title = match trigger.sigil {
            '/' => t(L10nKey::ComposerMenuCommands),
            _ => t(L10nKey::ComposerMenuFiles),
        };
        anterm_ui_kit::theme::floating_surface(div(), cx)
            .rounded(px(10.))
            .absolute()
            .left_0()
            .bottom_full()
            .mb(px(6.))
            .w(px(380.))
            .max_w_full()
            .p(px(5.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .text_size(px(13.))
            .child(
                div()
                    .h(px(24.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(ink.opacity(0.4))
                    .child(title),
            )
            .children(items.iter().enumerate().map(|(i, item)| {
                h_flex()
                    .id(("composer-menu", i))
                    .h(px(30.))
                    .px(px(9.))
                    .gap(px(10.))
                    .rounded(px(6.))
                    .when(i == highlighted, |s| s.bg(ink.opacity(0.05)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.pick_menu_item(i, window, cx);
                        }),
                    )
                    .when(item.is_file, |s| {
                        s.child(
                            gpui::svg()
                                .path("icons/file.svg")
                                .size(px(12.))
                                .text_color(ink.opacity(0.4)),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .font_family(self.render.font.family.clone())
                            .text_size(px(12.))
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_right()
                            .text_size(px(12.))
                            .text_color(ink.opacity(0.4))
                            .child(item.detail.clone()),
                    )
            }))
    }

    fn render_composer_helix_badge(&self, c: &Composer, cx: &Context<Self>) -> Option<gpui::Div> {
        self.prompt.helix_mode.then(|| {
            use anterm_editor::helix::HelixMode;
            let theme = cx.theme();
            let ink = theme.foreground;
            let mode_bg = match c.helix.mode {
                HelixMode::Normal => theme.blue,
                HelixMode::Select => theme.magenta,
                HelixMode::Insert => theme.green,
            };
            let pending = c.helix.pending_display();
            h_flex()
                .items_center()
                .gap(px(5.))
                .mr(px(4.))
                .when_some(pending, |d, seq| {
                    d.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(ink.opacity(0.07))
                            .font_family(self.render.font.family.clone())
                            .text_size(px(11.))
                            .text_color(ink)
                            .child(seq),
                    )
                })
                .child(
                    div()
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(mode_bg.opacity(0.2))
                        .font_family(self.render.font.family.clone())
                        .text_size(px(10.5))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(mode_bg)
                        .child(c.helix.mode.badge()),
                )
        })
    }

    fn render_composer_helix_block_cursor(
        &self,
        c: &Composer,
        focused: bool,
        cx: &Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !(self.prompt.helix_mode && focused && c.helix.mode.is_block_cursor()) {
            return None;
        }
        let state = c.input.read(cx);
        let text = state.text();
        let len = text.len();
        let raw_head = if c.helix.anchor == c.helix.head && state.selected_range().is_empty() {
            state.cursor().min(len)
        } else {
            c.helix.head.min(len)
        };
        let range = anterm_ui_kit::helix_bridge::block_cursor_byte_range(text, raw_head);
        let bounds = state.range_to_bounds(&range)?;
        let cell_w = bounds.size.width.max(self.render.cell_width.max(px(7.5)));
        let cell_h = bounds.size.height.max(px(14.));
        let theme = cx.theme();
        let fill_col = theme.caret.opacity(0.42);
        let border_col = theme.caret;
        Some(
            gpui::anchored()
                .position(bounds.origin)
                .child(
                    div()
                        .w(cell_w)
                        .h(cell_h)
                        .bg(fill_col)
                        .border_1()
                        .border_color(border_col),
                )
                .into_any_element(),
        )
    }

    fn attach_composer_handlers(
        &self,
        el: gpui::Stateful<gpui::Div>,
        picker_open: bool,
        menu_open: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        el.capture_key_down(
            cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                if !this.prompt.helix_mode || !this.composer_state.focused {
                    return;
                }
                if this.composer_state.active.as_ref().is_some_and(|c| c.picker.is_some())
                    || this.composer_menu(cx).is_some()
                {
                    return;
                }
                let mode = this
                    .composer_state
                    .active
                    .as_ref()
                    .map(|c| c.helix.mode)
                    .unwrap_or(anterm_editor::helix::HelixMode::Insert);
                if mode == anterm_editor::helix::HelixMode::Insert {
                    let m = &ev.keystroke.modifiers;
                    if !m.control && !m.alt && !m.platform && ev.keystroke.key == "escape" {
                        if let Some(c) = this.composer_state.active.as_mut() {
                            let (cursor, len) = {
                                let state = c.input.read(cx);
                                (state.cursor(), state.value().len())
                            };
                            let pos = cursor.min(len);
                            c.helix.mode = anterm_editor::helix::HelixMode::Normal;
                            c.helix.clear_pending();
                            c.helix.set_selection(pos, pos);
                        }
                        cx.stop_propagation();
                        window.prevent_default();
                        cx.notify();
                    }
                    return;
                }
                if this.composer_handle_helix_key(&ev.keystroke, window, cx) {
                    cx.stop_propagation();
                    window.prevent_default();
                }
            }),
        )
        // The terminal surface this sits in focuses the grid on any
        // click and opens its own context menu on a right one. Neither
        // is right for a click on the box.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                window.prevent_default();
                // Not the layer under it, which takes a click as one
                // on the terminal.
                cx.stop_propagation();
                // A click on the frame around the text, not only on
                // the text, is a click on the box.
                if !this.composer_state.focused {
                    this.focus_composer(window, cx);
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, _: &MouseDownEvent, _w, cx| {
                this.pointer.context_menu_allowed = false;
                cx.stop_propagation();
            }),
        )
        .on_scroll(|_, _, cx| cx.stop_propagation())
        // Files dropped on the box are attachments, not words for the
        // terminal's line; the surface's own drop handler would focus
        // the grid first.
        .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
            cx.stop_propagation();
            this.focus_composer(window, cx);
            this.drop_files(paths, cx);
        }))
        .on_drop(cx.listener(
            |this, drag: &anterm_ui_kit::panel_tokens::RemotePathDrag, window, cx| {
                cx.stop_propagation();
                this.focus_composer(window, cx);
                this.drop_remote_path(drag, cx);
            },
        ))
        .capture_action(cx.listener(|this, _: &input::Paste, _w, cx| {
            // Text pastes are the box's own. A copied file or a
            // screenshot has no text to paste, and the terminal
            // already knows how to turn those into a path.
            let Some(item) = cx.read_from_clipboard() else {
                return;
            };
            if item.text().is_some() && !crate::view::clipboard_has_paths(&item) {
                return;
            }
            cx.stop_propagation();
            this.paste_from_clipboard(cx);
        }))
        // Shift+Tab is the agent's: it cycles the permission mode.
        .capture_action(cx.listener(|this, _: &input::OutdentInline, _w, cx| {
            cx.stop_propagation();
            this.send_to_pty(BACK_TAB, cx);
        }))
        .capture_action(cx.listener(|this, _: &input::Backspace, _w, cx| {
            let Some(c) = this.composer_state.active.as_ref() else {
                return;
            };
            let state = c.input.read(cx);
            if state.cursor() != 0 || !state.selected_range().is_empty() {
                return;
            }
            if let Some(last) = c.attached.len().checked_sub(1) {
                cx.stop_propagation();
                this.detach(last, cx);
            }
        }))
        .when(picker_open, |el| {
            el.capture_action(cx.listener(|this, _: &input::Escape, _w, cx| {
                cx.stop_propagation();
                if let Some(c) = this.composer_state.active.as_mut() {
                    c.picker = None;
                }
                cx.notify();
            }))
        })
        .when(menu_open, |el| {
            el.capture_action(cx.listener(|this, _: &input::MoveUp, _w, cx| {
                if this.step_menu(false, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::MoveDown, _w, cx| {
                if this.step_menu(true, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, action: &input::Enter, window, cx| {
                if action.shift {
                    return;
                }
                cx.stop_propagation();
                let at = this.composer_state.active.as_ref().map_or(0, |c| c.highlighted);
                this.pick_menu_item(at, window, cx);
            }))
            .capture_action(cx.listener(|this, _: &input::IndentInline, window, cx| {
                cx.stop_propagation();
                let at = this.composer_state.active.as_ref().map_or(0, |c| c.highlighted);
                this.pick_menu_item(at, window, cx);
            }))
            .capture_action(cx.listener(|this, _: &input::Escape, _w, cx| {
                cx.stop_propagation();
                if let Some(c) = this.composer_state.active.as_mut() {
                    c.dismissed = Some(c.input.read(cx).value().to_string());
                }
                cx.notify();
            }))
        })
    }
}
