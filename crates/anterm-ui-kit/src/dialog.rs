//! Chrome for the small modal cards the app draws itself — the SSH sheet, the
//! worktree prompt — and the buttons and key hints the home page shares with
//! them.
//!
//! The workspace switcher is the reference: the same 12px card, 48px header
//! row with an `esc` cap, hairline-divided footer and faint keycaps. Each of
//! these cards used to spell its own padding, title weight and button variants
//! out a builder call at a time, and they had drifted — a 20px inset here, a
//! semibold title there, the accent-filled primary everywhere. One place now,
//! so two sheets on screen at once cannot disagree about what a dialog is.

use gpui::{
    App, ClickEvent, Div, ElementId, Hsla, Rems, SharedString, Stateful, Window, div, prelude::*,
    rems,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use crate::panel_tokens::{HEADING, TAB_TEXT, TEXT};
use crate::presets::Surface;

/// Rem-scaled helper for dialog layout units (defined at the 16px base rem).
#[inline]
pub fn rem_px(px_at_16: f32) -> Rems {
    rems(px_at_16 / 16.)
}

/// The card's corner — a notch rounder than a menu: a dialog is the one thing
/// on screen while it is up.
pub const CARD_RADIUS: f32 = 12.;

/// Vertical offset from the top of the window for floating modal cards.
pub const CARD_TOP: f32 = 112.0;

/// The title row, the height of the switcher's search row, so a prompt that
/// opens where the switcher just closed puts its first line in the same place.
pub const HEADER_H: f32 = 48.;

/// The button row. A 28px button in it leaves 6px above and below, the same
/// fit as the switcher's footer and its New workspace button.
pub const FOOTER_H: f32 = 40.;

/// How far the title and the fields sit in from the card's edge — the
/// switcher's search-row inset, so the text columns of the two line up.
pub const INSET: f32 = 18.;

/// A text field: a borderless well on the faint fill, the sidebar search's
/// shape. The field's own `.small()` padding (8px) is what sets the text in.
pub const FIELD_H: f32 = 28.;

/// Space between one field and the next.
const FIELD_GAP: f32 = 14.;

/// A button: the field's height and the field's corner — the v5 confirm
/// card's 28px pill-less rectangle, on every card alike.
pub const BUTTON_H: f32 = 28.;
const BUTTON_PAD: f32 = 14.;

/// A key named in a hint: an 18px cap on the faint fill.
pub const KEYCAP: f32 = 18.;

/// The ladder a dialog's hovers are taken from. Cards sit on the popover
/// surface; the home page passes the window's own.
pub fn popover_rungs(cx: &App) -> Surface {
    cx.global::<crate::presets::Surfaces>().popover
}

/// The card itself, opaque and occluding. Callers add the header, body and
/// footer as children, in that order.
///
/// `title` is what a screen reader announces the card as — a dialog, named
/// by the question it asks.
pub fn card(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    width: f32,
    cx: &App,
) -> Stateful<Div> {
    v_flex()
        .id(id)
        .role(gpui::Role::Dialog)
        .aria_label(title)
        .occlude()
        .w(rem_px(width))
        .map(|panel| crate::theme::floating_surface(panel, cx))
        .rounded_xl()
        .overflow_hidden()
}

/// The title row: the title at body size in medium, and the `esc` cap on the
/// right that says how to back out. Every card here answers Escape.
pub fn header(title: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .gap(rem_px(10.))
        .h(rem_px(HEADER_H))
        .pl(rem_px(INSET))
        .pr(rem_px(14.))
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rems(TEXT))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .child(keycap("esc", cx))
}

/// The card's content column.
pub fn body() -> Div {
    v_flex()
        .gap(rem_px(FIELD_GAP))
        .px(rem_px(INSET))
        .pt_4()
        .pb(rem_px(18.))
}

/// The button row: answers on the right, the action last, over a hairline.
pub fn footer(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(rem_px(6.))
        .h(rem_px(FOOTER_H))
        .px(rem_px(6.))
        .border_t_1()
        .border_color(cx.theme().border)
}

/// A field's name: the compact section-heading step, medium, secondary ink.
pub fn label(text: impl IntoElement, cx: &App) -> Div {
    div()
        .text_size(rems(HEADING))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// The fill of a well on a card: one rung up the card's own ladder.
///
/// Not the theme's `muted`. That one is a step off the *window*, and a card
/// sits on the popover surface, which a dark theme lifts to about the same
/// value — so in the dark every field on a dialog lost its shape and the
/// labels floated over nothing.
pub fn well_fill(cx: &App) -> Hsla {
    gpui::rgb(popover_rungs(cx).hover).into()
}

/// A text field in its well. The input loses its own border and fill — the
/// well is the shape — and keeps its padding, which is the text's inset.
pub fn field(input: Input, cx: &App) -> Div {
    h_flex()
        .items_center()
        .h(rem_px(FIELD_H))
        .rounded_md()
        .bg(well_fill(cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(input.appearance(false).small()),
        )
}

/// A block of read-only detail — a host and its key — set on the field's
/// fill, so it reads as a value to check rather than a line of prose.
pub fn well(cx: &App) -> Div {
    v_flex()
        .gap_1()
        .px(rem_px(10.))
        .py_2()
        .rounded_md()
        .bg(well_fill(cx))
}

/// A key named in a hint: a small faint cap, never a button outline. The same
/// cap the switcher's header and footer draw.
pub fn keycap(label: impl Into<SharedString>, cx: &App) -> gpui::AnyElement {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .min_w(rem_px(KEYCAP))
        .h(rem_px(KEYCAP))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .bg(well_fill(cx))
        .text_size(rems(11. / 16.))
        .text_color(theme.muted_foreground)
        .child(label.into())
        .into_any_element()
}

/// A key combination as one cap per key — `⌘` `T`, never `⌘T` in one. The
/// way Search Everywhere and the shortcut editor spell a chord; the home page
/// and the switcher's footer had each packed theirs into a single cap.
pub fn chord(tokens: impl IntoIterator<Item = impl Into<SharedString>>, cx: &App) -> Div {
    h_flex()
        .flex_shrink_0()
        .gap(rem_px(3.))
        .children(tokens.into_iter().map(|t| keycap(t, cx)))
}

/// What a button is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The one answer a card is for: an inverted neutral, the surface's ink as
    /// the fill — the Commit button's paint, not the accent. The accent is
    /// already spent on focus and on the rail; a dialog of blue buttons made
    /// every sheet look like a call to action.
    Primary,
    /// Everything else: the well's faint fill at rest, one rung up under the
    /// pointer. A bare word with no shape read as a link, not as the second
    /// answer to the question.
    Secondary,
    /// The action a card warns about. Red, and only on the one site that has
    /// earned it — overriding a changed host key.
    Danger,
}

/// A dialog button backed by `gpui_kit::component::button::Button` so every
/// modal action gets `Role::Button`, focus rings, Space/Enter activation, and
/// consistent disabled semantics.
pub fn button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    tone: Tone,
    enabled: bool,
    rungs: Surface,
    cx: &App,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let theme = cx.theme();
    let surface: Hsla = gpui::rgb(rungs.base).into();
    let disabled_bg: Hsla = gpui::rgb(rungs.hover).into();
    let base = Button::new(id)
        .small()
        .rounded(ButtonRounded::Medium)
        .disabled(!enabled)
        .label(text.into())
        .h(rem_px(BUTTON_H))
        .px(rem_px(BUTTON_PAD))
        .text_size(rems(TAB_TEXT))
        .when(tone != Tone::Secondary, |b| {
            b.font_weight(gpui::FontWeight::MEDIUM)
        })
        .on_click(on_click);

    if !enabled {
        return base
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(disabled_bg)
                    .foreground(theme.muted_foreground)
                    .hover(disabled_bg)
                    .active(disabled_bg),
            )
            .bg(disabled_bg);
    }

    match tone {
        Tone::Danger => base.danger(),
        Tone::Primary => {
            let fg = theme.foreground;
            let hover = fg.blend(surface.opacity(0.14));
            base.custom(
                ButtonCustomVariant::new(cx)
                    .color(fg)
                    .foreground(surface)
                    .hover(hover)
                    .active(hover),
            )
            .bg(fg)
        }
        Tone::Secondary => {
            let fill: Hsla = gpui::rgb(rungs.hover).into();
            let hover: Hsla = gpui::rgb(rungs.selected).into();
            base.custom(
                ButtonCustomVariant::new(cx)
                    .color(fill)
                    .foreground(theme.foreground)
                    .hover(hover)
                    .active(hover),
            )
            .bg(fill)
        }
    }
}
