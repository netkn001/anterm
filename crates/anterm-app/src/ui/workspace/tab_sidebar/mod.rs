use gpui::{
    Animation, AnimationExt as _, AnyElement, Axis, Bounds, Context, FontWeight, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollStrategy,
    SharedString, Size, Window, canvas, deferred, div, ease_out_quint, linear_color_stop,
    linear_gradient, prelude::*, px, size,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Side, Sizable as _, h_flex, v_flex, v_virtual_list,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::core::config::Config;
use crate::core::tag::{SidebarTags, TagColor, TagId};
use crate::ui::files::right_panel::RESIZE_HANDLE_WIDTH;
use crate::ui::workspace::app::{AntermApp, TITLE_BAR_BORDER, TitleStripRow as _};
use crate::ui::workspace::hints::tab_badge_label;
use crate::ui::workspace::reorder::{self, Reorder, Surface};
use crate::ui::workspace::tab_strip::{
    DragTab, REORDER_SLIDE_MS, abbreviate_home, elide_end_clusters, elide_label,
    elide_path_keep_tail, measure_text,
};
use anterm_i18n::{L10nKey, t, t_fmt};

mod layout;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod row_probe {
    use std::cell::Cell;

    thread_local! {
        static LAST_RENDERED: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn reset() {
        LAST_RENDERED.set(0);
    }

    pub(super) fn record(count: usize) {
        LAST_RENDERED.set(count);
    }

    pub(crate) fn last() -> usize {
        LAST_RENDERED.get()
    }
}

pub(crate) use layout::{TagVisual, tag_color_dot, tag_color_label, tag_visual};

#[cfg(test)]
use layout::{resolve_tab_tags, reveal_shift};
use layout::{
    ROW_CWD_FLOOR, ROW_TITLE_FLOOR, SidebarInfo, SidebarRowShown, counts_width, git_click,
    reordered_rows, row_metrics, sidebar_tag_sections, tabular,
};

enum SidebarListItem {
    SectionHeader {
        sec_ix: usize,
        tag_id: TagId,
        count: usize,
        top_gap: f32,
    },
    SectionDivider {
        top_gap: f32,
    },
    TabRow {
        sec_ix: usize,
        surface: Surface,
        row_slots: Rc<RefCell<Vec<Bounds<Pixels>>>>,
        row_preview: Option<Rc<reorder::Preview>>,
        slot: usize,
        tab_index: usize,
        row_h: f32,
        top_gap: f32,
    },
    EmptyTail,
}

struct SidebarRenderParams {
    active: usize,
    width: f32,
    show_badges: bool,
    sf: crate::ui::chrome::presets::Surface,
    badge_pos: Vec<Option<usize>>,
    font: gpui::Font,
    title_font_active: gpui::Font,
    meta_font: gpui::Font,
    rem: f32,
    rail_fill: gpui::Hsla,
    count_ink: gpui::Hsla,
    added_ink: gpui::Hsla,
    removed_ink: gpui::Hsla,
}

pub(crate) const MIN_SIDEBAR_WIDTH: f32 = 180.;

const GRAB_HANDLE_W: f32 = 48.;

const ROW_GAP: f32 = 1.;

/// Vertical breathing room between tag sections in the sidebar.
const SECTION_GAP: f32 = 10.;

/// Height of a tag section header row.
const SECTION_HEADER_HEIGHT: f32 = 24.;

/// A single-line tab row: one line of `text_sm` and a little air.
const ROW_HEIGHT: f32 = 30.;

/// A row with a git branch metadata line under its title.
const ROW_HEIGHT_TWO_LINE: f32 = 42.;

/// Metadata text size: 11.5px at the default 16px rem.
const META_REM: f32 = 11.5 / 16.;

/// The rail's side inset, for its header tiles, controls and rows alike.
const RAIL_INSET: f32 = 12.;

impl AntermApp {
    /// Whether the tab rail is on screen — the same three conditions `render`
    /// assembles the layout from, in one place the panel opposite can ask.
    pub(crate) fn sidebar_open(&self, cx: &gpui::App) -> bool {
        cx.global::<Config>().tab_bar_position == crate::core::config::TabBarPosition::Left
            && !self.tabs.is_empty()
            && !self.sidebar_collapsed
    }

    /// What the right panel has reserved, from the sidebar's point of view.
    pub(crate) fn right_panel_floor(&self, window: &Window, cx: &gpui::App) -> f32 {
        if self.right_panel_open(cx) {
            self.right_panel_min_px(window, cx)
        } else {
            0.
        }
    }

    pub(crate) fn sidebar_max_px(&self, window: &Window, cx: &gpui::App) -> f32 {
        crate::ui::workspace::app::side_panel_max(
            window.viewport_size().width.as_f32(),
            MIN_SIDEBAR_WIDTH,
            self.right_panel_floor(window, cx) + self.document_floor(cx),
        )
    }

    /// How wide the sidebar is drawn, given the live cell and the cap the rest
    /// of the window leaves it. Read here rather than clamped at each caller so
    /// the document column's budget and the sidebar itself can never disagree
    /// about how much width is already spoken for.
    pub(crate) fn sidebar_px(&self, window: &Window, cx: &gpui::App) -> f32 {
        self.sidebar_width
            .get()
            .clamp(MIN_SIDEBAR_WIDTH, self.sidebar_max_px(window, cx))
    }

    pub(crate) fn tab_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.active;
        let sf = cx.global::<crate::ui::chrome::presets::Surfaces>().rail;
        let show_badges = self.mod_hints.visible;
        let width = self.sidebar_px(window, cx);
        // Blanked here, written again from paint: a row hidden by an active
        // tag filter must leave no rectangle behind for a pane to be dropped
        // onto.
        *self.sidebar_slots.borrow_mut() = vec![Bounds::default(); self.tabs.len()];

        let filter_bar = self.render_tag_filter_bar(cx);

        let active_filter = self
            .active_tag_filter
            .filter(|id| self.sidebar_tags.contains(*id));
        let sections = sidebar_tag_sections(&self.tabs, &self.sidebar_tags, active_filter);
        let has_tagged_sections = sections.iter().any(|s| s.tag.is_some());

        // ⌘N runs ActivateTabN, which goes through `activate_visual` — the
        // Nth visible row as the sidebar lays it out.
        let badge_pos: Vec<Option<usize>> = {
            let mut pos = vec![None; self.tabs.len()];
            for (n, i) in self.visual_tab_order(cx).into_iter().enumerate() {
                if let Some(slot) = pos.get_mut(i) {
                    *slot = Some(n);
                }
            }
            pos
        };

        let pointer = window.mouse_position();
        let font = gpui::Font {
            family: cx.theme().font_family.clone(),
            features: Default::default(),
            fallbacks: None,
            weight: Default::default(),
            style: Default::default(),
        };
        let title_font_active = gpui::Font {
            weight: FontWeight::MEDIUM,
            ..font.clone()
        };
        let rem = window.rem_size().as_f32();
        let meta_font = gpui::Font {
            features: tabular(),
            ..font.clone()
        };
        let rail_fill: gpui::Hsla = gpui::rgb(sf.base).into();
        let count_ink = cx.theme().muted_foreground;
        let added_ink = crate::ui::chrome::presets::resting_ink(
            cx.theme().success,
            cx.theme().muted_foreground,
            rail_fill,
        );
        let removed_ink = crate::ui::chrome::presets::resting_ink(
            cx.theme().danger,
            cx.theme().muted_foreground,
            rail_fill,
        );

        let list_body: AnyElement = if let Some(filter_id) = active_filter
            && sections.is_empty()
        {
            self.sidebar_reveal.set(None);
            let tag_name = self
                .sidebar_tags
                .get(filter_id)
                .map(|tag| tag.name.as_str())
                .unwrap_or("");
            v_flex()
                .id("tab-sidebar-list")
                .flex_1()
                .min_h_0()
                .w_full()
                .px_3()
                .pt(px(6.))
                .pb(px(20.))
                .child(
                    v_flex()
                        .id("sidebar-tag-filter-empty")
                        .w_full()
                        .py_6()
                        .px_2()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(t_fmt(
                                    L10nKey::SidebarTagFilterEmpty,
                                    &[("name", tag_name)],
                                )),
                        )
                        .child(
                            Button::new("sidebar-tag-show-all")
                                .small()
                                .outline()
                                .label(t(L10nKey::SidebarTagShowAll))
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.clear_tag_filter(cx);
                                })),
                        ),
                )
                .into_any_element()
        } else {
            let reveal = self.sidebar_reveal.get();
            let mut reveal_ix: Option<usize> = None;
            let mut items: Vec<SidebarListItem> = Vec::with_capacity(self.tabs.len() + sections.len() + 1);
            let mut item_sizes: Vec<Size<Pixels>> = Vec::with_capacity(self.tabs.len() + sections.len() + 1);

            for (sec_ix, section) in sections.iter().enumerate() {
                let surface = Surface::SidebarRows(section.tag);
                let row_slots: Rc<RefCell<Vec<Bounds<Pixels>>>> =
                    Rc::new(RefCell::new(vec![Bounds::default(); section.tabs.len()]));
                let row_preview = reorder::preview(
                    &self.reorder,
                    &surface,
                    section.tabs.len(),
                    pointer,
                )
                .map(Rc::new);

                let sec_gap = if sec_ix > 0 { SECTION_GAP } else { 0. };
                let mut first_in_section = true;

                if let Some(tag_id) = section.tag
                    && self.sidebar_tags.get(tag_id).is_some()
                {
                    items.push(SidebarListItem::SectionHeader {
                        sec_ix,
                        tag_id,
                        count: section.tabs.len(),
                        top_gap: sec_gap,
                    });
                    item_sizes.push(size(px(0.), px(SECTION_HEADER_HEIGHT + sec_gap)));
                    first_in_section = false;
                } else if has_tagged_sections {
                    items.push(SidebarListItem::SectionDivider { top_gap: sec_gap });
                    item_sizes.push(size(px(0.), px(5. + sec_gap)));
                    first_in_section = false;
                }

                let row_display: Vec<usize> = match &row_preview {
                    Some(p) => {
                        if let Some(order) =
                            reordered_rows(self.tabs.len(), &section.tabs, p.from, p.target)
                        {
                            reorder::set_pending(&self.reorder, &surface, order);
                        }
                        p.order.clone()
                    }
                    None => (0..section.tabs.len()).collect(),
                };

                for slot in row_display {
                    let tab_index = section.tabs[slot];
                    let has_git = self.tabs[tab_index].git_status(Some(window), cx).is_some();
                    let row_h = if has_git {
                        ROW_HEIGHT_TWO_LINE
                    } else {
                        ROW_HEIGHT
                    };
                    let top_gap = if first_in_section {
                        first_in_section = false;
                        sec_gap
                    } else {
                        ROW_GAP
                    };
                    if reveal == Some(self.tabs[tab_index].tree_id.get()) {
                        reveal_ix = Some(items.len());
                    }
                    items.push(SidebarListItem::TabRow {
                        sec_ix,
                        surface: surface.clone(),
                        row_slots: row_slots.clone(),
                        row_preview: row_preview.clone(),
                        slot,
                        tab_index,
                        row_h,
                        top_gap,
                    });
                    item_sizes.push(size(px(0.), px(row_h + top_gap)));
                }
            }

            items.push(SidebarListItem::EmptyTail);
            item_sizes.push(size(px(0.), px(ROW_HEIGHT)));

            if let Some(ix) = reveal_ix {
                self.sidebar_scroll
                    .scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            self.sidebar_reveal.set(None);

            let items = Rc::new(items);
            let params = Rc::new(SidebarRenderParams {
                active,
                width,
                show_badges,
                sf,
                badge_pos,
                font,
                title_font_active,
                meta_font,
                rem,
                rail_fill,
                count_ink,
                added_ink,
                removed_ink,
            });
            let list = v_virtual_list(
                cx.entity(),
                "tab-sidebar-list",
                Rc::new(item_sizes),
                move |this, range, window, cx| {
                    #[cfg(test)]
                    row_probe::record(range.len());
                    range
                        .filter_map(|ix| items.get(ix))
                        .map(|item| this.render_sidebar_item(item, &params, window, cx))
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.sidebar_scroll)
            .flex_1()
            .w_full()
            .pt(px(6.))
            .pb(px(20.));

            crate::ui::chrome::scrollbar::with_vertical_scrollbar(
                "tab-sidebar-scrollbar",
                list,
                &self.sidebar_scroll,
            )
        };

        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        let chrome_shown = crate::ui::workspace::app::titlebar_chrome_shown(
            auto_hide_chrome,
            self.sidebar_chrome_hover.get(),
        );
        let controls = h_flex()
            .flex_shrink_0()
            .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
            .justify_end()
            .gap(px(2.))
            .pr(px(RAIL_INSET))
            .when_some(crate::ui::workspace::app::window_mark(), |row, mark| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .pl(px(crate::ui::workspace::app::CONTENT_INSET))
                        .child(mark),
                )
                .child(div().flex_1().min_w(px(GRAB_HANDLE_W)))
            })
            .child(
                crate::ui::workspace::app::resting_chrome(
                    div().occlude().flex_shrink_0(),
                    chrome_shown,
                )
                .child(self.new_tab_button("sidebar-add", cx)),
            )
            .child(
                crate::ui::workspace::app::resting_chrome(
                    div().occlude().flex_shrink_0(),
                    chrome_shown,
                )
                .child(
                    crate::ui::workspace::tab_strip::chrome_tile(
                        Button::new("sidebar-collapse")
                            .icon(Icon::empty().path("icons/panel-left.svg")),
                        false,
                        cx,
                    )
                    .rounded_lg()
                    .accessibility_label(t(L10nKey::TabTooltipHideSidebar))
                    .tooltip_with_action(
                        t(L10nKey::TabTooltipHideSidebar),
                        &crate::core::actions::ToggleLeftPanel,
                        None,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| this.toggle_left_panel(cx))),
                ),
            );

        let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
        let others_floor = self.right_panel_floor(window, cx) + self.document_floor(cx);
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| {
                    container.set(Some(bounds));
                }
            },
            {
                let container = container.clone();
                let width_cell = self.sidebar_width.clone();
                let dragging = self.sidebar_dragging.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |ev: &MouseMoveEvent, _phase, window, _cx| {
                            if !dragging.get() {
                                return;
                            }
                            let Some(b) = container.get() else {
                                return;
                            };
                            let raw = (ev.position.x - b.origin.x).as_f32();
                            let max = crate::ui::workspace::app::side_panel_max(
                                window.viewport_size().width.as_f32(),
                                MIN_SIDEBAR_WIDTH,
                                others_floor,
                            );
                            width_cell.set(raw.clamp(MIN_SIDEBAR_WIDTH, max));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let width_cell = width_cell.clone();
                        let dragging = dragging.clone();
                        move |_ev: &MouseUpEvent, _phase, window, cx| {
                            if !dragging.get() {
                                return;
                            }
                            dragging.set(false);
                            let w = width_cell.get();
                            let cfg = cx.global_mut::<Config>();
                            if cfg.sidebar_width != w {
                                cfg.sidebar_width = w;
                                cfg.save();
                            }
                            window.refresh();
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full();

        let handle_active = self.sidebar_dragging.get();
        let handle = div()
            .group("sidebar-resize")
            .occlude()
            .absolute()
            .top_0()
            .right(px(-(RESIZE_HANDLE_WIDTH / 2.)))
            .w(px(RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .when(handle_active, |d| d.bg(cx.theme().drag_border))
                    .group_hover("sidebar-resize", |s| s.bg(cx.theme().drag_border)),
            )
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.sidebar_dragging.clone();
                move |_ev, window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            });

        div()
            .relative()
            .flex_shrink_0()
            .w(px(width))
            .h_full()
            .bg(crate::ui::chrome::theme::rail_surface_color(cx))
            .border_r(crate::ui::chrome::theme::hairline(window))
            .border_color(cx.theme().sidebar_border)
            .child(backing)
            .child(
                v_flex()
                    .w(px(width - 1.))
                    .h_full()
                    .child(crate::ui::workspace::app::title_bar_drag(
                        controls.id("sidebar-titlebar-drag"),
                        "sidebar-titlebar-drag",
                        window,
                        cx,
                    ))
                    .children(filter_bar)
                    .child(list_body),
            )
            .child(handle)
            .when(auto_hide_chrome, |rail| {
                rail.child(crate::ui::workspace::app::hover_sheet(
                    "sidebar-chrome-hover",
                    &self.sidebar_chrome_hover,
                ))
            })
    }

    fn render_sidebar_item(
        &self,
        item: &SidebarListItem,
        p: &SidebarRenderParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            SidebarListItem::SectionHeader {
                sec_ix,
                tag_id,
                count,
                top_gap,
            } => {
                let Some(tag) = self.sidebar_tags.get(*tag_id) else {
                    return div().into_any_element();
                };
                let tag_id = *tag_id;
                let tag_color = tag.color;
                let vis = tag_visual(tag_color, cx);
                let sf = p.sf;
                let header = h_flex()
                    .id(("sidebar-tag-section-head", *sec_ix))
                    .w_full()
                    .h(px(SECTION_HEADER_HEIGHT))
                    .px_2()
                    .gap_1p5()
                    .items_center()
                    .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                    .hover(move |s| {
                        s.bg(vis.map_or_else(
                            || gpui::Hsla::from(gpui::rgb(sf.hover)).opacity(0.6),
                            |v| v.pill_bg,
                        ))
                    })
                    .when_some(vis, |h, v| {
                        h.child(
                            div()
                                .flex_shrink_0()
                                .w(px(3.))
                                .h(px(11.))
                                .rounded_full()
                                .bg(v.dot),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(
                                vis.map_or_else(|| gpui::rgb(sf.text_selected).into(), |v| v.text),
                            )
                            .child(tag.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(META_REM * p.rem))
                            .font_features(tabular())
                            .text_color(cx.theme().muted_foreground.opacity(0.8))
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.toggle_tag_filter(tag_id, cx);
                    }));
                let app = cx.entity().downgrade();
                div()
                    .w_full()
                    .px_3()
                    .pt(px(*top_gap))
                    .child(header.context_menu(move |menu, window, cx| {
                        tag_pill_menu(menu, tag_id, tag_color, &app, window, cx)
                    }))
                    .into_any_element()
            }
            SidebarListItem::SectionDivider { top_gap } => div()
                .w_full()
                .px_3()
                .pt(px(*top_gap))
                .child(
                    div()
                        .w_full()
                        .px_2()
                        .pb_1()
                        .child(div().w_full().h(px(1.)).bg(cx.theme().sidebar_border.opacity(0.6))),
                )
                .into_any_element(),
            SidebarListItem::EmptyTail => div()
                .w_full()
                .px_3()
                .child(
                    div()
                        .id("tab-sidebar-empty")
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .context_menu({
                            let app = cx.entity().downgrade();
                            move |menu, _window, _cx| {
                                let new_tab = app.clone();
                                menu.min_w(px(200.)).item(
                                    PopupMenuItem::new(t(L10nKey::AppMenuNewTab)).on_click(
                                        move |_, window, cx| {
                                            let _ = new_tab
                                                .update(cx, |this, cx| this.new_tab(window, cx));
                                        },
                                    ),
                                )
                            }
                        }),
                )
                .into_any_element(),
            SidebarListItem::TabRow {
                sec_ix,
                surface,
                row_slots,
                row_preview,
                slot,
                tab_index,
                row_h,
                top_gap,
            } => {
                let i = *tab_index;
                let slot = *slot;
                let Some(tab) = self.tabs.get(i) else {
                    return div().into_any_element();
                };
                let badge_idx = p.badge_pos.get(i).copied().flatten();
                let show_row_badge = p.show_badges && badge_idx.is_some_and(|n| n < 9);
                let is_active = i == p.active;
                let sf = p.sf;
                let ssh_dot = self.tab_ssh_dot(tab, cx);
                let asleep = tab.is_asleep();
                let agent = tab.agent(cx).or_else(|| tab.asleep_agent());
                let agent_status = tab.agent_status(cx);
                let agent_unread = tab.agent_unread_count(cx);
                let git_cwd = git_click(tab, window, cx);
                let badge_extra = if show_row_badge {
                    row_metrics::BADGE + row_metrics::GAP
                } else {
                    0.
                };
                let zoomed = self.tab_is_zoomed(i);
                let zoom_extra = if zoomed {
                    row_metrics::ZOOM + row_metrics::GAP
                } else {
                    0.
                };
                let zoom_extra = if asleep {
                    zoom_extra + row_metrics::ZOOM + row_metrics::GAP
                } else {
                    zoom_extra
                };
                let status_dot = match show_row_badge {
                    true => None,
                    false => self.row_status_dot(agent_status, agent_unread, p.rail_fill),
                };
                let status_extra = match status_dot.is_some() {
                    true => row_metrics::STATUS + row_metrics::GAP,
                    false => 0.,
                };
                let label_avail =
                    (row_metrics::text_budget(p.width) - badge_extra - zoom_extra - status_extra)
                        .max(48.);
                let title_size = 0.875 * p.rem;
                let meta_size = META_REM * p.rem;
                let title_font = if is_active {
                    &p.title_font_active
                } else {
                    &p.font
                };

                let (title_text, full_title) = match self.full_tab_label(tab, Some(window), cx) {
                    Some(raw) => {
                        let full = SharedString::from(raw);
                        (full.clone(), Some(full))
                    }
                    None => {
                        let placeholder = SharedString::from(t_fmt(
                            L10nKey::TabUnnamedShell,
                            &[("n", &((i + 1).to_string()))],
                        ));
                        (placeholder, None)
                    }
                };

                let mut branch_shown: Option<(SharedString, SharedString, u32, u32)> = None;
                let mut cwd_shown: Option<(SharedString, SharedString)> = None;
                let git_line = tab.git_status(Some(window), cx).map(|g| {
                    let mut line = h_flex()
                        .id(("sidebar-git", i))
                        .w_full()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(meta_size))
                        .text_color(cx.theme().muted_foreground)
                        .font_features(tabular());
                    let counts_w =
                        counts_width(&window.text_system(), &p.meta_font, meta_size, &g);
                    let branch_avail = (label_avail - counts_w).max(0.);
                    let shown = elide_end_clusters(
                        &window.text_system(),
                        &p.meta_font,
                        meta_size,
                        &g.branch,
                        branch_avail,
                    );
                    branch_shown = Some((
                        shown.clone(),
                        SharedString::from(g.branch.clone()),
                        g.added,
                        g.removed,
                    ));
                    line = line.child(div().flex_1().min_w_0().truncate().child(shown));
                    if g.added > 0 || g.removed > 0 {
                        let mut counts = h_flex()
                            .id(("sidebar-diff", i))
                            .flex_shrink_0()
                            .items_center()
                            .gap_1p5()
                            .when_some(git_cwd, |counts, (host, cwd)| {
                                counts
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            cx.stop_propagation();
                                            this.activate(i, window, cx);
                                            this.toggle_diff_overlay(host, cwd.clone(), window, cx);
                                        }),
                                    )
                            });
                        if g.added > 0 {
                            counts = counts.child(
                                div()
                                    .text_color(p.count_ink)
                                    .child(format!("+{}", g.added)),
                            );
                        }
                        if g.removed > 0 {
                            counts = counts.child(
                                div()
                                    .text_color(p.count_ink)
                                    .child(format!("−{}", g.removed)),
                            );
                        }
                        line = line.child(counts);
                    }
                    line
                });

                let cwd_full: Option<SharedString> = match git_line.is_none() {
                    false => None,
                    true => tab
                        .pane
                        .focused_or_first(window, cx)
                        .and_then(|leaf| {
                            let leaf = leaf.read(cx);
                            Some((leaf.effective_cwd()?, leaf.display_home(cx)))
                        })
                        .map(|(cwd, home)| {
                            let text = cwd.display().to_string();
                            SharedString::from(
                                abbreviate_home(&text, home.as_deref()).into_owned(),
                            )
                        })
                        .filter(|full| full.as_ref() != title_text.as_ref()),
                };
                let cwd_want = cwd_full.as_ref().map(|full| {
                    row_metrics::META_GAP
                        + measure_text(&window.text_system(), &p.font, meta_size, full)
                });
                let title_avail = match cwd_want {
                    Some(want) => {
                        (label_avail - want.min(label_avail * 0.5)).max(ROW_TITLE_FLOOR)
                    }
                    None => label_avail,
                };
                let shown_title = elide_label(
                    &window.text_system(),
                    title_font,
                    title_size,
                    &title_text,
                    title_avail,
                );
                if let Some(full) = cwd_full {
                    let avail = (label_avail
                        - measure_text(&window.text_system(), title_font, title_size, &shown_title)
                        - row_metrics::META_GAP)
                        .max(0.);
                    if avail >= ROW_CWD_FLOOR {
                        let shown = elide_path_keep_tail(
                            &window.text_system(),
                            &p.font,
                            meta_size,
                            &full,
                            avail,
                        );
                        cwd_shown = Some((shown, full));
                    }
                }

                let rename_input = self
                    .renaming
                    .as_ref()
                    .filter(|r| r.tab == tab.tree_id.get())
                    .map(|r| r.input.clone());

                let shown = SidebarRowShown {
                    title: full_title.map(|full| (shown_title.clone(), full)),
                    branch: branch_shown.clone(),
                    cwd: cwd_shown.clone(),
                };
                let info = self.sidebar_info(tab, window, cx, &shown);
                let muted = cx.theme().muted_foreground;
                let success = p.added_ink;
                let danger = p.removed_ink;

                let label_region = match rename_input {
                    Some(input) => div()
                        .id(("sidebar-rename", i))
                        .flex_1()
                        .min_w_0()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(Input::new(&input).appearance(false).px_0())
                        .into_any_element(),
                    None => v_flex()
                        .id(("sidebar-label", i))
                        .flex_1()
                        .min_w_0()
                        .justify_center()
                        .gap(px(1.))
                        .when_some(info, |col, info| {
                            col.tooltip(move |window, cx| {
                                let info = info.clone();
                                gpui_kit::component::tooltip::Tooltip::element(
                                    move |_window, _cx| {
                                        v_flex()
                                            .gap_1()
                                            .when_some(info.title.clone(), |c, title| {
                                                c.child(
                                                    div()
                                                        .max_w(px(420.))
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .child(title),
                                                )
                                            })
                                            .when_some(
                                                info.branch.clone(),
                                                |c, (branch, added, removed)| {
                                                    let mut line = h_flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(
                                                            gpui::svg()
                                                                .path("icons/git-branch.svg")
                                                                .flex_shrink_0()
                                                                .size(px(11.))
                                                                .text_color(muted),
                                                        )
                                                        .child(div().child(branch));
                                                    if added > 0 {
                                                        line = line.child(
                                                            div()
                                                                .text_color(success)
                                                                .child(format!("+{added}")),
                                                        );
                                                    }
                                                    if removed > 0 {
                                                        line = line.child(
                                                            div()
                                                                .text_color(danger)
                                                                .child(format!("−{removed}")),
                                                        );
                                                    }
                                                    c.child(line)
                                                },
                                            )
                                            .when_some(info.cwd.clone(), |c, cwd| {
                                                c.child(
                                                    div()
                                                        .max_w(px(420.))
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(cwd),
                                                )
                                            })
                                            .when_some(info.host.clone(), |c, host| {
                                                c.child(
                                                    h_flex()
                                                        .items_center()
                                                        .gap_1p5()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(
                                                            gpui::svg()
                                                                .path("icons/machine-remote.svg")
                                                                .flex_shrink_0()
                                                                .size(px(11.))
                                                                .text_color(muted),
                                                        )
                                                        .child(div().truncate().child(host)),
                                                )
                                            })
                                    },
                                )
                                .build(window, cx)
                            })
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .when(is_active, |d| d.font_weight(FontWeight::MEDIUM))
                                        .child(shown_title),
                                )
                                .when_some(cwd_shown.map(|(cwd, _)| cwd), |line, cwd| {
                                    line.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(cwd),
                                    )
                                }),
                        )
                        .children(git_line)
                        .into_any_element(),
                };

                let row = h_flex()
                    .id(("tab-row", i))
                    .role(gpui::Role::Tab)
                    .aria_label(SharedString::from(title_text.to_string()))
                    .aria_selected(is_active)
                    .group(SharedString::from(format!("tab-row-{i}")))
                    .on_drag(DragTab, {
                        let state = self.reorder.clone();
                        let slots = row_slots.clone();
                        let id = tab.tree_id.get();
                        let drag_surface = surface.clone();
                        move |_drag, grab, _window, cx| {
                            cx.stop_propagation();
                            *state.borrow_mut() = Some(
                                Reorder::new(
                                    drag_surface.clone(),
                                    slot,
                                    slots.borrow().clone(),
                                    Axis::Vertical,
                                    px(ROW_GAP),
                                    grab,
                                )
                                .of_tab(id),
                            );
                            cx.new(|_| DragTab)
                        }
                    })
                    .w_full()
                    .h(px(*row_h))
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap(px(row_metrics::GAP))
                    .px_2()
                    .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                    .when(is_active, |s| {
                        s.bg(gpui::rgb(sf.selected))
                            .text_color(gpui::rgb(sf.text_selected))
                    })
                    .when(!is_active, |s| {
                        s.text_color(gpui::rgb(sf.text_resting))
                            .hover(|s| s.bg(gpui::rgb(sf.hover)))
                    })
                    .when(asleep, |s| s.opacity(0.6))
                    .when(
                        row_preview.as_ref().is_some_and(|pr| pr.from == slot),
                        |s| s.opacity(0.75),
                    )
                    .child(
                        canvas(
                            {
                                let slots = row_slots.clone();
                                let by_tab = self.sidebar_slots.clone();
                                move |bounds, _window, _cx| {
                                    if let Some(s) = slots.borrow_mut().get_mut(slot) {
                                        *s = bounds;
                                    }
                                    if let Some(s) = by_tab.borrow_mut().get_mut(i) {
                                        *s = bounds;
                                    }
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.activate(i, window, cx);
                    }))
                    .child(self.tab_avatar_plain(
                        ("sidebar-avatar", i),
                        agent,
                        agent_status,
                        ssh_dot,
                        row_metrics::AVATAR,
                        cx,
                    ))
                    .when(zoomed, |row| {
                        row.child(self.zoom_mark(("sidebar-zoom", i), cx))
                    })
                    .when(asleep, |row| {
                        row.child(self.sleep_mark(("sidebar-asleep", i), cx))
                    })
                    .child(label_region)
                    .children(status_dot)
                    .when(show_row_badge, |row| {
                        let pos = badge_idx.unwrap_or(0);
                        row.child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(20.))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if is_active {
                                    gpui::rgb(sf.text_selected).into()
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(tab_badge_label(pos)),
                        )
                    })
                    .when(!show_row_badge, |row| {
                        let backing: gpui::Hsla = if is_active {
                            gpui::rgb(sf.selected).into()
                        } else {
                            gpui::rgb(sf.hover).into()
                        };
                        let mut fade_from = backing;
                        fade_from.a = 0.;
                        row.child(
                            h_flex()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right(px(6.))
                                .opacity(0.)
                                .group_hover(SharedString::from(format!("tab-row-{i}")), |s| {
                                    s.opacity(1.)
                                })
                                .child(div().w(px(10.)).h_full().bg(linear_gradient(
                                    90.,
                                    linear_color_stop(fade_from, 0.),
                                    linear_color_stop(backing, 1.),
                                )))
                                .child(
                                    div().h_full().flex().items_center().bg(backing).child(
                                        crate::ui::workspace::tab_strip::hit_target(
                                            Button::new(("sidebar-close", i))
                                                .icon(IconName::Close)
                                                .ghost()
                                                .xsmall(),
                                        )
                                        .tooltip(t(L10nKey::TabContextCloseTab))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.close_tab(i, window, cx);
                                        })),
                                    ),
                                ),
                        )
                    });

                let menu_app = cx.entity().downgrade();
                let menu_row = row.context_menu(move |menu, window, cx| {
                    AntermApp::tab_context_menu(menu, i, true, &menu_app, window, cx)
                });
                let inner: AnyElement = match row_preview {
                    Some(pr) if pr.from == slot => {
                        deferred(menu_row.relative().top(pr.held)).into_any_element()
                    }
                    Some(pr) => {
                        let offset = pr.offsets[slot].as_f32();
                        menu_row
                            .with_animation(
                                (
                                    SharedString::from(format!(
                                        "row-slide-{sec_ix}-{}",
                                        pr.generation
                                    )),
                                    slot,
                                ),
                                Animation::new(std::time::Duration::from_millis(
                                    REORDER_SLIDE_MS,
                                ))
                                .with_easing(ease_out_quint()),
                                move |el, delta| el.top(px(offset * (1. - delta))),
                            )
                            .into_any_element()
                    }
                    None => menu_row.into_any_element(),
                };
                div()
                    .w_full()
                    .px_3()
                    .pt(px(*top_gap))
                    .child(inner)
                    .into_any_element()
            }
        }
    }

    /// Compact wrapping tag filter bar above the tab list. Completely absent
    /// (zero visual weight) when no tags exist in `self.sidebar_tags`.
    fn render_tag_filter_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.sidebar_tags.is_empty() {
            return None;
        }
        let sf = cx.global::<crate::ui::chrome::presets::Surfaces>().rail;
        let active_filter = self
            .active_tag_filter
            .filter(|id| self.sidebar_tags.contains(*id));
        let all_selected = active_filter.is_none();

        let mut bar = h_flex()
            .id("sidebar-tag-filter-bar")
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .gap_1()
            .mx_3()
            .pt_1p5()
            .pb_2()
            .mb_1()
            .border_b_1()
            .border_color(cx.theme().sidebar_border.opacity(0.6));

        let chip_radius = crate::ui::chrome::rounding::ROW_RADIUS;

        let all_pill = h_flex()
            .id("sidebar-tag-pill-all")
            .h(px(22.))
            .px_2()
            .items_center()
            .rounded(chip_radius)
            .border_1()
            .text_xs()
            .when(all_selected, |s| {
                s.bg(gpui::rgb(sf.selected))
                    .border_color(cx.theme().sidebar_border.opacity(0.85))
                    .text_color(gpui::rgb(sf.text_selected))
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!all_selected, |s| {
                s.border_color(gpui::transparent_black())
                    .text_color(cx.theme().muted_foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| {
                        s.bg(gpui::rgb(sf.hover))
                            .text_color(gpui::rgb(sf.text_resting))
                    })
            })
            .on_click(cx.listener(|this, _, _window, cx| {
                this.clear_tag_filter(cx);
            }))
            .child(t(L10nKey::SidebarTagsAll));

        bar = bar.child(all_pill);

        for (ix, tag) in self.sidebar_tags.tags.iter().enumerate() {
            let tag_id = tag.id;
            let tag_color = tag.color;
            let renaming = self
                .tag_rename
                .as_ref()
                .filter(|r| r.tag == tag_id)
                .map(|r| r.input.clone());

            if let Some(input) = renaming {
                bar = bar.child(
                    h_flex()
                        .id(("sidebar-tag-rename", ix))
                        .h(px(22.))
                        .w(px(96.))
                        .px_2()
                        .items_center()
                        .rounded(chip_radius)
                        .bg(gpui::rgb(sf.selected))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                cx.stop_propagation();
                                this.cancel_tag_rename(window, cx);
                            }
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&input).appearance(false).px_0().xsmall()),
                        ),
                );
                continue;
            }

            let selected = active_filter == Some(tag_id);
            let vis = tag_visual(tag_color, cx);
            let mut pill = h_flex()
                .id(("sidebar-tag-pill", ix))
                .h(px(22.))
                .max_w(px(140.))
                .px_1p5()
                .gap_1p5()
                .items_center()
                .rounded(chip_radius)
                .border_1()
                .text_xs()
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                });

            pill = match vis {
                Some(vis) => pill
                    .when(selected, |s| {
                        s.bg(vis.pill_bg_selected)
                            .border_color(vis.pill_border_selected)
                            .text_color(vis.text_selected)
                    })
                    .when(!selected, |s| {
                        s.bg(vis.pill_bg)
                            .border_color(gpui::transparent_black())
                            .text_color(vis.text)
                            .hover(|s| s.bg(vis.pill_bg_hover))
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(6.))
                            .rounded_full()
                            .bg(vis.dot),
                    ),
                None => pill
                    .when(selected, |s| {
                        s.bg(gpui::rgb(sf.selected))
                            .border_color(cx.theme().sidebar_border.opacity(0.85))
                            .text_color(gpui::rgb(sf.text_selected))
                    })
                    .when(!selected, |s| {
                        s.bg(gpui::rgb(sf.hover).opacity(0.55))
                            .border_color(gpui::transparent_black())
                            .text_color(cx.theme().muted_foreground)
                            .hover(|s| {
                                s.bg(gpui::rgb(sf.hover))
                                    .text_color(gpui::rgb(sf.text_resting))
                            })
                    }),
            };

            let pill = pill
                .child(div().min_w_0().truncate().child(tag.name.clone()))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.toggle_tag_filter(tag_id, cx);
                }));

            let app = cx.entity().downgrade();
            bar = bar.child(pill.context_menu(move |menu, window, cx| {
                tag_pill_menu(menu, tag_id, tag_color, &app, window, cx)
            }));
        }

        Some(bar.into_any_element())
    }

    /// What the sidebar row hid: the full title, the full branch and diff
    /// counts, the working directory, and the remote host.
    fn sidebar_info(
        &self,
        tab: &crate::ui::workspace::app::Tab,
        window: &mut Window,
        cx: &gpui::App,
        shown: &SidebarRowShown,
    ) -> Option<SidebarInfo> {
        let elided = |pair: &Option<(SharedString, SharedString)>| {
            pair.as_ref()
                .filter(|(shown, full)| shown != full)
                .map(|(_, full)| full.clone())
        };
        let mut info = SidebarInfo {
            title: elided(&shown.title),
            branch: shown
                .branch
                .as_ref()
                .filter(|(shown, full, _, _)| shown != full)
                .map(|(_, full, added, removed)| (full.clone(), *added, *removed)),
            cwd: elided(&shown.cwd),
            host: None,
        };
        if let Some(target) = tab.pane.focused_or_first(window, cx).and_then(|leaf| {
            leaf.read(cx)
                .remote_context()
                .map(|r| SharedString::from(r.target.clone()))
        }) {
            info.host = Some(target);
        }
        (info.title.is_some()
            || info.branch.is_some()
            || info.cwd.is_some()
            || info.host.is_some())
        .then_some(info)
    }

    /// Change this workspace's tag catalog, and send the change to the machine
    /// so every other window onto the workspace sees it too.
    pub(crate) fn edit_tags(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut SidebarTags),
    ) {
        edit(&mut self.sidebar_tags);
        crate::ui::remote::tree_sync::push_tags(cx, self.sidebar_tags.clone());
        self.save_session(cx);
        cx.notify();
    }

    /// Adopt the machine's tag catalog from a pull or `TagsChanged` delta.
    pub(crate) fn adopt_sidebar_tags(&mut self, tags: SidebarTags, cx: &mut Context<Self>) {
        if self.sidebar_tags == tags {
            return;
        }
        self.sidebar_tags = tags;
        if let Some(active) = self.active_tag_filter
            && !self.sidebar_tags.contains(active)
        {
            self.active_tag_filter = None;
        }
        if self
            .tag_rename
            .as_ref()
            .is_some_and(|r| !self.sidebar_tags.contains(r.tag))
        {
            self.tag_rename = None;
        }
        cx.notify();
    }

    /// Attach or detach `tag_id` on tab `index`.
    pub(crate) fn toggle_tab_tag(
        &mut self,
        index: usize,
        tag_id: TagId,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let mut tags = tab.tags.borrow_mut();
        if let Some(pos) = tags.iter().position(|id| *id == tag_id) {
            tags.remove(pos);
        } else if self.sidebar_tags.contains(tag_id) {
            tags.push(tag_id);
        } else {
            return;
        }
        drop(tags);
        self.save_session(cx);
        cx.notify();
    }

    /// Create (or find case-insensitively) a tag named `name` with `color` and
    /// attach it to tab `index`.
    pub(crate) fn create_and_attach_tag(
        &mut self,
        index: usize,
        name: &str,
        color: TagColor,
        cx: &mut Context<Self>,
    ) -> Option<TagId> {
        let id = self.sidebar_tags.create(name, color)?;
        if let Some(tab) = self.tabs.get(index) {
            let mut tags = tab.tags.borrow_mut();
            if !tags.contains(&id) {
                tags.push(id);
            }
        }
        crate::ui::remote::tree_sync::push_tags(cx, self.sidebar_tags.clone());
        self.save_session(cx);
        cx.notify();
        Some(id)
    }

    /// Rename tag `id` to `name`. Rejects blank names or case-insensitive
    /// collisions with another tag.
    pub(crate) fn rename_tag(&mut self, id: TagId, name: &str, cx: &mut Context<Self>) -> bool {
        if !self.sidebar_tags.rename(id, name) {
            return false;
        }
        crate::ui::remote::tree_sync::push_tags(cx, self.sidebar_tags.clone());
        self.save_session(cx);
        cx.notify();
        true
    }

    /// Update tag `id`'s semantic dot color.
    pub(crate) fn set_tag_color(&mut self, id: TagId, color: TagColor, cx: &mut Context<Self>) {
        if !self.sidebar_tags.set_color(id, color) {
            return;
        }
        crate::ui::remote::tree_sync::push_tags(cx, self.sidebar_tags.clone());
        self.save_session(cx);
        cx.notify();
    }

    /// Delete tag `id` from the catalog and detach it from every tab. Never
    /// closes any tab. Clears the active filter if it pointed at `id`.
    pub(crate) fn delete_tag(&mut self, id: TagId, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.tags.borrow_mut().retain(|t| *t != id);
        }
        if self.active_tag_filter == Some(id) {
            self.active_tag_filter = None;
        }
        if self.tag_rename.as_ref().is_some_and(|r| r.tag == id) {
            self.tag_rename = None;
        }
        self.edit_tags(cx, |tags| {
            tags.remove(id);
        });
    }

    /// Single-select filter toggle: clicking an inactive tag filters to it;
    /// clicking the active tag again clears the filter.
    pub(crate) fn toggle_tag_filter(&mut self, id: TagId, cx: &mut Context<Self>) {
        if self.active_tag_filter == Some(id) {
            self.active_tag_filter = None;
        } else if self.sidebar_tags.contains(id) {
            self.active_tag_filter = Some(id);
        }
        cx.notify();
    }

    /// Clear any active sidebar tag filter.
    pub(crate) fn clear_tag_filter(&mut self, cx: &mut Context<Self>) {
        if self.active_tag_filter.take().is_some() {
            cx.notify();
        }
    }

    /// Switch the active tag filter to `id` (or `None` to show all tabs), and
    /// when a specific tag is chosen and the active tab does not carry it,
    /// activate the most recently used tab tagged with `id`.
    pub(crate) fn switch_tag(
        &mut self,
        id: Option<TagId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tag_id) = id else {
            self.clear_tag_filter(cx);
            return;
        };
        if !self.sidebar_tags.contains(tag_id) {
            return;
        }
        self.active_tag_filter = Some(tag_id);
        let active_matches = self
            .tabs
            .get(self.active)
            .is_some_and(|t| t.tags.borrow().contains(&tag_id));
        if !active_matches
            && let Some(target) = (0..self.tabs.len())
                .filter(|&i| self.tabs[i].tags.borrow().contains(&tag_id))
                .max_by_key(|&i| (self.tabs[i].last_used.get(), std::cmp::Reverse(i)))
        {
            self.activate(target, window, cx);
        }
        cx.notify();
    }

    /// Open inline rename for tag pill `id`.
    pub(crate) fn start_tag_rename(
        &mut self,
        id: TagId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.sidebar_tags.get(id).map(|t| t.name.clone()) else {
            return;
        };
        let input = Self::rename_box(current, window, cx);
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this, _input, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.commit_tag_rename(window, cx)
                }
                _ => {}
            },
        )];
        self.tag_rename = Some(crate::ui::workspace::app::TagRename {
            tag: id,
            input,
            _subs: subs,
        });
        cx.notify();
    }

    /// Commit the inline rename for the tag being edited.
    pub(crate) fn commit_tag_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.tag_rename.take() else {
            return;
        };
        let value = rename.input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            self.rename_tag(rename.tag, &value, cx);
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Cancel inline tag rename without saving.
    pub(crate) fn cancel_tag_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tag_rename.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Visual tab order for `⌘1..9` and `SelectNextTab`/`SelectPrevTab`:
    /// follows the tag-grouped sidebar sections when the left sidebar is active
    /// and the filter matches at least one tab; otherwise all tabs in flat
    /// order.
    pub(crate) fn visual_tab_order(&self, cx: &gpui::App) -> Vec<usize> {
        if cx.global::<Config>().tab_bar_position != crate::core::config::TabBarPosition::Left {
            return (0..self.tabs.len()).collect();
        }
        let active_filter = self
            .active_tag_filter
            .filter(|id| self.sidebar_tags.contains(*id));
        let ordered: Vec<usize> =
            sidebar_tag_sections(&self.tabs, &self.sidebar_tags, active_filter)
                .into_iter()
                .flat_map(|s| s.tabs)
                .collect();
        if ordered.is_empty() {
            (0..self.tabs.len()).collect()
        } else {
            ordered
        }
    }

    /// The run of tabs `MoveTabLeft`/`MoveTabRight` walks across: stays inside
    /// `index`'s tag section when the left sidebar is grouping tabs by tag.
    pub(crate) fn tab_run(&self, index: usize, cx: &gpui::App) -> Vec<usize> {
        if cx.global::<Config>().tab_bar_position != crate::core::config::TabBarPosition::Left {
            return (0..self.tabs.len()).collect();
        }
        let active_filter = self
            .active_tag_filter
            .filter(|id| self.sidebar_tags.contains(*id));
        sidebar_tag_sections(&self.tabs, &self.sidebar_tags, active_filter)
            .into_iter()
            .find(|s| s.tabs.contains(&index))
            .map(|s| s.tabs)
            .unwrap_or_else(|| (0..self.tabs.len()).collect())
    }

    pub(crate) fn activate_visual(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(&i) = self.visual_tab_order(cx).get(n) {
            self.activate(i, window, cx);
        }
    }
}

/// Right-click context menu for a tag pill in the sidebar filter bar.
fn tag_pill_menu(
    menu: PopupMenu,
    id: TagId,
    current_color: TagColor,
    app: &gpui::WeakEntity<AntermApp>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let rename_app = app.clone();
    let color_app = app.clone();
    let delete_app = app.clone();

    menu.min_w(px(180.))
        .item(
            PopupMenuItem::new(t(L10nKey::SidebarRenameTag)).on_click(move |_, window, cx| {
                let _ = rename_app.update(cx, |this, cx| this.start_tag_rename(id, window, cx));
            }),
        )
        .submenu(
            t(L10nKey::SidebarTagColor),
            window,
            cx,
            move |sub, _window, cx| {
                let mut sub = sub.check_side(Side::Right);
                let muted_fg = cx.theme().muted_foreground;
                for color in TagColor::ALL {
                    let app = color_app.clone();
                    let dot = tag_color_dot(color, cx);
                    let label = tag_color_label(color);
                    sub = sub.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            let swatch = match dot {
                                Some(c) => {
                                    div().flex_shrink_0().size(px(10.)).rounded_full().bg(c)
                                }
                                None => div()
                                    .flex_shrink_0()
                                    .size(px(10.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(muted_fg.opacity(0.6)),
                            };
                            h_flex().items_center().gap_2().child(swatch).child(label)
                        })
                        .checked(color == current_color)
                        .on_click(move |_, _window, cx| {
                            let _ = app.update(cx, |this, cx| this.set_tag_color(id, color, cx));
                        }),
                    );
                }
                sub
            },
        )
        .separator()
        .item(
            PopupMenuItem::new(t(L10nKey::SidebarDeleteTag)).on_click(move |_, _window, cx| {
                let _ = delete_app.update(cx, |this, cx| this.delete_tag(id, cx));
            }),
        )
}
