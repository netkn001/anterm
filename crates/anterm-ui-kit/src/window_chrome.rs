//! Shared window-chrome layout metrics, title-bar drag gestures, overlay
//! surface layers, and icon-tile button helpers.
//!
//! Lives in `ui/chrome/` so lower-level workspace panels (`ui/editor/`,
//! `ui/files/`, `ui/workspace/`) share exact geometry without reverse-importing
//! from `ui/workspace/`.

use gpui::{App, Pixels, Stateful, Styled, Window, div, img, prelude::*, px};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, InteractiveElementExt as _, Selectable as _, Sizable as _,
};

/// The narrowest the terminal column may be squeezed to by the panels beside
/// it: roughly forty columns at the default font, which is a prompt with room
/// to read what it printed.
pub const TERMINAL_MIN_W: f32 = 360.;

pub const TITLE_BAR_HEIGHT: f32 = 48.;

/// The rule every title-bar row carries along its bottom edge.
pub const TITLE_BAR_BORDER: f32 = 1.;

/// The line the traffic lights and every [`TitleStripRow`]'s contents sit on,
/// from the top of the window's content.
pub const TITLE_STRIP_CENTRE: f32 = (TITLE_BAR_HEIGHT - TITLE_BAR_BORDER) / 2.;

pub fn title_strip_centre() -> Pixels {
    px(TITLE_STRIP_CENTRE)
}

/// Split the title bar's bottom edge between the rule a row draws and the
/// padding that stands in for the part of it the rule does not reach.
pub fn title_strip_rule_and_pad(rule: Pixels) -> (Pixels, Pixels) {
    let border = rule.min(px(TITLE_BAR_BORDER));
    (border, px(TITLE_BAR_BORDER - border.as_f32()))
}

/// Give a row the title bar's geometry: its height, its vertical centring, and
/// a bottom edge that leaves the content box the height
/// [`title_strip_centre`] assumes.
pub trait TitleStripRow: Sized {
    fn title_strip_row(self, rule: Pixels, ink: gpui::Hsla) -> Self;
}

impl<R: Styled> TitleStripRow for R {
    fn title_strip_row(self, rule: Pixels, ink: gpui::Hsla) -> Self {
        let (border, pad) = title_strip_rule_and_pad(rule);
        self.h(px(TITLE_BAR_HEIGHT))
            .items_center()
            .border_b(border)
            .border_color(ink)
            .pb(pad)
    }
}

pub const TILE_SIZE: f32 = 32.;
pub const TILE_GLYPH: f32 = 16.;
pub const TILE_SIZE_SM: f32 = 24.;
pub const TILE_GLYPH_SM: f32 = TILE_GLYPH;
pub const TILE_SIZE_XS: f32 = 18.;
pub const TILE_GLYPH_XS: f32 = 11.;

pub const TILE_GLYPH_LINE: f32 = 16.;

pub const TILE_PAD: f32 = (TILE_SIZE - TILE_GLYPH) / 2.;
pub const TILE_PAD_SM: f32 = (TILE_SIZE_SM - TILE_GLYPH_SM) / 2.;

pub const CONTENT_INSET: f32 = 12.;

const TILE_EDGE_GAP: f32 = 5.;

pub fn tile_trailing_inset() -> f32 {
    (CONTENT_INSET - TILE_PAD).max(TILE_EDGE_GAP)
}

pub const TITLE_BAR_LEAD: f32 = if cfg!(target_os = "macos") {
    crate::theme::TRAFFIC_LIGHTS_LEAD
} else {
    12.
};

pub const WINDOW_CONTROLS_W: f32 = if cfg!(target_os = "macos") { 0. } else { 102. };

pub fn title_bar_hug_offset() -> f32 {
    if cfg!(target_os = "macos") {
        0.
    } else {
        tile_trailing_inset() - TITLE_BAR_LEAD
    }
}

pub struct WindowMoveArm {
    should_move: bool,
}

pub fn title_bar_drag(
    row: Stateful<gpui::Div>,
    key: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<gpui::Div> {
    window_move_gesture(row, key, window, cx).on_double_click(|_, window, _| {
        if cfg!(target_os = "linux") {
            window.zoom_window();
        } else {
            window.titlebar_double_click();
        }
    })
}

pub fn window_move_gesture(
    row: Stateful<gpui::Div>,
    key: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<gpui::Div> {
    let arm = window.use_keyed_state(key, cx, |_, _| WindowMoveArm { should_move: false });
    row.window_control_area(gpui::WindowControlArea::Drag)
        .on_mouse_down(
            gpui::MouseButton::Left,
            window.listener_for(&arm, |arm, _: &gpui::MouseDownEvent, _, _| {
                arm.should_move = true;
            }),
        )
        .on_mouse_down_out(
            window.listener_for(&arm, |arm, _: &gpui::MouseDownEvent, _, _| {
                arm.should_move = false;
            }),
        )
        .on_mouse_up(
            gpui::MouseButton::Left,
            window.listener_for(&arm, |arm, _: &gpui::MouseUpEvent, _, _| {
                arm.should_move = false;
            }),
        )
        .on_mouse_up_out(
            gpui::MouseButton::Left,
            window.listener_for(&arm, |arm, _: &gpui::MouseUpEvent, _, _| {
                arm.should_move = false;
            }),
        )
        .on_mouse_move(
            window.listener_for(&arm, |arm, _: &gpui::MouseMoveEvent, window, _| {
                if arm.should_move {
                    arm.should_move = false;
                    window.start_window_move();
                }
            }),
        )
}

pub fn overlay_surface_layers(cx: &App) -> Vec<gpui::Div> {
    match window_background_image_layer(cx) {
        Some(image) => vec![
            image,
            div()
                .absolute()
                .inset_0()
                .bg(crate::theme::workspace_background(cx)),
        ],
        None => Vec::new(),
    }
}

/// The theme's background image as a full-bleed layer.
pub fn window_background_image_layer(cx: &App) -> Option<gpui::Div> {
    let image = cx
        .try_global::<crate::presets::ActiveBackground>()?
        .image
        .clone()?;
    Some(
        div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .opacity(image.opacity)
            .child(
                img(image.path)
                    .size_full()
                    .object_fit(gpui::ObjectFit::Cover),
            ),
    )
}

pub const BUTTON_ICON_SCALE: f32 = 0.75;
pub const RAIL_TILE: f32 = 26.;
pub const ROW_STATUS_DOT: f32 = 5.;
pub const ROW_STATUS_SLOT: f32 = 14.;
pub const RAIL_TILE_GLYPH: f32 = 14.;
pub const RAIL_TILE_RADIUS: f32 = 6.;

pub fn chrome_tile_variant_for(selected: bool, cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(cx.theme().transparent)
        .foreground(if selected {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .hover(gpui::rgb(cx.global::<crate::presets::Surfaces>().sidebar.hover).into())
        .active(cx.theme().sidebar_accent)
}

pub fn chrome_tile(button: Button, selected: bool, cx: &App) -> Button {
    chrome_tile_sized(button, TILE_SIZE, TILE_GLYPH, selected, cx)
}

pub fn chrome_tile_sized(
    button: Button,
    tile: f32,
    glyph: f32,
    selected: bool,
    cx: &App,
) -> Button {
    button
        .custom(chrome_tile_variant_for(selected, cx))
        .selected(selected)
        .with_size(px(glyph / BUTTON_ICON_SCALE))
        .w(px(tile))
        .h(px(tile))
}

use std::rc::Rc;

use anterm_core::config::{DocumentLayout, RightPanelTab};
use anterm_i18n::{L10nKey, t};
use gpui::{AnyElement, FontWeight, SharedString, rems};
use gpui_kit::component::h_flex;

/// The half of the window neither side panel may grow past on its own.
const SIDE_PANEL_MAX_RATIO: f32 = 0.5;

/// How wide one side panel may grow, given the floor under the terminal and the
/// floor under the *other* panel.
pub fn side_panel_max(viewport: f32, own_floor: f32, others_floor: f32) -> f32 {
    (viewport - TERMINAL_MIN_W - others_floor)
        .min(viewport * SIDE_PANEL_MAX_RATIO)
        .max(own_floor)
}

/// Whether the title bar carries minimize, maximize and close right now.
pub fn window_controls_drawn(fullscreen: bool) -> bool {
    !cfg!(target_os = "macos") && !fullscreen
}

/// How much of the title bar's trailing end the window buttons take.
pub fn window_controls_w(fullscreen: bool) -> f32 {
    match window_controls_drawn(fullscreen) {
        true => WINDOW_CONTROLS_W,
        false => 0.,
    }
}

pub fn measure_text(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
) -> f32 {
    text_system
        .shape_line(
            SharedString::from(text),
            px(size),
            &[gpui::TextRun {
                len: text.len(),
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
}

pub const MIN_TARGET: f32 = 24.;

pub fn hit_target(button: Button) -> Button {
    button.w(px(MIN_TARGET)).h(px(MIN_TARGET))
}

pub const RIGHT_PANEL_TABS: [(RightPanelTab, L10nKey); 3] = [
    (RightPanelTab::Info, L10nKey::PanelInfoTitle),
    (RightPanelTab::Files, L10nKey::PanelFilesTitle),
    (RightPanelTab::Scm, L10nKey::PanelChangesTitle),
];

pub const TAB_OUTER_PAD: f32 = 2.;
pub const TAB_INNER_PAD: f32 = 4.;

fn right_panel_tab_size(window: &Window) -> f32 {
    window.rem_size().as_f32() * crate::panel_tokens::TAB_TEXT
}

fn right_panel_tab_font(cx: &App) -> gpui::Font {
    gpui::Font {
        family: cx.theme().font_family.clone(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::MEDIUM,
        style: Default::default(),
    }
}

pub fn right_panel_tab_labels_w(window: &Window, cx: &App) -> f32 {
    let size = right_panel_tab_size(window);
    let font = right_panel_tab_font(cx);
    RIGHT_PANEL_TABS
        .iter()
        .map(|(_, key)| {
            measure_text(window.text_system(), &font, size, t(*key))
                + 2. * (TAB_OUTER_PAD + TAB_INNER_PAD)
        })
        .sum()
}

pub fn trailing_chrome_tiles_w() -> f32 {
    let trailing_pad = if cfg!(target_os = "macos") {
        tile_trailing_inset()
    } else {
        4.
    };
    trailing_pad + TILE_SIZE
}

pub fn trailing_chrome_w(fullscreen: bool) -> f32 {
    trailing_chrome_tiles_w() + window_controls_w(fullscreen)
}

/// Which document surface sits on top when both the code panel and the diff
/// overlay are open in a tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OverlayTop {
    #[default]
    Code,
    Diff,
}

/// The narrowest a docked document column may be squeezed to.
pub const DOCUMENT_MIN_W: f32 = 280.;

/// How wide the docked document column is, given body width and ratio.
pub fn document_column_px(body: f32, ratio: f32) -> Option<f32> {
    if !body.is_finite() || body < TERMINAL_MIN_W + DOCUMENT_MIN_W {
        return None;
    }
    let ratio = if ratio.is_finite() { ratio } else { 0.5 };
    Some((body * ratio).clamp(DOCUMENT_MIN_W, body - TERMINAL_MIN_W))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentChrome {
    Fill,
    Dock,
    DockHoisted,
}

impl DocumentChrome {
    pub fn is_dock(self) -> bool {
        !matches!(self, DocumentChrome::Fill)
    }

    pub fn renders_own_header(self) -> bool {
        !matches!(self, DocumentChrome::DockHoisted)
    }

    pub fn header_is_title_strip(self) -> bool {
        match self {
            DocumentChrome::Fill | DocumentChrome::DockHoisted => true,
            DocumentChrome::Dock => cfg!(target_os = "macos"),
        }
    }
}

pub const HEADER_CELL_H: f32 = 28.;
pub const HEADER_CELL_RADIUS: f32 = 7.;
pub const HEADER_CELL_MAX_W: f32 = 168.;
pub const HEADER_CELL_TEXT: f32 = 12.5 / 16.;

const SWITCH_CELL_W: f32 = 24.;
const SWITCH_CELL_H: f32 = 22.;
const SWITCH_FRAME_W: f32 = 14.;
const SWITCH_FRAME_H: f32 = 11.;
const SWITCH_EDGE: f32 = 2.;
const SWITCH_GAP: f32 = 2.;
const SWITCH_LEAD: f32 = 8.;
const SWITCH_CELL_RADIUS: f32 = 5.;
const SWITCH_TRACK_RADIUS: f32 = 7.;

pub fn layout_switch_cell(chrome: DocumentChrome) -> DocumentLayout {
    match chrome.is_dock() {
        true => DocumentLayout::Dock,
        false => DocumentLayout::Fill,
    }
}

pub fn switch_ink(live: bool, cx: &App) -> gpui::Hsla {
    let theme = cx.theme();
    match live {
        true => theme.foreground,
        false => theme.muted_foreground,
    }
}

fn switch_frame(ink: gpui::Hsla) -> gpui::Div {
    div()
        .relative()
        .w(px(SWITCH_FRAME_W))
        .h(px(SWITCH_FRAME_H))
        .border_1()
        .border_color(ink)
        .rounded(px(2.))
}

pub fn glyph_dock(ink: gpui::Hsla) -> AnyElement {
    switch_frame(ink)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(4.))
                .w(px(1.))
                .bg(ink),
        )
        .into_any_element()
}

pub fn glyph_fill(ink: gpui::Hsla) -> AnyElement {
    switch_frame(ink)
        .child(
            div()
                .absolute()
                .top(px(1.5))
                .bottom(px(1.5))
                .left(px(1.5))
                .right(px(1.5))
                .rounded(px(0.8))
                .bg(ink),
        )
        .into_any_element()
}

fn glyph_two_panes(ink: gpui::Hsla, side_by_side: bool) -> AnyElement {
    const GAP: f32 = 2.;
    let (w, h) = match side_by_side {
        true => ((SWITCH_FRAME_W - GAP) / 2., SWITCH_FRAME_H),
        false => (SWITCH_FRAME_W, (SWITCH_FRAME_H - GAP) / 2.),
    };
    let pane = || {
        div()
            .w(px(w))
            .h(px(h))
            .border_1()
            .border_color(ink)
            .rounded(px(1.5))
    };
    div()
        .flex()
        .when(!side_by_side, |d| d.flex_col())
        .w(px(SWITCH_FRAME_W))
        .h(px(SWITCH_FRAME_H))
        .gap(px(GAP))
        .child(pane())
        .child(pane())
        .into_any_element()
}

pub fn glyph_split(ink: gpui::Hsla) -> AnyElement {
    glyph_two_panes(ink, true)
}

pub fn glyph_unified(ink: gpui::Hsla) -> AnyElement {
    glyph_two_panes(ink, false)
}

pub fn switch_track(id: &'static str, cells: Vec<AnyElement>, cx: &App) -> AnyElement {
    let sf = cx.global::<crate::presets::Surfaces>().window;
    h_flex()
        .id(id)
        .occlude()
        .flex_none()
        .ml(px(SWITCH_LEAD))
        .p(px(SWITCH_EDGE))
        .gap(px(SWITCH_GAP))
        .rounded(px(SWITCH_TRACK_RADIUS))
        .bg(gpui::rgb(sf.hover))
        .children(cells)
        .into_any_element()
}

pub fn switch_cell<H>(
    id: impl Into<gpui::ElementId>,
    glyph: AnyElement,
    live: bool,
    tip: L10nKey,
    on_pick: H,
    cx: &App,
) -> AnyElement
where
    H: Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
{
    let theme = cx.theme();
    let sf = cx.global::<crate::presets::Surfaces>().window;
    let dark = theme.mode.is_dark();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(t(tip))
        .aria_toggled(if live {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .w(px(SWITCH_CELL_W))
        .h(px(SWITCH_CELL_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(SWITCH_CELL_RADIUS))
        .cursor_pointer()
        .when(live, |cell| {
            cell.bg(match dark {
                true => gpui::rgb(sf.selected).into(),
                false => theme.background,
            })
            .shadow_xs()
        })
        .child(glyph)
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(t(tip)).build(window, cx))
        .on_click(on_pick)
        .into_any_element()
}

pub fn document_terminal_button<H>(
    chrome: DocumentChrome,
    name: &str,
    on_dock: H,
    window: &Window,
    cx: &App,
) -> Option<AnyElement>
where
    H: Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
{
    if chrome != DocumentChrome::Fill {
        return None;
    }
    let theme = cx.theme();
    let name = name.to_string();
    let badge_bg = gpui::rgb(cx.global::<crate::presets::Surfaces>().window.selected);
    Some(
        h_flex()
            .flex_none()
            .items_center()
            .child(
                h_flex()
                    .id("document-back-to-terminal")
                    .when(cfg!(test), |button| {
                        button.debug_selector(|| "document-back-to-terminal".into())
                    })
                    .role(gpui::Role::Button)
                    .aria_label(format!("{}: {name}", t(L10nKey::DocumentDock)))
                    .occlude()
                    .h(px(HEADER_CELL_H))
                    .pl(px(8.))
                    .pr(px(10.))
                    .gap(px(7.))
                    .items_center()
                    .rounded(px(HEADER_CELL_RADIUS))
                    .text_size(rems(HEADER_CELL_TEXT))
                    .text_color(theme.muted_foreground)
                    .hover(|s| s.bg(theme.sidebar_accent).text_color(theme.foreground))
                    .child(
                        div()
                            .flex_none()
                            .size(px(16.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(badge_bg)
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(9.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(">_"),
                    )
                    .child(
                        div()
                            .max_w(px(HEADER_CELL_MAX_W))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(name),
                    )
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(t(L10nKey::DocumentDock))
                            .build(window, cx)
                    })
                    .on_click(on_dock),
            )
            .child(
                div()
                    .flex_none()
                    .w(crate::theme::hairline(window))
                    .h(px(16.))
                    .mx(px(6.))
                    .bg(theme.border),
            )
            .into_any_element(),
    )
}

pub fn document_layout_switch_control<F>(
    chrome: DocumentChrome,
    on_pick: F,
    cx: &App,
) -> AnyElement
where
    F: Fn(DocumentLayout, &mut Window, &mut App) + 'static,
{
    let on_pick = Rc::new(on_pick);
    let current = layout_switch_cell(chrome);
    let cell = |i: usize,
                layout: DocumentLayout,
                glyph: fn(gpui::Hsla) -> AnyElement,
                tip: L10nKey| {
        let live = layout == current;
        let ink = switch_ink(live, cx);
        let on_pick = on_pick.clone();
        switch_cell(
            ("document-layout", i),
            glyph(ink),
            live,
            tip,
            move |_, window, cx| on_pick(layout, window, cx),
            cx,
        )
    };
    switch_track(
        "document-layout-switch",
        vec![
            cell(0, DocumentLayout::Dock, glyph_dock, L10nKey::DocumentDock),
            cell(1, DocumentLayout::Fill, glyph_fill, L10nKey::DocumentFill),
        ],
        cx,
    )
}
