//! Rendering, character-level fuzzy highlighting, and keyboard routing for
//! [`SearchView`].

use gpui::{
    App, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent, Task, Window, div,
    prelude::*, px, rems,
};
use gpui_kit::component::{
    ActiveTheme as _, IndexPath, Selectable, h_flex,
    input::Input,
    list::{List, ListDelegate, ListEvent, ListState},
    v_flex,
};

use super::{
    EDIT_GESTURE, KEY_CONTEXT, PalettePage, PickerScope, SearchDelegate, SearchEvent, SearchView,
    TAGS_GESTURE,
};
use super::tags_page::TagPopoverRow;
use crate::core::actions::{SearchNextTab, SearchPrevTab};
use crate::ui::chrome::dialog::keycap;
use crate::ui::chrome::search::command::Item;
use crate::ui::chrome::search::score::fuzzy_match_ranges;
use crate::ui::chrome::search::{FileList, SearchTab};
use anterm_i18n::{L10nKey, t};

const ROW_H: f32 = 30.;
const HEADER_H: f32 = 22.;
const LIST_PAD: f32 = 6.;
const ROW_PAD: f32 = 8.;
const LABEL_INSET: f32 = LIST_PAD + ROW_PAD;

const PALETTE_RADIUS: f32 = 10.;
const ROW_RADIUS: f32 = 6.;
const CARD_MARGIN: f32 = 24.;
const CARD_MAX_W: f32 = 540.;

const SEARCH_H: f32 = 44.;
const SEARCH_TEXT: f32 = 14. / 16.;
const ROW_TEXT: f32 = 13. / 16.;
const ROW_META: f32 = 11.5 / 16.;
const SCOPE_TEXT: f32 = 11.5 / 16.;
const EDITOR_TABS_H: f32 = 30.;
const VISIBLE_ROWS: f32 = 12.;

fn is_edit_gesture(ks: &gpui::Keystroke) -> bool {
    if ks.key != "e" {
        return false;
    }
    let m = &ks.modifiers;
    let secondary = if cfg!(target_os = "macos") {
        m.platform
    } else {
        m.control
    };
    secondary && !m.shift && !m.alt
}

fn is_tags_gesture(ks: &gpui::Keystroke) -> bool {
    if ks.key != "l" {
        return false;
    }
    let m = &ks.modifiers;
    let secondary = if cfg!(target_os = "macos") {
        m.platform
    } else {
        m.control
    };
    secondary && !m.shift && !m.alt
}

impl SearchDelegate {
    fn render_row(&self, ix: IndexPath, item: &Item, cx: &App) -> gpui::AnyElement {
        let picked = Some(ix) == self.selected;
        let (fg, muted) = {
            let t = cx.theme();
            (t.foreground, t.muted_foreground)
        };
        let (_, query, _) = self.effective_scope_and_query();

        let tag_vis = item
            .tag_color
            .and_then(|c| crate::ui::workspace::tab_sidebar::tag_visual(c, cx));

        let title = gpui::SharedString::from(item.title.clone());
        let highlight_color = tag_vis.map_or(fg, |vis| vis.text);
        let ranges = fuzzy_match_ranges(query, &item.title);
        let title = if ranges.is_empty() {
            gpui::StyledText::new(title)
        } else {
            let style = gpui::HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                color: Some(highlight_color),
                ..Default::default()
            };
            gpui::StyledText::new(title).with_highlights(ranges.into_iter().map(|r| (r, style)))
        };

        let mut left = h_flex().flex_1().min_w_0().items_center().gap_2();
        if let Some(vis) = tag_vis {
            left = left.child(
                h_flex()
                    .flex_shrink(1.)
                    .min_w(px(40.))
                    .h(px(22.))
                    .px_2()
                    .gap_1p5()
                    .items_center()
                    .rounded(crate::ui::chrome::rounding::ROW_RADIUS)
                    .bg(if picked { vis.pill_bg_selected } else { vis.pill_bg })
                    .border_1()
                    .border_color(if picked {
                        vis.pill_border_selected
                    } else {
                        vis.pill_border
                    })
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
                            .text_color(if picked { vis.text_selected } else { vis.text })
                            .child(title),
                    ),
            );
        } else {
            left = left.child(
                div()
                    .flex_shrink(1.)
                    .min_w(px(40.))
                    .truncate()
                    .text_color(match query.is_empty() {
                        true => fg,
                        false => fg.opacity(0.78),
                    })
                    .child(title),
            );
        }
        if let Some(subtitle) = item.subtitle.clone() {
            left = left.child(
                div().flex_1().min_w(px(24.)).flex().justify_end().child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(rems(ROW_META))
                        .text_color(muted)
                        .child(subtitle),
                ),
            );
        }

        let mut right = h_flex().flex_shrink_0().items_center().gap_2();
        if let Some(note) = item.note.clone() {
            right = right.child(
                div()
                    .text_size(rems(ROW_META))
                    .text_color(muted)
                    .child(note),
            );
        }
        if picked && item.kind.edit_variant().is_some() {
            right = right.child(
                h_flex()
                    .items_center()
                    .gap(gpui::rems(5. / 16.))
                    .text_size(rems(ROW_META))
                    .text_color(muted)
                    .child(t(L10nKey::EditHint))
                    .child(crate::ui::chrome::dialog::chord(
                        crate::ui::chrome::keymap::key_tokens(EDIT_GESTURE),
                        cx,
                    )),
            );
        }
        if let Some(spec) = item.kind.key_spec(cx) {
            let tokens = crate::ui::chrome::keymap::key_tokens(&spec);
            right = right.child(
                h_flex()
                    .gap(gpui::rems(3. / 16.))
                    .children(tokens.into_iter().map(|t| keycap(t, cx))),
            );
        }

        h_flex()
            .w_full()
            .gap_3()
            .items_center()
            .justify_between()
            .child(left)
            .child(right)
            .into_any_element()
    }
}

impl ListDelegate for SearchDelegate {
    type Item = SearchRow;

    fn sections_count(&self, _cx: &App) -> usize {
        self.sections.len().max(1)
    }

    fn items_count(&self, section: usize, _cx: &App) -> usize {
        self.sections
            .get(section)
            .map(|s| s.rows.len())
            .unwrap_or(0)
    }

    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        if self.scope == PickerScope::Tab(SearchTab::Symbols)
            && let Some(live) = self.catalog.live_query.clone()
        {
            live(query, cx);
        }
        self.refresh(cx);
        self.selected = None;
        let first = self.first_row();
        self.set_selected_index(first, window, cx);
        Task::ready(())
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let title = self.sections.get(section)?.title.clone()?;
        Some(
            h_flex()
                .h(px(HEADER_H))
                .when(section > 0, |h| h.mt_1())
                .px(px(LABEL_INSET))
                .items_center()
                .text_size(rems(crate::ui::files::right_panel::HEADING))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        let (effective_scope, _, _) = self.effective_scope_and_query();
        let mut headline = t(L10nKey::SearchNoResults);
        let hint = match effective_scope {
            PickerScope::Tab(SearchTab::Files) => match &self.catalog.files {
                FileList::NoRoots => {
                    headline = t(L10nKey::SearchFilesNoRoots);
                    t(L10nKey::SearchFilesNoRootsHint)
                }
                FileList::Indexing => {
                    headline = t(L10nKey::SearchFilesIndexing);
                    ""
                }
                FileList::Failed => t(L10nKey::SearchFilesFailed),
                FileList::Ready(_) => t(L10nKey::PaletteTryDifferentSearch),
            },
            PickerScope::Tab(SearchTab::Tabs) if self.catalog.tabs.is_empty() => {
                headline = t(L10nKey::SearchTabsEmpty);
                ""
            }
            PickerScope::Tab(SearchTab::Tags) if self.catalog.tags.is_empty() => {
                headline = t(L10nKey::SearchTagsEmpty);
                t(L10nKey::SearchTagsEmptyHint)
            }
            PickerScope::Tab(SearchTab::Hosts) => t(L10nKey::ConnectSshHint),
            PickerScope::Tab(SearchTab::Locations) if self.catalog.locations.is_empty() => {
                t(L10nKey::SearchLocationsNone)
            }
            PickerScope::Tab(SearchTab::Symbols) if self.catalog.symbols.is_empty() => {
                headline = t(L10nKey::SearchSymbolsNone);
                t(L10nKey::SearchSymbolsNoneHint)
            }
            _ => t(L10nKey::PaletteTryDifferentSearch),
        };
        let theme = cx.theme();
        v_flex()
            .py_6()
            .px(px(LABEL_INSET))
            .gap_1()
            .items_start()
            .text_size(rems(ROW_TEXT))
            .text_color(theme.foreground)
            .child(headline)
            .when(!hint.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(rems(ROW_META))
                        .text_color(theme.muted_foreground)
                        .child(hint),
                )
            })
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let item = self.item_at(ix)?;
        let label: gpui::SharedString = match &item.subtitle {
            Some(sub) => format!("{}, {sub}", item.title).into(),
            None => item.title.clone().into(),
        };
        let content = self.render_row(ix, item, cx);
        Some(SearchRow {
            id: ("search-row", ix.section * 1000 + ix.row).into(),
            label,
            selected: Some(ix) == self.selected,
            child: content,
        })
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        if ix.is_none()
            && let Some(first) = self.first_row()
        {
            cx.defer_in(window, move |state, window, cx| {
                if state.selected_index().is_none() {
                    state.set_selected_index(Some(first), window, cx);
                }
            });
        }
        let moved = self.selected != ix;
        self.selected = ix;
        if moved && let Some(ix) = ix {
            cx.emit(ListEvent::Select(ix));
        }
        cx.notify();
    }
}

impl SearchView {
    /// Quiet inline scope badge shown on secondary pickers and prompts; the
    /// root `Cmd+P` Command Palette stays pure and badge-free like Zed.
    fn picker_badge(&self, cx: &App) -> Option<(gpui::SharedString, bool)> {
        let dismissible = self.can_pop();
        match self.top_page() {
            PalettePage::RenameTab(r) => {
                let heading = t(L10nKey::CmdRenameTab).trim_end_matches('…');
                let is_other_tab = self
                    .catalog
                    .active_tab_rename
                    .as_ref()
                    .is_some_and(|(active, _)| *active != r.tab);
                let label: gpui::SharedString = if is_other_tab && !r.prefill.is_empty() {
                    format!("{heading} · {}", r.prefill).into()
                } else {
                    heading.into()
                };
                Some((label, dismissible))
            }
            PalettePage::EditTags(p) => {
                let heading = t(L10nKey::SidebarTabTags);
                let is_other_tab = self
                    .catalog
                    .active_tab_rename
                    .as_ref()
                    .is_some_and(|(active, _)| *active != p.tab);
                let label: gpui::SharedString = if is_other_tab
                    && let Some(tab_name) = self.catalog.tab_labels.get(&p.tab)
                {
                    format!("{heading} · {tab_name}").into()
                } else {
                    heading.into()
                };
                Some((label, dismissible))
            }
            PalettePage::Picker(_) => {
                let (scope, prefix_routed) = self.effective_picker_scope(cx)?;
                match scope {
                    PickerScope::Tab(SearchTab::Actions) => None,
                    PickerScope::Themes => Some((
                        t(L10nKey::CmdChangeTheme).trim_end_matches('…').into(),
                        dismissible,
                    )),
                    PickerScope::Tab(_) if self.editor_row().is_some() => None,
                    PickerScope::Tab(tab) => {
                        let heading = match tab {
                            SearchTab::Locations => self.heading.unwrap_or_else(|| tab.title()),
                            _ => tab.title(),
                        };
                        let count = match tab {
                            SearchTab::Files
                            | SearchTab::Shells
                            | SearchTab::Hosts
                            | SearchTab::Tags => 0,
                            _ => self.list.read(cx).delegate().row_count(),
                        };
                        let label: gpui::SharedString = if count > 0 {
                            format!("{heading} · {count}").into()
                        } else {
                            heading.into()
                        };
                        Some((label, dismissible && !prefix_routed))
                    }
                }
            }
        }
    }

    /// Minimal Zed-style command input row: seamless borderless input with a
    /// clean hairline divider beneath it.
    fn render_input_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sf = cx.global::<crate::ui::chrome::presets::Surfaces>().popover;
        let theme = cx.theme();
        let (fg, muted, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let pill_bg = gpui::rgb(sf.selected);
        let hover_bg = gpui::rgb(sf.hover);
        let has_text = !self.input.read(cx).value().is_empty();
        let badge = self.picker_badge(cx);

        h_flex()
            .flex_none()
            .w_full()
            .h(px(SEARCH_H))
            .px(px(LABEL_INSET))
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(border.opacity(0.65))
            .when_some(badge, |bar, (label, dismissible)| {
                bar.child(
                    h_flex()
                        .id("search-scope-badge")
                        .flex_shrink_0()
                        .h(px(22.))
                        .px(gpui::rems(7. / 16.))
                        .gap_1()
                        .items_center()
                        .rounded_sm()
                        .bg(pill_bg)
                        .text_size(rems(SCOPE_TEXT))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .child(label)
                        .when(dismissible, |p| {
                            p.cursor_pointer()
                                .child(
                                    gpui::svg()
                                        .path("icons/settings/close.svg")
                                        .size(px(10.))
                                        .text_color(muted),
                                )
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.cancel_or_dismiss(window, cx);
                                }))
                        }),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(rems(SEARCH_TEXT))
                    .child(Input::new(&self.input).appearance(false).px_0().py_0()),
            )
            .when(has_text, |bar| {
                bar.child(
                    div()
                        .id("search-clear-btn")
                        .flex_shrink_0()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .cursor_pointer()
                        .text_color(muted)
                        .hover(move |d| d.bg(hover_bg).text_color(fg))
                        .child(
                            gpui::svg()
                                .path("icons/settings/close.svg")
                                .size(px(11.)),
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.input
                                .update(cx, |state, cx| state.set_value("", window, cx));
                            if this.active_picker_scope().is_some() {
                                this.apply_query_to_list(String::new(), window, cx);
                            } else {
                                if let Some(p) = this.edit_tags_page_mut() {
                                    p.query.clear();
                                    p.selected = 0;
                                    p.editing_color = None;
                                    p.renaming = None;
                                }
                                cx.notify();
                            }
                            this.focus(window, cx);
                        })),
                )
            })
            .when(!has_text, |bar| bar.child(keycap("esc", cx)))
    }

    fn render_rename_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let value = self.input.read(cx).value();
        let is_reset = value.trim().is_empty();
        let hint = if is_reset {
            t(L10nKey::SearchRenameTabResetHint)
        } else {
            t(L10nKey::SearchRenameTabHint)
        };
        let muted = cx.theme().muted_foreground;

        h_flex()
            .flex_none()
            .w_full()
            .h(px(34.))
            .px(px(LABEL_INSET))
            .gap_3()
            .items_center()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(ROW_META))
                    .text_color(muted)
                    .child(hint),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .child(keycap("↵", cx))
                    .when(!value.is_empty(), |h| h.child(keycap("esc", cx))),
            )
    }

    fn render_picker_footer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (scope, _) = self.effective_picker_scope(cx)?;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let hint_chip = |chord_spec: &str, label: &'static str, cx: &App| {
            h_flex()
                .items_center()
                .gap(gpui::rems(5. / 16.))
                .text_size(rems(ROW_META))
                .text_color(muted)
                .child(crate::ui::chrome::dialog::chord(
                    crate::ui::chrome::keymap::key_tokens(chord_spec),
                    cx,
                ))
                .child(label)
        };
        let prefix_chip = |prefix: &'static str, label: &'static str, cx: &App| {
            h_flex()
                .items_center()
                .gap(gpui::rems(5. / 16.))
                .text_size(rems(ROW_META))
                .text_color(muted)
                .child(keycap(prefix, cx))
                .child(label)
        };

        let items = match scope {
            PickerScope::Tab(SearchTab::Actions)
                if !self.can_pop() && self.input.read(cx).value().is_empty() =>
            {
                h_flex()
                    .items_center()
                    .gap(gpui::rems(14. / 16.))
                    .child(prefix_chip("@", SearchTab::Tabs.title(), cx))
                    .child(prefix_chip("#", SearchTab::Tags.title(), cx))
                    .child(prefix_chip("/", SearchTab::Files.title(), cx))
            }
            PickerScope::Tab(SearchTab::Tabs) => {
                if self.catalog.tabs.is_empty() {
                    return None;
                }
                h_flex()
                    .items_center()
                    .gap(gpui::rems(14. / 16.))
                    .child(prefix_chip("↵", t(L10nKey::SearchHintSwitch), cx))
                    .child(hint_chip(EDIT_GESTURE, t(L10nKey::SearchHintRename), cx))
                    .child(hint_chip(TAGS_GESTURE, t(L10nKey::SearchHintTags), cx))
            }
            PickerScope::Tab(SearchTab::Tags) => {
                if self.catalog.tags.is_empty() {
                    return None;
                }
                h_flex()
                    .items_center()
                    .gap(gpui::rems(14. / 16.))
                    .child(prefix_chip("↵", t(L10nKey::SearchHintSwitch), cx))
                    .child(hint_chip(EDIT_GESTURE, t(L10nKey::SearchHintEditTag), cx))
            }
            _ => return None,
        };

        Some(
            h_flex()
                .flex_none()
                .w_full()
                .h(px(30.))
                .px(px(LABEL_INSET))
                .items_center()
                .border_t_1()
                .border_color(border.opacity(0.55))
                .child(items)
                .into_any_element(),
        )
    }

    fn render_editor_tabs(&self, tabs: Vec<SearchTab>, cx: &mut Context<Self>) -> impl IntoElement {
        let sf = cx.global::<crate::ui::chrome::presets::Surfaces>().popover;
        let (active_bg, hover_bg) = (gpui::rgb(sf.selected), gpui::rgb(sf.hover));
        let theme = cx.theme();
        let (fg, muted) = (theme.foreground, theme.muted_foreground);
        let current = self.tab();
        h_flex()
            .flex_none()
            .h(px(EDITOR_TABS_H))
            .px(px(LABEL_INSET))
            .gap_1()
            .items_center()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .children(tabs.into_iter().enumerate().map(|(i, tab)| {
                let active = tab == current;
                div()
                    .id(("search-tab", i))
                    .h(px(22.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .text_size(rems(SCOPE_TEXT))
                    .cursor_pointer()
                    .map(|d| match active {
                        true => d
                            .bg(active_bg)
                            .text_color(fg)
                            .font_weight(FontWeight::MEDIUM),
                        false => d
                            .text_color(muted)
                            .hover(move |d| d.bg(hover_bg).text_color(fg)),
                    })
                    .child(tab.title())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.go_to_tab(tab, window, cx);
                        this.focus(window, cx);
                    }))
            }))
    }
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scrim = crate::ui::chrome::presets::scrim_fill(cx);
        let renaming = self.is_renaming();
        let editing_tags = self.is_editing_tags();
        let editor_tabs = (!renaming && !editing_tags)
            .then(|| self.editor_row())
            .flatten();
        let picker_footer = self.render_picker_footer(cx);

        let viewport = window.viewport_size();
        let top = (viewport.height.as_f32() * 0.14).clamp(16., 96.);
        let editor_tabs_h = if editor_tabs.is_some() {
            EDITOR_TABS_H
        } else {
            0.
        };
        let footer_h = if picker_footer.is_some() { 30. } else { 0. };
        let chrome = SEARCH_H + editor_tabs_h + footer_h + CARD_MARGIN;
        let list_max_h = px((viewport.height.as_f32() - top - chrome)
            .max(ROW_H)
            .min(ROW_H * VISIBLE_ROWS + LIST_PAD * 2.));

        let card = v_flex()
            .relative()
            .w(px((viewport.width.as_f32() - 32.).clamp(0., CARD_MAX_W)))
            .map(|panel| crate::ui::chrome::theme::floating_surface(panel, cx))
            .rounded(px(PALETTE_RADIUS))
            .overflow_hidden()
            .child(self.render_input_bar(cx))
            .when_some(editor_tabs, |card, tabs| {
                card.child(self.render_editor_tabs(tabs, cx))
            })
            .when(renaming, |card| card.child(self.render_rename_footer(cx)))
            .when(editing_tags, |card| {
                card.child(self.render_edit_tags_body(list_max_h, cx))
            })
            .when(!renaming && !editing_tags, |card| {
                card.child(
                    List::new(&self.list)
                        .pt(px(LIST_PAD))
                        .pb(px(LIST_PAD))
                        .max_h(list_max_h),
                )
            })
            .when_some(picker_footer, |card, footer| card.child(footer));

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(top))
            .bg(scrim)
            .key_context(KEY_CONTEXT)
            .on_action(
                cx.listener(|this, _: &SearchNextTab, window, cx| this.step_tab(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SearchPrevTab, window, cx| this.step_tab(false, window, cx)),
            )
            .on_action(cx.listener(
                |this, _: &crate::core::actions::EditorGoToSymbol, _window, cx| {
                    if this.active_picker_scope() == Some(PickerScope::Tab(SearchTab::Symbols))
                        && !this.can_pop()
                    {
                        cx.emit(SearchEvent::Dismiss);
                    } else {
                        cx.propagate();
                    }
                },
            ))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                let ks = &ev.keystroke;
                let no_mods = !ks.modifiers.modified();
                let on_picker = this.active_picker_scope().is_some();
                let on_tags = this.is_editing_tags();
                let tag_renaming = this.is_tag_inline_renaming();
                match ks.key.as_str() {
                    "escape" => {
                        cx.stop_propagation();
                        this.cancel_or_dismiss(window, cx);
                    }
                    "up" if no_mods && on_picker => {
                        cx.stop_propagation();
                        this.move_selection(-1, window, cx);
                    }
                    "down" if no_mods && on_picker => {
                        cx.stop_propagation();
                        this.move_selection(1, window, cx);
                    }
                    "up" if no_mods && on_tags && !tag_renaming => {
                        cx.stop_propagation();
                        this.move_edit_tags_selection(-1, cx);
                    }
                    "down" if no_mods && on_tags && !tag_renaming => {
                        cx.stop_propagation();
                        this.move_edit_tags_selection(1, cx);
                    }
                    "tab" if no_mods && on_tags && !tag_renaming => {
                        if matches!(
                            this.selected_edit_tags_row(cx),
                            Some(TagPopoverRow::Create(_))
                        ) {
                            cx.stop_propagation();
                            this.cycle_edit_tags_create_color(cx);
                        }
                    }
                    "f2" if no_mods && on_tags && !tag_renaming => {
                        if this.on_edit_gesture(window, cx) {
                            cx.stop_propagation();
                        }
                    }
                    "backspace"
                        if no_mods
                            && on_picker
                            && this.can_pop()
                            && this.input.read(cx).value().is_empty() =>
                    {
                        cx.stop_propagation();
                        this.pop_page(window, cx);
                    }
                    _ if (on_picker || (on_tags && !tag_renaming))
                        && is_edit_gesture(ks)
                        && this.on_edit_gesture(window, cx) =>
                    {
                        cx.stop_propagation();
                    }
                    _ if on_picker && is_tags_gesture(ks) && this.on_tags_gesture(window, cx) => {
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    cx.emit(SearchEvent::Dismiss);
                }),
            )
            .child(div().occlude().child(card))
    }
}

#[derive(IntoElement)]
pub struct SearchRow {
    id: gpui::ElementId,
    label: gpui::SharedString,
    selected: bool,
    child: gpui::AnyElement,
}

impl Selectable for SearchRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for SearchRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let sf = cx.global::<crate::ui::chrome::presets::Surfaces>().popover;
        let (hover, picked) = (gpui::rgb(sf.hover), gpui::rgb(sf.selected));
        h_flex()
            .id(self.id)
            .role(gpui::Role::ListBoxOption)
            .aria_label(self.label)
            .aria_selected(self.selected)
            .items_center()
            .flex_none()
            .h(px(ROW_H))
            .mx(px(LIST_PAD))
            .px(px(ROW_PAD))
            .rounded(px(ROW_RADIUS))
            .overflow_hidden()
            .cursor_pointer()
            .text_size(rems(ROW_TEXT))
            .text_color(cx.theme().foreground)
            .when(self.selected, |r| r.bg(picked))
            .when(!self.selected, |r| r.hover(move |s| s.bg(hover)))
            .child(self.child)
    }
}
