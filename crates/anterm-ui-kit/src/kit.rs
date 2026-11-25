//! The settings page's own controls.
//!
//! Settings is drawn to one design, and that design is quieter than the rest
//! of the app: ink at a handful of fixed strengths instead of the theme's named
//! roles, hairline rings instead of borders, and controls that sit on the page
//! rather than on cards. Everything here derives from the active theme's
//! foreground and background, so the page follows every theme — light, dark,
//! and the tinted ones in between — without a palette of its own.
//!
//! Sizes are in rems off a 16px base, written as the pixel sizes the design
//! calls for, so the page still grows with the interface font.

use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement,
    MouseButton, Rems, SharedString, Stateful, Window, div, point, prelude::*, px, rems,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch as KitSwitch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, Size, h_flex, v_flex};

/// A size from the design, in pixels at the default 16px interface font.
pub fn fs(px_at_16: f32) -> Rems {
    rems(px_at_16 / 16.)
}

/// The page's ink at the strengths the design uses, plus the few surfaces it
/// lifts off the page.
#[derive(Clone, Copy)]
pub struct Tk {
    pub dark: bool,
    pub fg: Hsla,
    pub page: Hsla,
    pub k04: Hsla,
    pub k05: Hsla,
    pub k06: Hsla,
    pub k08: Hsla,
    pub k15: Hsla,
    pub k3: Hsla,
    pub k35: Hsla,
    pub k4: Hsla,
    pub k45: Hsla,
    pub k5: Hsla,
    pub k6: Hsla,
    /// A group heading: quieter than a label, but still read at a glance.
    pub heading: Hsla,
    /// A nav item at rest.
    pub nav: Hsla,
    /// A raised button's fill.
    pub btn: Hsla,
    /// A popover's fill.
    pub menu: Hsla,
    /// A switch or slider knob at rest.
    pub knob: Hsla,
    /// The chosen cell of a segmented control.
    pub chip: Hsla,
    pub warn: Hsla,
    pub warn_text: Hsla,
    pub danger: Hsla,
    pub ok: Hsla,
}

fn lift(c: Hsla, by: f32) -> Hsla {
    Hsla {
        l: (c.l + by).clamp(0., 1.),
        ..c
    }
}

impl Tk {
    pub fn of(cx: &App) -> Self {
        let theme = cx.theme();
        let dark = theme.is_dark();
        let fg = theme.foreground;
        let page = theme.background;
        let a = |light: f32, dark_a: f32| fg.opacity(if dark { dark_a } else { light });
        // Light controls sit a step above the page in the theme's own hue, so
        // a cream theme gets cream fields and a white one stays white. Opaque:
        // the page carries the window's opacity, a control should not.
        let raised = Hsla {
            a: 1.,
            ..lift(page, 0.035)
        };
        Self {
            dark,
            fg,
            page,
            k04: a(0.04, 0.055),
            k05: a(0.05, 0.07),
            k06: a(0.06, 0.08),
            k08: a(0.08, 0.08),
            k15: a(0.15, 0.16),
            k3: a(0.3, 0.3),
            k35: a(0.35, 0.36),
            k4: a(0.4, 0.45),
            k45: a(0.45, 0.5),
            k5: a(0.5, 0.55),
            k6: a(0.6, 0.65),
            heading: a(0.9, 0.9),
            nav: a(0.82, 0.8),
            btn: if dark { fg.opacity(0.1) } else { raised },
            menu: if dark { lift(page, 0.06) } else { raised },
            knob: if dark { fg } else { raised },
            chip: if dark { fg.opacity(0.16) } else { raised },
            // Semantic status roles come from `cx.theme()` (which already
            // resolves contrast-checked tokens for every dark/light preset).
            warn: theme.warning,
            warn_text: theme.warning,
            danger: theme.danger,
            ok: theme.success,
        }
    }

    /// The hairline ring and the faint drop a raised control sits on.
    pub fn raised(&self) -> Vec<BoxShadow> {
        if self.dark {
            vec![ring(self.fg.opacity(0.1), 0.5, false)]
        } else {
            vec![
                ring(gpui::black().opacity(0.08), 0.5, false),
                drop(gpui::black().opacity(0.06), 1., 2., 0.),
            ]
        }
    }

    /// The same, a step firmer, for a hovered raised control.
    pub fn raised_hover(&self) -> Vec<BoxShadow> {
        if self.dark {
            vec![ring(self.fg.opacity(0.2), 0.5, false)]
        } else {
            vec![
                ring(gpui::black().opacity(0.16), 0.5, false),
                drop(gpui::black().opacity(0.08), 1., 2., 0.),
            ]
        }
    }

    pub fn popover(&self) -> Vec<BoxShadow> {
        if self.dark {
            vec![
                ring(self.fg.opacity(0.12), 0.5, false),
                drop(gpui::black().opacity(0.6), 16., 40., -8.),
            ]
        } else {
            vec![
                ring(gpui::black().opacity(0.12), 0.5, false),
                drop(gpui::black().opacity(0.22), 16., 40., -8.),
            ]
        }
    }

    pub fn mono(cx: &App) -> SharedString {
        cx.theme().mono_font_family.clone()
    }
}

pub fn ring(color: Hsla, width: f32, inset: bool) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(width),
        inset,
    }
}

pub fn drop(color: Hsla, y: f32, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type ToggleHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// An on/off switch backed by `gpui_kit::component::switch::Switch` for native
/// keyboard activation (Space/Enter), focus rings, spring thumb motion, and
/// accessibility semantics while adopting the active theme's foreground ink.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    on: bool,
    disabled: bool,
    small: bool,
    accessibility_label: Option<SharedString>,
    handler: Option<ToggleHandler>,
}

pub fn switch(id: impl Into<ElementId>) -> Switch {
    Switch {
        id: id.into(),
        on: false,
        disabled: false,
        small: false,
        accessibility_label: None,
        handler: None,
    }
}

impl Switch {
    pub fn checked(mut self, on: bool) -> Self {
        self.on = on;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }
    pub fn on_click(mut self, f: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tk = Tk::of(cx);
        let size = if self.small {
            Size::Small
        } else {
            Size::Medium
        };
        KitSwitch::new(self.id)
            .checked(self.on)
            .disabled(self.disabled)
            .with_size(size)
            .color(tk.fg)
            .when_some(self.accessibility_label, |sw, label| {
                sw.accessibility_label(label)
            })
            .when_some(self.handler, |sw, handler| {
                sw.on_change(move |next, window, cx| handler(next, window, cx))
            })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    /// A raised button on the page.
    Secondary,
    /// Solid ink: the one action a block exists for.
    Primary,
    /// Quiet borderless in-app command (rendered with `Role::Button`, never
    /// link semantics per GPUI Kit Design Guides).
    Link,
    /// Quiet destructive button in the theme's `danger` ink.
    Danger,
}

#[derive(IntoElement)]
pub struct Btn {
    id: ElementId,
    label: SharedString,
    kind: BtnKind,
    disabled: bool,
    dimmed: bool,
    icon: Option<SharedString>,
    size: Option<Rems>,
    handler: Option<ClickHandler>,
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: BtnKind,
) -> Btn {
    Btn {
        id: id.into(),
        label: label.into(),
        kind,
        disabled: false,
        dimmed: false,
        icon: None,
        size: None,
        handler: None,
    }
}

impl Btn {
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Clickable, but drawn at rest — a Save with nothing to save yet.
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }
    pub fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.icon = Some(path.into());
        self
    }
    pub fn text_size(mut self, size: Rems) -> Self {
        self.size = Some(size);
        self
    }
    pub fn on_click(
        mut self,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Btn {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tk = Tk::of(cx);
        let kind = self.kind;
        // A primary with nothing to do yet — disabled, or dimmed with nothing
        // to save — falls to the faint fill with secondary text so it never
        // competes with active commitments.
        let resting_primary = kind == BtnKind::Primary && (self.disabled || self.dimmed);
        let has_icon = self.icon.is_some();
        let mut btn = Button::new(self.id)
            .small()
            .disabled(self.disabled)
            .label(self.label)
            .when_some(self.icon, |b, path| {
                b.icon(Icon::empty().path(path).size(fs(10.)).text_color(tk.k6))
            })
            .when_some(self.handler, |b, handler| {
                b.on_click(move |ev, window, cx| handler(ev, window, cx))
            });

        btn = match kind {
            BtnKind::Secondary => btn
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(tk.btn)
                        .foreground(tk.fg)
                        .hover(tk.k05)
                        .active(tk.k08),
                )
                .rounded(ButtonRounded::Medium)
                .h(fs(26.))
                .px(fs(12.))
                .when(has_icon, |b| b.pl(fs(9.)).pr(fs(11.)))
                .bg(tk.btn)
                .shadow(tk.raised())
                .text_size(self.size.unwrap_or(fs(12.5))),
            BtnKind::Primary => {
                let (bg, fg, hover_bg) = if resting_primary {
                    (tk.k05, tk.k45, tk.k08)
                } else {
                    (tk.fg, tk.page, tk.fg.blend(tk.page.opacity(0.12)))
                };
                btn.custom(
                    ButtonCustomVariant::new(cx)
                        .color(bg)
                        .foreground(fg)
                        .hover(hover_bg)
                        .active(hover_bg),
                )
                .rounded(ButtonRounded::Medium)
                .h(fs(26.))
                .px(fs(12.))
                .bg(bg)
                .text_size(self.size.unwrap_or(fs(12.5)))
                .font_weight(FontWeight::MEDIUM)
            }
            BtnKind::Link | BtnKind::Danger => {
                let fg = if kind == BtnKind::Danger {
                    tk.danger
                } else {
                    tk.k6
                };
                btn.custom(
                    ButtonCustomVariant::new(cx)
                        .color(cx.theme().transparent)
                        .foreground(fg)
                        .hover(tk.k05)
                        .active(tk.k08),
                )
                .rounded(ButtonRounded::Medium)
                .h(fs(24.))
                .px(fs(6.))
                .text_size(self.size.unwrap_or(fs(12.)))
            }
        };

        let fades = kind != BtnKind::Primary;
        btn.when(fades && self.dimmed && !self.disabled, |b| b.opacity(0.55))
    }
}

/// A key on a keycap: `⌘`, `T`, `Space`.
pub fn keycap(key: impl Into<SharedString>, tk: &Tk) -> Div {
    div()
        .flex_shrink_0()
        .min_w(fs(20.))
        .h(fs(20.))
        .px(fs(5.))
        .rounded_sm()
        .bg(tk.k05)
        .shadow(vec![BoxShadow {
            color: tk.k15,
            offset: point(px(0.), px(-0.5)), // physical hairline bevel
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: true,
        }])
        .flex()
        .items_center()
        .justify_center()
        .text_size(fs(11.5))
        .text_color(tk.k6)
        .child(key.into())
}

/// A search glass the size the design draws it beside a field.
pub fn search_glass(size: f32, tk: &Tk) -> Icon {
    Icon::empty()
        .path("icons/settings/search.svg")
        .size(fs(size))
        .text_color(tk.k35)
}

/// A hairline: one device pixel. The design draws field edges at half a
/// point, which is exactly one pixel on a Retina panel — and on a 1x display
/// half a pixel at 15% ink is anti-aliased to nothing, so the shell and proxy
/// fields read as loose monospace text beside the boxed dropdowns.
///
/// Read from the scale the settings window last rendered at: the page's
/// controls are built from `cx` alone, and threading a `Window` through every
/// row to reach two fields would be a lot of signature for one number. A
/// thread-local rather than a global, because setting a global from `render`
/// queues an observer effect on every frame.
pub fn hairline() -> f32 {
    (1. / SCALE.with(std::cell::Cell::get)).max(0.5)
}

/// Record the scale the page is being drawn at, once per frame.
pub fn note_scale(window: &Window) {
    SCALE.with(|s| s.set(window.scale_factor().max(1.)));
}

thread_local! {
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(2.) };
}

/// A search field with a hairline ring, for filtering a list on the page.
pub fn search_field(input: &gpui::Entity<InputState>, tk: &Tk) -> Div {
    h_flex()
        .h(fs(26.))
        .px_2()
        .gap(fs(6.))
        .items_center()
        .rounded_md()
        .shadow(vec![ring(tk.k15, hairline(), true)])
        .child(search_glass(10., tk))
        .child(
            // On the input, not around it: `Input` sets its own `text_sm`
            // and only a size given to it directly outranks that.
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .appearance(false)
                    .px_0()
                    .py_0()
                    .h(fs(24.))
                    .text_size(fs(12.5)),
            ),
        )
}

/// A text field the way the page draws one: a hairline ring on the page, in
/// the terminal's monospace, with a firmer ring while it has focus.
pub fn text_field(
    input: &gpui::Entity<InputState>,
    focused: bool,
    invalid: bool,
    tk: &Tk,
    cx: &App,
) -> Div {
    field_box(input, focused, invalid, false, tk, cx)
}

/// A [`text_field`] for a masked input, with the eye that reveals it.
pub fn secret_field(
    input: &gpui::Entity<InputState>,
    focused: bool,
    tk: &Tk,
    cx: &App,
) -> Div {
    field_box(input, focused, false, true, tk, cx)
}

fn field_box(
    input: &gpui::Entity<InputState>,
    focused: bool,
    invalid: bool,
    reveal: bool,
    tk: &Tk,
    cx: &App,
) -> Div {
    let ring_color = if invalid {
        tk.danger
    } else if focused {
        tk.k45
    } else {
        tk.k15
    };
    div()
        .h(fs(26.))
        .px(fs(10.))
        .flex()
        .items_center()
        .rounded_md()
        .shadow(vec![ring(
            ring_color,
            if focused || invalid { 1. } else { hairline() },
            true,
        )])
        .font_family(Tk::mono(cx))
        .text_color(tk.fg)
        .child(
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .appearance(false)
                    .px_0()
                    .py_0()
                    .h(fs(24.))
                    // The input's own `text_sm` beat the 12px set on this
                    // box, so the value stood a size above every dropdown
                    // label in the same column.
                    .text_size(fs(12.))
                    .when(reveal, |i| i.mask_toggle()),
            ),
        )
}

/// A group's heading and, when it has one, the sentence under it.
pub fn section_head(title: &str, desc: Option<String>, tk: &Tk) -> Div {
    v_flex()
        .gap(fs(3.))
        .pb_2()
        .child(
            div()
                .text_size(fs(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(tk.heading)
                .child(title.to_string()),
        )
        .when_some(desc.filter(|d| !d.is_empty()), |col, desc| {
            col.child(
                div()
                    .text_size(fs(12.))
                    .line_height(fs(17.))
                    .text_color(tk.k6)
                    .child(desc),
            )
        })
}

/// The width of a control in the page's right-hand column — dropdown or text
/// field alike. The column used to size each dropdown to its value (anything
/// from 148 to 260) beside fields fixed at 180, so its left edge zig-zagged
/// down the page. One width, and a value too long for it elides. Only fields
/// that hold a path or a command line are wider, because those are unreadable
/// cut short.
pub const CONTROL_W: f32 = 180.;

/// The trigger of a dropdown: a raised button with the current value, an
/// optional mark before it, and a chevron, [`CONTROL_W`] wide.
pub fn select_trigger(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    leading: Option<AnyElement>,
    open: bool,
    tk: &Tk,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(fs(26.))
        .w(fs(CONTROL_W))
        .flex_shrink_0()
        .pl(fs(if leading.is_some() { 8. } else { 10. }))
        .pr(fs(7.))
        .gap_2()
        .items_center()
        .rounded_md()
        .bg(if open { tk.k05 } else { tk.btn })
        .shadow(tk.raised())
        .hover(move |s| s.shadow(tk.raised_hover()))
        .text_size(fs(12.5))
        .text_color(tk.fg)
        .whitespace_nowrap()
        .children(leading)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .child(chevron_down(tk))
}

pub fn chevron_down(tk: &Tk) -> Icon {
    Icon::empty()
        .path("icons/settings/chevron-down.svg")
        .size(fs(10.))
        .text_color(tk.k45)
}

pub fn check_mark(visible: bool, tk: &Tk) -> Div {
    div().size(fs(12.)).flex_shrink_0().when(visible, |d| {
        d.child(
            Icon::empty()
                .path("icons/settings/check.svg")
                .size(fs(12.))
                .text_color(tk.fg),
        )
    })
}

/// A popover's surface.
pub fn menu_panel(tk: &Tk) -> Div {
    v_flex()
        .p_1()
        .gap(px(1.)) // physical 1px micro-gap between menu rows
        .rounded_lg()
        .bg(tk.menu)
        .shadow(tk.popover())
        .text_size(fs(12.5))
        .text_color(tk.fg)
}

/// One pickable row in a popover menu.
pub fn menu_row(id: impl Into<ElementId>, highlighted: bool, tk: &Tk) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_shrink_0()
        .h(fs(26.))
        .pl(fs(6.))
        .pr_3()
        .gap(fs(6.))
        .items_center()
        .rounded_sm()
        .whitespace_nowrap()
        .when(highlighted, |r| r.bg(tk.k06))
        .hover(move |s| s.bg(tk.k06))
}

pub fn menu_heading(label: impl Into<SharedString>, tk: &Tk) -> Div {
    div()
        .h(fs(24.))
        .px_2()
        .flex()
        .items_center()
        .text_size(fs(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(tk.k45)
        .child(label.into())
}

pub fn menu_separator(tk: &Tk) -> Div {
    div().h(px(1.)).mx(fs(6.)).my_1().bg(tk.k08) // physical 1px hairline separator
}

/// Where a trigger's popover hangs: under the trigger, its right edge on the
/// trigger's (or its left on the trigger's), painted over the rest of the page
/// and pushed back inside the window when it would run off it.
pub fn popover_below(align_right: bool, gap: f32, content: impl IntoElement) -> Div {
    let anchor = if align_right {
        gpui::Anchor::TopRight
    } else {
        gpui::Anchor::TopLeft
    };
    div()
        .absolute()
        .top(fs(26. + gap))
        .when(align_right, |d| d.right_0())
        .when(!align_right, |d| d.left_0())
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(8.)) // physical viewport safety margin
                    .child(
                        div()
                            .occlude()
                            // A click inside the popover is not a click on the
                            // page under it, which would close it.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(content),
                    ),
            )
            .with_priority(2),
        )
}

/// The stepper: − value +, one raised control with hairline dividers.
pub struct Stepper {
    pub value: String,
    pub unit: String,
    pub can_dec: bool,
    pub can_inc: bool,
}

pub fn stepper(
    id: &'static str,
    s: Stepper,
    tk: &Tk,
    on_dec: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_inc: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let end = |suffix: &'static str, glyph: &'static str, enabled: bool| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .role(gpui::Role::Button)
            .w(fs(28.))
            .h(fs(26.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(fs(13.))
            .text_color(tk.k6)
            .when(!enabled, |d| d.opacity(0.35))
            .when(enabled, |d| d.active(move |s| s.bg(tk.k05)))
            .child(glyph)
    };
    let divider = || div().h(fs(14.)).w(px(1.)).bg(tk.k15); // physical 1px divider
    h_flex()
        .id(id)
        .h(fs(26.))
        .items_center()
        .rounded_md()
        .bg(tk.btn)
        .shadow(tk.raised())
        .child(
            end("dec", "−", s.can_dec)
                .rounded_l_md()
                .when(s.can_dec, |d| d.on_click(on_dec)),
        )
        .child(divider())
        .child(
            h_flex()
                .min_w(fs(52.))
                .px(fs(6.))
                .justify_center()
                .text_size(fs(12.5))
                .text_color(tk.fg)
                .child(s.value)
                .child(div().text_color(tk.k4).child(s.unit)),
        )
        .child(divider())
        .child(
            end("inc", "+", s.can_inc)
                .rounded_r_md()
                .when(s.can_inc, |d| d.on_click(on_inc)),
        )
        .into_any_element()
}

/// What a slider is being dragged by; the id tells two sliders apart.
#[derive(Clone)]
pub struct SliderDrag(pub SharedString);

impl Render for SliderDrag {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A thin track with an ink fill and a raised knob, 148 wide, with its value
/// read out beside it. `on_set` takes the fraction along the track the pointer
/// is at; `on_release` is where a drag's value gets written down for good.
pub fn slider(
    id: impl Into<SharedString>,
    fraction: f32,
    readout: String,
    tk: &Tk,
    on_set: Rc<dyn Fn(f32, &mut Window, &mut App)>,
    on_release: Rc<dyn Fn(&mut Window, &mut App)>,
) -> AnyElement {
    let id: SharedString = id.into();
    let f = fraction.clamp(0., 1.);
    const W: f32 = 148.;
    let frac_at = |bounds: gpui::Bounds<gpui::Pixels>, x: gpui::Pixels| -> f32 {
        let w = bounds.size.width.as_f32().max(1.);
        ((x - bounds.origin.x).as_f32() / w).clamp(0., 1.)
    };
    let set_down = on_set.clone();
    let set_move = on_set;
    let drag_id = id.clone();
    let release_up = on_release.clone();
    let release_out = on_release;
    let bounds = Rc::new(std::cell::Cell::new(gpui::Bounds::<gpui::Pixels>::default()));
    h_flex()
        .gap(px(8.))
        .items_center()
        .child(
            div()
                .id(ElementId::Name(id.clone()))
                .relative()
                .w(px(W))
                .h(px(18.))
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(8.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(tk.k15),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(8.))
                        .h(px(2.))
                        .w(px(W * f))
                        .rounded(px(1.))
                        .bg(tk.fg),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(W * f - 7.))
                        .size(px(14.))
                        .rounded_full()
                        .bg(tk.knob)
                        .shadow(vec![
                            ring(gpui::black().opacity(0.18), 0.5, false),
                            drop(gpui::black().opacity(0.18), 1., 3., 0.),
                        ]),
                )
                .child({
                    // The track's bounds for a click that never becomes a
                    // drag: a mouse-down event carries a position but not the
                    // box it landed in.
                    let bounds = bounds.clone();
                    gpui::canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                        .absolute()
                        .size_full()
                })
                .on_mouse_down(MouseButton::Left, {
                    let bounds = bounds.clone();
                    move |ev: &gpui::MouseDownEvent, window, cx| {
                        set_down(frac_at(bounds.get(), ev.position.x), window, cx);
                    }
                })
                .on_drag(SliderDrag(drag_id), |drag, _, _, cx| {
                    cx.new(|_| drag.clone())
                })
                .on_drag_move({
                    let id = id.clone();
                    move |ev: &gpui::DragMoveEvent<SliderDrag>, window, cx| {
                        if ev.drag(cx).0 != id {
                            return;
                        }
                        set_move(frac_at(ev.bounds, ev.event.position.x), window, cx);
                    }
                })
                .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                    release_up(window, cx)
                })
                .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                    release_out(window, cx)
                }),
        )
        .child(
            div()
                .w(fs(40.))
                .flex_shrink_0()
                .text_right()
                .whitespace_nowrap()
                .text_size(fs(12.5))
                .text_color(tk.k6)
                .child(readout),
        )
        .into_any_element()
}

/// Generic segmented toggle bar usable by standalone forms outside Settings.
pub fn segmented_control<V: 'static>(
    id: impl Into<SharedString>,
    options: &[&str],
    selected: usize,
    cx: &mut gpui::Context<V>,
    on_pick: impl Fn(&mut V, usize, &mut Window, &mut gpui::Context<V>) + 'static,
) -> AnyElement {
    let tk = Tk::of(cx);
    let id: SharedString = id.into();
    let on_pick = Rc::new(on_pick);
    h_flex()
        .id(ElementId::Name(id.clone()))
        .flex_shrink_0()
        .p(px(2.)) // physical 2px concentric tray inset
        .gap(px(2.))
        .rounded_md()
        .bg(tk.k05)
        .children(options.iter().enumerate().map(|(i, label)| {
            let active = i == selected;
            let on_pick = on_pick.clone();
            h_flex()
                .id(ElementId::NamedInteger(id.clone(), i as u64))
                .role(gpui::Role::Button)
                .items_center()
                .justify_center()
                .h(fs(22.))
                .px(fs(10.))
                .rounded_sm()
                .whitespace_nowrap()
                .text_size(fs(12.))
                .when(active, |s| {
                    s.bg(tk.chip).shadow(tk.raised()).text_color(tk.fg)
                })
                .when(!active, |s| {
                    s.text_color(tk.k5).hover(move |h| h.text_color(tk.fg))
                })
                .child(label.to_string())
                .on_click(cx.listener(move |this, _, window, cx| {
                    on_pick(this, i, window, cx);
                }))
        }))
        .into_any_element()
}
