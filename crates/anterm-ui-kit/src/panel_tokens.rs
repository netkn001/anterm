//! Shared side-panel typography, spacing tokens, drag payloads, and stateless
//! micro-widgets shared across `dialog`, `prompt`, `anterm-remote-ui`,
//! `anterm-files-ui`, `anterm-editor-ui`, and `anterm-terminal`.

use std::path::PathBuf;

use anterm_i18n::{L10nKey, t};
use gpui::{AnyElement, FontWeight, div, prelude::*, px, rems};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};

use crate::window_chrome::{CONTENT_INSET, TILE_GLYPH};

pub const RESIZE_HANDLE_WIDTH: f32 = 8.;

const STEP: f32 = 1. / 16.;
pub const TEXT: f32 = 14. * STEP;
pub const TEXT_MONO: f32 = TEXT - STEP;
pub const META: f32 = 12. * STEP;
pub const META_MONO: f32 = META - STEP;
pub const HEADING: f32 = 11.5 * STEP;
pub const TAB_TEXT: f32 = 12.5 * STEP;

pub const INFO_ROW_H: f32 = 28.;
pub const PROC_ROW_H: f32 = 26.;
pub const SECTION_GAP: f32 = 16.;
pub const ROW_GLYPH: f32 = TILE_GLYPH;
pub const ROW_INSET: f32 = 8.;
pub const TEXT_INSET: f32 = CONTENT_INSET + ROW_INSET;
pub const ROW_FILL_RADIUS: gpui::Pixels = px(6.);

pub const SEARCH_H: f32 = 30.;
const SEARCH_GLYPH: f32 = 11.;
const SEARCH_GLYPH_GAP: f32 = 3.;

/// Width of the fixed cell a git status letter is centred in.
pub const BADGE_W: f32 = 10.;

/// A row dragged out of a remote Files tree: a path on that remote, which
/// nothing on this machine can open.
#[derive(Clone)]
pub struct RemotePathDrag {
    pub path: PathBuf,
}

pub fn action_strip(row: &gpui::SharedString, backing: u32) -> gpui::Div {
    h_flex()
        .absolute()
        .right(px(ROW_INSET))
        .top_0()
        .bottom_0()
        .items_center()
        .gap(px(1.))
        .bg(gpui::rgb(backing))
        .invisible()
        .group_hover(row.clone(), |s| s.visible())
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
}

/// Shared panel search/filter well usable by any standalone panel entity.
pub fn panel_search_box(
    input: &gpui::Entity<gpui_kit::component::input::InputState>,
    trailing: Option<AnyElement>,
    cx: &gpui::App,
) -> AnyElement {
    let has_trailing = trailing.is_some();
    h_flex()
        .flex_none()
        .items_center()
        .h(rems(INFO_ROW_H / 16.))
        .px_3()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(rems(SEARCH_GLYPH_GAP / 16.))
                .h(rems(INFO_ROW_H / 16.))
                .px_2()
                .rounded_md()
                .bg(cx.theme().muted)
                .child(
                    Icon::new(IconName::Search)
                        .size(rems(SEARCH_GLYPH / 16.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(input).appearance(false).xsmall().cleanable(true)),
                ),
        )
        .when(has_trailing, |row| row.gap(rems(6. / 16.)))
        .children(trailing)
        .into_any_element()
}

/// A single-letter git status marker in a fixed-width cell.
pub fn git_badge(letter: &str, color: gpui::Hsla, mono: &gpui::SharedString) -> AnyElement {
    div()
        .flex_none()
        .w(rems(BADGE_W / 16.))
        .text_center()
        .text_size(rems(META_MONO))
        .font_family(mono.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(letter.to_string())
        .into_any_element()
}

/// A small filled pill around a mono token — a pid, a port number.
pub fn info_chip(
    text: &str,
    bg: gpui::Hsla,
    fg: gpui::Hsla,
    mono: &gpui::SharedString,
) -> AnyElement {
    div()
        .flex_none()
        .px(rems(5. / 16.))
        .py(rems(1.5 / 16.))
        .rounded_sm()
        .bg(bg)
        .text_size(rems(META_MONO))
        .font_family(mono.clone())
        .text_color(fg)
        .child(text.to_string())
        .into_any_element()
}

pub fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        t(L10nKey::PanelRevealInFinder)
    } else {
        t(L10nKey::PanelOpenFolder)
    }
}
