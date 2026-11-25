//! Unified in-palette `Edit Tab Tags` page: search, create, attach/detach,
//! recolor (inline 9-swatch palette), rename, and delete tags directly inside
//! the command palette without opening a separate popover window.

use gpui::{
    Context, Entity, FontWeight, MouseButton, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use anterm_core::machine::TabId;
use anterm_i18n::{L10nKey, t, t_fmt};

use super::{PalettePage, SearchEvent, SearchView};
use crate::core::tag::{SidebarTags, TagColor, TagId};
use crate::ui::chrome::dialog;
use crate::ui::workspace::tab_sidebar::{TagVisual, tag_color_label, tag_visual};

const TAG_ROW_HEIGHT: f32 = 34.;

/// One selectable row in the `Edit Tab Tags` page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TagPopoverRow {
    Existing(TagId),
    Create(String),
}

/// Builds the filtered rows for `query` against `catalog`.
///
/// Matching is case-insensitive substring match against each tag's name. When
/// the trimmed query is non-empty and does not exact-match any existing tag
/// (case-insensitively), a `Create` row is appended at the end.
pub(crate) fn popover_rows(catalog: &SidebarTags, query: &str) -> Vec<TagPopoverRow> {
    let trimmed = query.trim();
    let needle = trimmed.to_lowercase();
    let mut rows: Vec<TagPopoverRow> = catalog
        .tags
        .iter()
        .filter(|tag| needle.is_empty() || tag.name.to_lowercase().contains(&needle))
        .map(|tag| TagPopoverRow::Existing(tag.id))
        .collect();
    if !trimmed.is_empty() && catalog.find_by_name(trimmed).is_none() {
        rows.push(TagPopoverRow::Create(trimmed.to_string()));
    }
    rows
}

/// Advances `color` to the next entry in [`TagColor::ALL`], wrapping around.
pub(crate) fn next_tag_color(color: TagColor) -> TagColor {
    let pos = TagColor::ALL
        .iter()
        .position(|&c| c == color)
        .unwrap_or(0);
    TagColor::ALL[(pos + 1) % TagColor::ALL.len()]
}

/// Inline rename state for an existing tag inside the `Edit Tab Tags` page.
pub(super) struct TagInlineRename {
    pub(super) tag: TagId,
    pub(super) input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// State for the `Edit Tab Tags` page on the command palette navigation stack.
pub(super) struct EditTagsPage {
    pub(super) tab: TabId,
    pub(super) query: String,
    pub(super) selected: usize,
    pub(super) create_color: TagColor,
    /// Existing tag currently showing its inline 9-swatch color picker tray.
    pub(super) editing_color: Option<TagId>,
    /// Existing tag currently being renamed inline.
    pub(super) renaming: Option<TagInlineRename>,
}

impl EditTagsPage {
    pub(super) fn new(tab: TabId) -> Self {
        Self {
            tab,
            query: String::new(),
            selected: 0,
            create_color: TagColor::None,
            editing_color: None,
            renaming: None,
        }
    }
}

impl SearchView {
    pub(super) fn edit_tags_page(&self) -> Option<&EditTagsPage> {
        match self.top_page() {
            PalettePage::EditTags(p) => Some(p),
            _ => None,
        }
    }

    pub(super) fn edit_tags_page_mut(&mut self) -> Option<&mut EditTagsPage> {
        match self.pages.last_mut() {
            Some(PalettePage::EditTags(p)) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn is_editing_tags(&self) -> bool {
        self.edit_tags_page().is_some()
    }

    pub(super) fn is_tag_inline_renaming(&self) -> bool {
        self.edit_tags_page()
            .is_some_and(|p| p.renaming.is_some())
    }

    /// Opens the unified `Edit Tab Tags` page targeting `tab`.
    /// When `push` is true, pushes onto the navigation stack so `Escape` pops
    /// back to the parent page; otherwise replaces the stack as a standalone
    /// root modal. Optionally starts inline rename on `initial_rename` right away.
    pub(crate) fn open_edit_tags_for(
        &mut self,
        tab: TabId,
        initial_rename: Option<TagId>,
        push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = PalettePage::EditTags(EditTagsPage::new(tab));
        if push {
            self.snapshot_top_page(cx);
            self.pages.push(page);
        } else {
            self.pages = vec![page];
        }
        self.sync_placeholder(window, cx);
        self.set_input_silently(String::new(), window, cx);
        self.focus(window, cx);
        if let Some(tag_id) = initial_rename {
            self.start_edit_tags_rename(tag_id, window, cx);
        }
        cx.notify();
    }

    pub(super) fn move_edit_tags_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        let len = popover_rows(&self.catalog.sidebar_tags, &query).len();
        if len == 0 {
            return;
        }
        if let Some(p) = self.edit_tags_page_mut() {
            let cur = p.selected.min(len - 1) as isize;
            p.selected = (cur + delta).rem_euclid(len as isize) as usize;
        }
        cx.notify();
    }

    pub(crate) fn confirm_edit_tags_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((tab, selected, create_color)) = self
            .edit_tags_page()
            .map(|p| (p.tab, p.selected, p.create_color))
        else {
            return;
        };
        let query = self.input.read(cx).value().to_string();
        let rows = popover_rows(&self.catalog.sidebar_tags, &query);
        let Some(row) = rows.get(selected.min(rows.len().saturating_sub(1))).cloned() else {
            return;
        };
        match row {
            TagPopoverRow::Existing(tag) => {
                cx.emit(SearchEvent::ToggleTabTag { tab, tag });
            }
            TagPopoverRow::Create(name) => {
                cx.emit(SearchEvent::CreateAndAttachTag {
                    tab,
                    name,
                    color: create_color,
                });
                self.set_input_silently(String::new(), window, cx);
                if let Some(p) = self.edit_tags_page_mut() {
                    p.query.clear();
                    p.selected = 0;
                    p.create_color = TagColor::None;
                }
                self.focus(window, cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn cycle_edit_tags_create_color(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.edit_tags_page_mut() {
            p.create_color = next_tag_color(p.create_color);
            cx.notify();
        }
    }

    pub(crate) fn set_edit_tags_create_color(&mut self, color: TagColor, cx: &mut Context<Self>) {
        if let Some(p) = self.edit_tags_page_mut() {
            p.create_color = color;
            cx.notify();
        }
    }

    pub(crate) fn toggle_edit_tags_color_editor(&mut self, id: TagId, cx: &mut Context<Self>) {
        if let Some(p) = self.edit_tags_page_mut() {
            p.renaming = None;
            p.editing_color = if p.editing_color == Some(id) {
                None
            } else {
                Some(id)
            };
            cx.notify();
        }
    }

    pub(crate) fn start_edit_tags_rename(
        &mut self,
        id: TagId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit_tags_page().is_none() {
            return;
        }
        let Some(current) = self.catalog.sidebar_tags.get(id).map(|t| t.name.clone()) else {
            return;
        };
        let input = crate::ui::chrome::prefill::filled_box(current, window, cx);
        input.update(cx, |s, cx| s.focus(window, cx));
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.commit_edit_tags_rename(window, cx);
                }
            },
        )];
        if let Some(p) = self.edit_tags_page_mut() {
            p.editing_color = None;
            p.renaming = Some(TagInlineRename {
                tag: id,
                input,
                _subs: subs,
            });
        }
        cx.notify();
    }

    pub(crate) fn commit_edit_tags_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self
            .edit_tags_page_mut()
            .and_then(|p| p.renaming.take())
        else {
            return;
        };
        let value = rename.input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            cx.emit(SearchEvent::RenameSidebarTag {
                tag: rename.tag,
                name: value,
            });
        }
        self.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_edit_tags_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .edit_tags_page_mut()
            .and_then(|p| p.renaming.take())
            .is_some()
        {
            self.focus(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn delete_tag_in_edit_tags(
        &mut self,
        id: TagId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_renaming = self
            .edit_tags_page()
            .and_then(|p| p.renaming.as_ref())
            .is_some_and(|r| r.tag == id);
        if let Some(p) = self.edit_tags_page_mut() {
            if p.editing_color == Some(id) {
                p.editing_color = None;
            }
            if p.renaming.as_ref().is_some_and(|r| r.tag == id) {
                p.renaming = None;
            }
        }
        cx.emit(SearchEvent::DeleteSidebarTag { tag: id });
        if was_renaming {
            self.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn selected_edit_tags_row(&self, cx: &gpui::App) -> Option<TagPopoverRow> {
        let p = self.edit_tags_page()?;
        let query = self.input.read(cx).value().to_string();
        let rows = popover_rows(&self.catalog.sidebar_tags, &query);
        rows.get(p.selected.min(rows.len().saturating_sub(1)))
            .cloned()
    }

    fn render_color_palette(
        &self,
        scope_id: &'static str,
        active_color: TagColor,
        editing_existing: Option<TagId>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let active_vis = tag_visual(active_color, cx);
        let popover_bg = cx.theme().popover;
        let fg = cx.theme().foreground;
        let muted_fg = cx.theme().muted_foreground;
        let border = cx.theme().border;

        let mut swatches = h_flex().items_center().gap_1();
        for (ix, color) in TagColor::ALL.into_iter().enumerate() {
            let is_active = color == active_color;
            let vis = tag_visual(color, cx);
            let label = tag_color_label(color);

            let disc = match vis {
                Some(vis) => div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .rounded_full()
                    .bg(vis.dot)
                    .when(is_active, |el| {
                        el.child(
                            Icon::new(IconName::Check)
                                .w(px(10.))
                                .h(px(10.))
                                .text_color(vis.on_swatch),
                        )
                    }),
                None => div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(16.))
                    .rounded_full()
                    .bg(popover_bg)
                    .border_1()
                    .border_color(muted_fg.opacity(0.65))
                    .child(if is_active {
                        Icon::new(IconName::Check)
                            .w(px(10.))
                            .h(px(10.))
                            .text_color(fg)
                    } else {
                        Icon::new(IconName::Close)
                            .w(px(9.))
                            .h(px(9.))
                            .text_color(muted_fg.opacity(0.75))
                    }),
            };

            let ring_color = vis.map_or(fg, |v| v.dot);
            let swatch = div()
                .id((scope_id, ix))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(px(24.))
                .rounded_full()
                .border_2()
                .when(is_active, |el| el.bg(popover_bg).border_color(ring_color))
                .when(!is_active, |el| {
                    el.border_color(gpui::transparent_black())
                        .hover(|s| s.border_color(fg.opacity(0.35)))
                })
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _window, cx| {
                    cx.stop_propagation();
                    match editing_existing {
                        Some(tag) => {
                            if is_active {
                                if let Some(p) = this.edit_tags_page_mut() {
                                    p.editing_color = None;
                                }
                                cx.notify();
                            } else {
                                cx.emit(SearchEvent::SetTagColor { tag, color });
                            }
                        }
                        None => {
                            this.set_edit_tags_create_color(color, cx);
                        }
                    }
                }))
                .child(disc);

            swatches = swatches.child(swatch);
        }

        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
            .bg(cx.theme().muted.opacity(0.65))
            .border_1()
            .border_color(border.opacity(0.75))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(swatches)
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(active_vis.map_or(muted_fg, |v| v.text))
                    .child(tag_color_label(active_color)),
            )
    }

    pub(super) fn render_edit_tags_body(
        &self,
        max_h: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(p) = self.edit_tags_page() else {
            return div().into_any_element();
        };
        let target_tab = p.tab;
        let attached_tags = self
            .catalog
            .tab_tags
            .get(&target_tab)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let query = self.input.read(cx).value().to_string();
        let rows = popover_rows(&self.catalog.sidebar_tags, &query);
        let selected = if rows.is_empty() {
            0
        } else {
            p.selected.min(rows.len() - 1)
        };
        let create_color = p.create_color;
        let editing_color = p.editing_color;
        let renaming_tag = p.renaming.as_ref().map(|r| (r.tag, r.input.clone()));
        let rungs = dialog::popover_rungs(cx);

        if rows.is_empty() {
            let muted = cx.theme().muted_foreground;
            return v_flex()
                .w_full()
                .px(gpui::rems(14. / 16.))
                .py(gpui::rems(18. / 16.))
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(t(L10nKey::SearchTagsEmpty)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t(L10nKey::SidebarTagSearchPlaceholder)),
                )
                .into_any_element();
        }

        let mut list = v_flex()
            .id("palette-edit-tags-list")
            .w_full()
            .max_h(max_h)
            .overflow_y_scroll()
            .p_1p5()
            .gap_1();

        for (row_idx, row) in rows.iter().enumerate() {
            let is_selected = row_idx == selected;
            match row {
                TagPopoverRow::Existing(tag_id) => {
                    let Some(tag) = self.catalog.sidebar_tags.get(*tag_id) else {
                        continue;
                    };
                    let tag_id = tag.id;
                    let tag_color = tag.color;
                    let attached = attached_tags.contains(&tag_id);
                    let is_editing = editing_color == Some(tag_id);
                    let vis = tag_visual(tag_color, cx);

                    if let Some((_, rename_input)) =
                        renaming_tag.as_ref().filter(|(id, _)| *id == tag_id)
                    {
                        let fg = cx.theme().foreground;
                        let muted_fg = cx.theme().muted_foreground;
                        let dot_el = match vis {
                            Some(vis) => div()
                                .flex_shrink_0()
                                .size(px(7.))
                                .rounded_full()
                                .bg(vis.dot),
                            None => div()
                                .flex_shrink_0()
                                .size(px(7.))
                                .rounded_full()
                                .border_1()
                                .border_color(muted_fg.opacity(0.6)),
                        };
                        let rename_row = h_flex()
                            .id(("palette-tag-rename-row", row_idx))
                            .w_full()
                            .h(px(TAG_ROW_HEIGHT))
                            .px_2()
                            .gap_1p5()
                            .items_center()
                            .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                            .bg(gpui::rgb(rungs.selected))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(dot_el)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(rename_input).appearance(false).small().px_0()),
                            )
                            .child(
                                h_flex()
                                    .id(("palette-tag-rename-ok", row_idx))
                                    .flex_shrink_0()
                                    .size(px(22.))
                                    .items_center()
                                    .justify_center()
                                    .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                                    .text_color(fg)
                                    .hover(|s| s.bg(gpui::rgb(rungs.hover)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.commit_edit_tags_rename(window, cx);
                                    }))
                                    .child(Icon::new(IconName::Check).w(px(12.)).h(px(12.))),
                            )
                            .child(
                                h_flex()
                                    .id(("palette-tag-rename-cancel", row_idx))
                                    .flex_shrink_0()
                                    .size(px(22.))
                                    .items_center()
                                    .justify_center()
                                    .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                                    .text_color(muted_fg)
                                    .hover(|s| s.bg(gpui::rgb(rungs.hover)).text_color(fg))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.cancel_edit_tags_rename(window, cx);
                                    }))
                                    .child(Icon::new(IconName::Close).w(px(11.)).h(px(11.))),
                            );
                        list = list.child(rename_row);
                        continue;
                    }

                    let row_group = SharedString::from(format!("palette-tag-row-{row_idx}"));
                    let rename_group =
                        SharedString::from(format!("palette-tag-rename-btn-{row_idx}"));
                    let delete_group =
                        SharedString::from(format!("palette-tag-delete-btn-{row_idx}"));
                    let color_btn_tip = t(L10nKey::SidebarTagColor);
                    let rename_tip = t(L10nKey::SidebarRenameTag);
                    let delete_tip = t(L10nKey::SidebarDeleteTag);
                    let fg = cx.theme().foreground;
                    let muted_fg = cx.theme().muted_foreground;
                    let danger = cx.theme().danger;

                    let rename_btn = h_flex()
                        .id(("palette-tag-rename-btn", row_idx))
                        .group(rename_group.clone())
                        .flex_shrink_0()
                        .size(px(22.))
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                        .when(!is_selected, |el| {
                            el.opacity(0.)
                                .group_hover(row_group.clone(), |s| s.opacity(1.))
                        })
                        .hover(move |s| s.bg(gpui::rgb(rungs.hover)))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(rename_tip).build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.start_edit_tags_rename(tag_id, window, cx);
                        }))
                        .child(
                            gpui::svg()
                                .path(gpui_kit::assets::IconName::Pencil.path())
                                .flex_shrink_0()
                                .w(px(12.))
                                .h(px(12.))
                                .text_color(muted_fg)
                                .group_hover(rename_group, move |s| s.text_color(fg)),
                        );

                    let delete_btn = h_flex()
                        .id(("palette-tag-delete-btn", row_idx))
                        .group(delete_group.clone())
                        .flex_shrink_0()
                        .size(px(22.))
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                        .when(!is_selected, |el| {
                            el.opacity(0.)
                                .group_hover(row_group.clone(), |s| s.opacity(1.))
                        })
                        .hover(move |s| s.bg(danger.opacity(0.14)))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(delete_tip).build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.delete_tag_in_edit_tags(tag_id, window, cx);
                        }))
                        .child(
                            gpui::svg()
                                .path(gpui_kit::assets::IconName::Trash.path())
                                .flex_shrink_0()
                                .w(px(12.))
                                .h(px(12.))
                                .text_color(muted_fg)
                                .group_hover(delete_group, move |s| s.text_color(danger)),
                        );

                    let swatch_btn = h_flex()
                        .id(("palette-tag-color-btn", row_idx))
                        .flex_shrink_0()
                        .size(px(24.))
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .border_1()
                        .when(is_editing, |el| {
                            el.bg(gpui::rgb(rungs.selected))
                                .border_color(cx.theme().border)
                        })
                        .when(!is_editing, |el| {
                            el.border_color(gpui::transparent_black())
                                .hover(|s| s.bg(gpui::rgb(rungs.hover)))
                        })
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(color_btn_tip)
                                .build(window, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_edit_tags_color_editor(tag_id, cx);
                        }))
                        .child(match vis {
                            Some(vis) => div()
                                .size(px(12.))
                                .rounded_full()
                                .bg(vis.dot)
                                .border_1()
                                .border_color(vis.pill_border_selected),
                            None => div()
                                .size(px(12.))
                                .rounded_full()
                                .border_1()
                                .border_color(muted_fg.opacity(0.6)),
                        });

                    let row_el = h_flex()
                        .id(("palette-tag-row", row_idx))
                        .group(row_group)
                        .w_full()
                        .h(px(TAG_ROW_HEIGHT))
                        .px_2()
                        .gap_1()
                        .items_center()
                        .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                        .when(is_selected, |el| el.bg(gpui::rgb(rungs.selected)))
                        .when(!is_selected, |el| {
                            el.hover(|s| s.bg(gpui::rgb(rungs.hover)))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            if let Some(p) = this.edit_tags_page_mut() {
                                p.selected = row_idx;
                            }
                            cx.emit(SearchEvent::ToggleTabTag {
                                tab: target_tab,
                                tag: tag_id,
                            });
                        }))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .child(render_tag_chip(tag.name.clone(), vis, cx)),
                        )
                        .child(rename_btn)
                        .child(delete_btn)
                        .child(swatch_btn)
                        .child(
                            div()
                                .w(px(16.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(attached, |el| {
                                    el.child(
                                        Icon::new(IconName::Check)
                                            .xsmall()
                                            .text_color(vis.map_or(fg, |v| v.text)),
                                    )
                                }),
                        );

                    if is_editing {
                        list = list.child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(row_el)
                                .child(self.render_color_palette(
                                    "palette-tag-existing-swatch",
                                    tag_color,
                                    Some(tag_id),
                                    cx,
                                )),
                        );
                    } else {
                        list = list.child(row_el);
                    }
                }
                TagPopoverRow::Create(name) => {
                    let create_name = name.clone();
                    let label = t_fmt(L10nKey::SidebarCreateAndAddTag, &[("name", name)]);
                    let create_vis = tag_visual(create_color, cx);

                    let action_row = h_flex()
                        .id(("palette-tag-row", row_idx))
                        .w_full()
                        .h(px(TAG_ROW_HEIGHT))
                        .px_2()
                        .gap_2()
                        .items_center()
                        .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                        .when(is_selected, |el| el.bg(gpui::rgb(rungs.selected)))
                        .when(!is_selected, |el| {
                            el.hover(|s| s.bg(gpui::rgb(rungs.hover)))
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            let color = this
                                .edit_tags_page()
                                .map_or(TagColor::None, |p| p.create_color);
                            cx.emit(SearchEvent::CreateAndAttachTag {
                                tab: target_tab,
                                name: create_name.clone(),
                                color,
                            });
                            this.set_input_silently(String::new(), window, cx);
                            if let Some(p) = this.edit_tags_page_mut() {
                                p.query.clear();
                                p.selected = 0;
                                p.create_color = TagColor::None;
                            }
                            this.focus(window, cx);
                            cx.notify();
                        }))
                        .child(
                            Icon::new(IconName::Plus)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(cx.theme().foreground)
                                .child(label),
                        )
                        .child(render_tag_chip(name.clone(), create_vis, cx));

                    list = list.child(
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(action_row)
                            .child(self.render_color_palette(
                                "palette-tag-create-swatch",
                                create_color,
                                None,
                                cx,
                            )),
                    );
                }
            }
        }

        list.into_any_element()
    }
}

/// Renders a tag name inside a soft tinted chip matching the sidebar filter bar
/// so color changes preview accurately in-place.
fn render_tag_chip(name: String, vis: Option<TagVisual>, cx: &gpui::App) -> gpui::Div {
    let mut chip = h_flex()
        .min_w_0()
        .max_w_full()
        .h(px(22.))
        .px_2()
        .gap_1p5()
        .items_center()
        .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
        .border_1();

    match vis {
        Some(vis) => chip
            .bg(vis.pill_bg)
            .border_color(vis.pill_border)
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(6.5))
                    .rounded_full()
                    .bg(vis.dot),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(vis.text)
                    .child(name),
            ),
        None => {
            chip = chip
                .bg(cx.theme().muted)
                .border_color(cx.theme().border.opacity(0.65))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(6.5))
                        .rounded_full()
                        .border_1()
                        .border_color(cx.theme().muted_foreground.opacity(0.55)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(name),
                );
            chip
        }
    }
}
