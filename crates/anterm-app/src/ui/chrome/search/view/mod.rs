//! Pure Zed-style Command Palette modal (`Cmd+P`) built on a unified
//! navigation stack (`Vec<PalettePage>`) covering pickers (`Actions`, `Tabs`,
//! `Tags`, `Themes`, `Hosts`, `Shells`, `Files`, `Symbols`, `Locations`),
//! in-place prompts (`Rename Tab`), and tag management (`Edit Tab Tags`).

mod render;
mod tags_page;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tags_page::{TagPopoverRow, next_tag_color, popover_rows};
use tags_page::EditTagsPage;

use std::rc::Rc;

use gpui::{App, Context, Entity, EventEmitter, ScrollStrategy, Subscription, Window, prelude::*};
use gpui_kit::component::{
    IndexPath,
    input::{InputEvent, InputState},
    list::{ListEvent, ListState},
};

use super::command::{CommandKind, Item, PaletteAction, PickerTarget};
use super::sources::{Catalog, Section, plain, route_actions_query};
use super::{FileList, SearchTab};
use crate::core::tag::{SidebarTags, TagColor, TagId};
use anterm_i18n::{L10nKey, t};

pub(crate) use anterm_ui_kit::actions::SEARCH_KEY_CONTEXT as KEY_CONTEXT;
pub(super) const EDIT_GESTURE: &str = "secondary-e";
pub(super) const TAGS_GESTURE: &str = "secondary-l";

/// Active list source inside a [`PickerPage`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickerScope {
    Tab(SearchTab),
    Themes,
}

/// Filterable list page on the palette stack.
struct PickerPage {
    scope: PickerScope,
    query: String,
    selected: Option<IndexPath>,
}

/// Focused text-input page for renaming a tab in place.
struct RenameTabPage {
    tab: anterm_core::machine::TabId,
    prefill: String,
}

/// One page on the command palette's navigation stack.
///
/// The bottom entry is the root mode; pushing any sub-page parks the parent's
/// query and selection so `Escape` pops cleanly back to it, and dismisses the
/// overlay once the stack reaches depth 1.
enum PalettePage {
    Picker(PickerPage),
    RenameTab(RenameTabPage),
    EditTags(EditTagsPage),
}

pub(crate) struct SearchDelegate {
    catalog: Rc<Catalog>,
    scope: PickerScope,
    themes: Vec<Item>,
    query: String,
    sections: Vec<Section>,
    selected: Option<IndexPath>,
}

impl SearchDelegate {
    fn new(
        catalog: Rc<Catalog>,
        scope: PickerScope,
        themes: Vec<Item>,
        query: String,
        cx: &App,
    ) -> Self {
        let mut this = Self {
            catalog,
            scope,
            themes,
            query,
            sections: Vec::new(),
            selected: None,
        };
        this.refresh(cx);
        this
    }

    fn effective_scope_and_query(&self) -> (PickerScope, &str, bool) {
        match self.scope {
            PickerScope::Tab(SearchTab::Actions) => {
                let (tab, query, routed) = route_actions_query(&self.query);
                (PickerScope::Tab(tab), query, routed)
            }
            other => (other, self.query.trim(), false),
        }
    }

    fn refresh(&mut self, cx: &App) {
        self.sections = match self.scope {
            PickerScope::Tab(SearchTab::Actions) => {
                let (tab, query, _) = route_actions_query(&self.query);
                self.catalog.sections(tab, query, cx)
            }
            PickerScope::Tab(tab) => self.catalog.sections(tab, &self.query, cx),
            PickerScope::Themes => plain(&self.themes, &self.query),
        };
    }

    fn item_at(&self, ix: IndexPath) -> Option<&Item> {
        self.sections.get(ix.section)?.rows.get(ix.row)
    }

    fn selected_item(&self) -> Option<&Item> {
        self.selected.and_then(|ix| self.item_at(ix))
    }

    fn position_of(&self, kind: &CommandKind) -> Option<IndexPath> {
        self.sections.iter().enumerate().find_map(|(s, section)| {
            let row = section.rows.iter().position(|i| &i.kind == kind)?;
            Some(IndexPath::new(row).section(s))
        })
    }

    fn row_count(&self) -> usize {
        self.sections.iter().map(|s| s.rows.len()).sum()
    }

    fn first_row(&self) -> Option<IndexPath> {
        let section = self.sections.iter().position(|s| !s.rows.is_empty())?;
        Some(IndexPath::new(0).section(section))
    }

    fn step_index(&self, current: Option<IndexPath>, delta: isize) -> Option<IndexPath> {
        let flat: Vec<IndexPath> = self
            .sections
            .iter()
            .enumerate()
            .flat_map(|(s, sec)| {
                (0..sec.rows.len()).map(move |r| IndexPath::new(r).section(s))
            })
            .collect();
        if flat.is_empty() {
            return None;
        }
        let pos = current
            .and_then(|ix| {
                flat.iter()
                    .position(|p| p.section == ix.section && p.row == ix.row)
            })
            .unwrap_or(0) as isize;
        let next = (pos + delta).rem_euclid(flat.len() as isize) as usize;
        Some(flat[next])
    }
}

pub enum SearchEvent {
    Confirm(CommandKind),
    /// Record usage of a palette command that transitioned to an in-modal page
    /// without closing the search overlay.
    RecordUsage(&'static str),
    /// Lazily warm the project file index when entering Files via command or `/` prefix.
    EnsureFileIndex,
    /// Commit a tab rename entered directly inside the command palette.
    RenameTab {
        tab: anterm_core::machine::TabId,
        value: String,
        prefill: String,
    },
    /// Toggle tag attachment on a tab from the in-palette `Edit Tab Tags` page.
    ToggleTabTag {
        tab: anterm_core::machine::TabId,
        tag: TagId,
    },
    /// Create a new tag and attach it to a tab from `Edit Tab Tags`.
    CreateAndAttachTag {
        tab: anterm_core::machine::TabId,
        name: String,
        color: TagColor,
    },
    /// Update an existing tag's semantic color from `Edit Tab Tags`.
    SetTagColor {
        tag: TagId,
        color: TagColor,
    },
    /// Rename an existing tag from `Edit Tab Tags`.
    RenameSidebarTag {
        tag: TagId,
        name: String,
    },
    /// Delete a tag from `Edit Tab Tags`.
    DeleteSidebarTag {
        tag: TagId,
    },
    /// Open a shell or saved host in a split pane alongside the focused pane.
    ConfirmSplit(CommandKind),
    Dismiss,
    PreviewTheme(usize),
    CancelThemePreview,
    PreviewSymbol {
        line: u32,
        column: u32,
    },
    SwitchEditorTab(SearchTab),
    PreviewLocation {
        path: std::path::PathBuf,
        line: u32,
        column: u32,
    },
}

pub struct SearchView {
    input: Entity<InputState>,
    list: Entity<ListState<SearchDelegate>>,
    catalog: Rc<Catalog>,
    /// Non-empty navigation stack; the last element is the active page.
    pages: Vec<PalettePage>,
    previewing: Option<usize>,
    symbol_initial: Option<CommandKind>,
    symbol_moved: bool,
    heading: Option<&'static str>,
    editor_tabs: Vec<SearchTab>,
    /// Guards programmatic `input.set_value` calls against firing redundant
    /// `InputEvent::Change` list refreshes.
    syncing_input: bool,
    _input_sub: Subscription,
    _sub: Subscription,
}

impl SearchView {
    pub fn new(
        catalog: Catalog,
        tab: SearchTab,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tab.placeholder()));
        if !query.is_empty() {
            input.update(cx, |state, cx| state.set_value(query, window, cx));
        }
        input.update(cx, |state, cx| state.focus(window, cx));
        let _input_sub = cx.subscribe_in(&input, window, Self::on_input_event);

        let catalog = Rc::new(catalog);
        let delegate = SearchDelegate::new(
            catalog.clone(),
            PickerScope::Tab(tab),
            Vec::new(),
            query.to_string(),
            cx,
        );
        let list = Self::build_list(delegate, None, window, cx);
        let _sub = cx.subscribe_in(&list, window, Self::on_list_event);
        Self {
            input,
            list,
            catalog,
            pages: vec![PalettePage::Picker(PickerPage {
                scope: PickerScope::Tab(tab),
                query: query.to_string(),
                selected: None,
            })],
            previewing: None,
            symbol_initial: None,
            symbol_moved: false,
            heading: None,
            editor_tabs: Vec::new(),
            syncing_input: false,
            _input_sub,
            _sub,
        }
    }

    /// Puts the keyboard back in the search field (`palette`).
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |state, cx| state.focus(window, cx));
    }

    fn top_page(&self) -> &PalettePage {
        self.pages
            .last()
            .expect("palette stack is always non-empty")
    }

    fn can_pop(&self) -> bool {
        self.pages.len() > 1
    }

    fn active_picker_scope(&self) -> Option<PickerScope> {
        match self.top_page() {
            PalettePage::Picker(p) => Some(p.scope),
            PalettePage::RenameTab(_) | PalettePage::EditTags(_) => None,
        }
    }

    /// Effective picker scope after applying live prefix routing (`@`, `#`, `/`)
    /// on the root `Actions` page.
    fn effective_picker_scope(&self, cx: &App) -> Option<(PickerScope, bool)> {
        let scope = self.active_picker_scope()?;
        if scope == PickerScope::Tab(SearchTab::Actions) {
            let raw = self.input.read(cx).value();
            let (tab, _, routed) = route_actions_query(raw.as_ref());
            Some((PickerScope::Tab(tab), routed))
        } else {
            Some((scope, false))
        }
    }

    pub(super) fn is_renaming(&self) -> bool {
        matches!(self.top_page(), PalettePage::RenameTab(_))
    }

    pub(crate) fn tab(&self) -> SearchTab {
        self.pages
            .iter()
            .rev()
            .find_map(|page| match page {
                PalettePage::Picker(PickerPage {
                    scope: PickerScope::Tab(tab),
                    ..
                }) => Some(*tab),
                _ => None,
            })
            .unwrap_or(SearchTab::Actions)
    }

    pub(super) fn set_input_silently(
        &mut self,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.syncing_input = true;
        self.input
            .update(cx, |state, cx| state.set_value(value, window, cx));
        self.syncing_input = false;
    }

    pub(super) fn snapshot_top_page(&mut self, cx: &App) {
        let query = self.input.read(cx).value().to_string();
        let selected = self.list.read(cx).selected_index();
        match self.pages.last_mut() {
            Some(PalettePage::Picker(page)) => {
                page.query = query;
                page.selected = selected;
            }
            Some(PalettePage::EditTags(page)) => {
                page.query = query;
            }
            _ => {}
        }
    }

    fn placeholder_for(&self) -> &'static str {
        match self.top_page() {
            PalettePage::Picker(p) => match p.scope {
                PickerScope::Tab(tab) => tab.placeholder(),
                PickerScope::Themes => t(L10nKey::SearchTheme),
            },
            PalettePage::RenameTab(_) => t(L10nKey::SearchPlaceholderRenameTab),
            PalettePage::EditTags(_) => t(L10nKey::SidebarTagSearchPlaceholder),
        }
    }

    pub(super) fn sync_placeholder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.placeholder_for();
        self.input.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
        });
    }

    fn build_list(
        delegate: SearchDelegate,
        selected: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ListState<SearchDelegate>> {
        let first = selected.or_else(|| delegate.first_row());
        let list = cx.new(|cx| ListState::new(delegate, window, cx).searchable(false));
        list.update(cx, |state, cx| {
            state.set_selected_index(first, window, cx);
            if selected.is_some() {
                state.scroll_to_selected_item(window, cx);
            }
        });
        list
    }

    fn mount_picker_list(
        &mut self,
        scope: PickerScope,
        query: String,
        selected: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let themes = match scope {
            PickerScope::Themes => Item::themes(cx),
            PickerScope::Tab(_) => Vec::new(),
        };
        let delegate = SearchDelegate::new(self.catalog.clone(), scope, themes, query, cx);
        let list = Self::build_list(delegate, selected, window, cx);
        self._sub = cx.subscribe_in(&list, window, Self::on_list_event);
        self.list = list;
        self.sync_placeholder(window, cx);
        self.focus(window, cx);
        cx.notify();
    }

    /// Pushes a new picker page onto the navigation stack, parking the parent's
    /// query and selection so `Escape` pops cleanly back to it.
    fn push_picker(
        &mut self,
        scope: PickerScope,
        query: String,
        selected: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snapshot_top_page(cx);
        self.pages.push(PalettePage::Picker(PickerPage {
            scope,
            query: query.clone(),
            selected,
        }));
        self.set_input_silently(query.clone(), window, cx);
        self.mount_picker_list(scope, query, selected, window, cx);
    }

    /// Transitions into the in-place `Rename Tab` prompt targeting `tab` with
    /// initial label `current`. When `push` is true, pushes onto the stack so
    /// `Escape` returns to the parent page; otherwise replaces the stack as a
    /// standalone root prompt.
    pub(crate) fn open_rename_tab_for(
        &mut self,
        tab: anterm_core::machine::TabId,
        current: String,
        push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = PalettePage::RenameTab(RenameTabPage {
            tab,
            prefill: current.clone(),
        });
        if push {
            self.snapshot_top_page(cx);
            self.pages.push(page);
        } else {
            self.pages = vec![page];
        }
        self.sync_placeholder(window, cx);
        self.set_input_silently(current, window, cx);
        self.focus(window, cx);
        crate::ui::chrome::prefill::select_all_when_drawn(&self.input, window, cx);
        cx.notify();
    }

    /// Transitions into `Rename Tab` targeting the active tab from `catalog`.
    pub(crate) fn open_rename_tab(
        &mut self,
        from_palette: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((tab, current)) = self.catalog.active_tab_rename.clone() else {
            return false;
        };
        self.open_rename_tab_for(tab, current, from_palette, window, cx);
        true
    }

    /// Pops the top page off the navigation stack and restores the parent page.
    /// Returns `false` when already at the root page.
    pub(super) fn pop_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.pages.len() <= 1 {
            return false;
        }
        let popped = self.pages.pop().expect("checked len > 1");
        if matches!(
            popped,
            PalettePage::Picker(PickerPage {
                scope: PickerScope::Themes,
                ..
            })
        ) {
            self.previewing = None;
            cx.emit(SearchEvent::CancelThemePreview);
        }
        match self.top_page() {
            PalettePage::Picker(p) => {
                let (scope, query, selected) = (p.scope, p.query.clone(), p.selected);
                self.set_input_silently(query.clone(), window, cx);
                self.mount_picker_list(scope, query, selected, window, cx);
            }
            PalettePage::RenameTab(r) => {
                let prefill = r.prefill.clone();
                self.sync_placeholder(window, cx);
                self.set_input_silently(prefill, window, cx);
                self.focus(window, cx);
                cx.notify();
            }
            PalettePage::EditTags(p) => {
                let query = p.query.clone();
                self.sync_placeholder(window, cx);
                self.set_input_silently(query, window, cx);
                self.focus(window, cx);
                cx.notify();
            }
        }
        true
    }

    /// Updates the palette's cached tag & tab snapshots after a live tag
    /// mutation inside `Edit Tab Tags`, keeping both the active page and any
    /// parked `Tabs`/`Tags` parent lists synchronized.
    pub(crate) fn refresh_tag_state(
        &mut self,
        sidebar_tags: SidebarTags,
        tab_tags: std::collections::HashMap<anterm_core::machine::TabId, Vec<TagId>>,
        tabs_items: Vec<Item>,
        tags_items: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(
            |catalog| {
                catalog.sidebar_tags = sidebar_tags;
                catalog.tab_tags = tab_tags;
                catalog.tabs = tabs_items;
                catalog.tags = tags_items;
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn apply_query_to_list(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_picker_scope() == Some(PickerScope::Tab(SearchTab::Actions)) {
            let (tab, _, routed) = route_actions_query(&query);
            if routed
                && tab == SearchTab::Files
                && !matches!(self.catalog.files, FileList::Ready(_) | FileList::Indexing)
            {
                cx.emit(SearchEvent::EnsureFileIndex);
            }
        }
        if self.active_picker_scope() == Some(PickerScope::Tab(SearchTab::Symbols))
            && let Some(live) = self.catalog.live_query.clone()
        {
            live(&query, cx);
        }
        if let Some(PalettePage::Picker(p)) = self.pages.last_mut() {
            p.query = query.clone();
        }
        self.list.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.query = query;
            delegate.refresh(cx);
            delegate.selected = None;
            let first = delegate.first_row();
            state.set_selected_index(first, window, cx);
            state.scroll_to_item(IndexPath::default(), ScrollStrategy::Top, window, cx);
        });
        cx.notify();
    }

    fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::Change => {
                if self.syncing_input {
                    return;
                }
                match self.top_page() {
                    PalettePage::Picker(_) => {
                        let raw = input.read(cx).value().to_string();
                        self.apply_query_to_list(raw, window, cx);
                    }
                    PalettePage::RenameTab(_) => cx.notify(),
                    PalettePage::EditTags(_) => {
                        let raw = input.read(cx).value().to_string();
                        if let Some(p) = self.edit_tags_page_mut() {
                            p.query = raw;
                            p.selected = 0;
                            p.editing_color = None;
                            p.renaming = None;
                        }
                        cx.notify();
                    }
                }
            }
            InputEvent::PressEnter { .. } => match self.top_page() {
                PalettePage::Picker(_) => self.confirm_selected(window, cx),
                PalettePage::RenameTab(prompt) => {
                    let value = input.read(cx).value().to_string();
                    cx.emit(SearchEvent::RenameTab {
                        tab: prompt.tab,
                        value,
                        prefill: prompt.prefill.clone(),
                    });
                }
                PalettePage::EditTags(_) => self.confirm_edit_tags_selection(window, cx),
            },
            _ => {}
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |state, cx| {
            let next = state.delegate().step_index(state.selected_index(), delta);
            if let Some(ix) = next {
                state.set_selected_index(Some(ix), window, cx);
                state.scroll_to_item(ix, ScrollStrategy::Top, window, cx);
            }
        });
        cx.notify();
    }

    fn confirm_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .list
            .read(cx)
            .selected_index()
            .or_else(|| self.list.read(cx).delegate().first_row());
        let Some(ix) = ix else { return };
        self.confirm_row(ix, window, cx);
    }

    pub(super) fn cancel_or_dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_tag_inline_renaming() {
            self.cancel_edit_tags_rename(window, cx);
            return;
        }
        if !self.pop_page(window, cx) {
            cx.emit(SearchEvent::Dismiss);
        }
    }

    /// Resets the palette stack to a single standalone picker on `tab`.
    pub(crate) fn set_tab(
        &mut self,
        tab: SearchTab,
        query: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_picker_scope() == Some(PickerScope::Themes) {
            self.previewing = None;
            cx.emit(SearchEvent::CancelThemePreview);
        }
        let next_query = match query {
            Some(q) => {
                self.set_input_silently(q.to_string(), window, cx);
                q.to_string()
            }
            None => self.input.read(cx).value().to_string(),
        };
        self.pages = vec![PalettePage::Picker(PickerPage {
            scope: PickerScope::Tab(tab),
            query: next_query.clone(),
            selected: None,
        })];
        self.mount_picker_list(PickerScope::Tab(tab), next_query, None, window, cx);
    }

    pub(crate) fn set_files(
        &mut self,
        files: FileList,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(|catalog| catalog.files = files, window, cx);
    }

    pub(crate) fn set_symbols(
        &mut self,
        symbols: Vec<(usize, Item)>,
        selected: Option<CommandKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.symbol_moved = false;
        self.symbol_initial = selected.clone();
        self.update_catalog(|catalog| catalog.symbols = symbols, window, cx);
        if let Some(kind) = selected {
            self.list.update(cx, |state, cx| {
                if let Some(ix) = state.delegate().position_of(&kind) {
                    state.set_selected_index(Some(ix), window, cx);
                    state.scroll_to_selected_item(window, cx);
                }
            });
        }
    }

    pub(crate) fn set_locations(
        &mut self,
        heading: Option<&'static str>,
        locations: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.heading = heading;
        self.symbol_moved = false;
        self.symbol_initial = locations.first().map(|item| item.kind.clone());
        self.update_catalog(|catalog| catalog.locations = locations, window, cx);
    }

    pub(crate) fn set_editor_tabs(&mut self, tabs: Vec<SearchTab>, cx: &mut Context<Self>) {
        self.editor_tabs = tabs;
        cx.notify();
    }

    pub(super) fn editor_row(&self) -> Option<Vec<SearchTab>> {
        if self.can_pop() {
            return None;
        }
        (self.tab().in_editor_row() && self.editor_tabs.len() > 1)
            .then(|| self.editor_tabs.clone())
    }

    pub(super) fn go_to_tab(&mut self, tab: SearchTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab() {
            return;
        }
        if tab.in_editor_row() {
            cx.emit(SearchEvent::SwitchEditorTab(tab));
        } else {
            self.set_tab(tab, None, window, cx);
        }
    }

    pub(crate) fn set_project_symbols(
        &mut self,
        symbols: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(|catalog| catalog.project_symbols = symbols, window, cx);
    }

    pub(crate) fn set_live_query(
        &mut self,
        live: crate::ui::chrome::search::LiveQuery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_catalog(|catalog| catalog.live_query = Some(live), window, cx);
    }

    fn update_catalog(
        &mut self,
        change: impl FnOnce(&mut Catalog),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut catalog = (*self.catalog).clone();
        change(&mut catalog);
        self.catalog = Rc::new(catalog);
        if matches!(
            self.active_picker_scope(),
            None | Some(PickerScope::Themes)
        ) {
            return;
        }
        let catalog = self.catalog.clone();
        self.list.update(cx, |state, cx| {
            let before = state.delegate().selected_item().map(|i| i.kind.clone());
            let delegate = state.delegate_mut();
            delegate.catalog = catalog;
            delegate.refresh(cx);
            let keep = before.and_then(|kind| delegate.position_of(&kind));
            let target = keep.or_else(|| state.delegate().first_row());
            state.set_selected_index(target, window, cx);
        });
        cx.notify();
    }

    pub(super) fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.editor_row() else { return };
        let n = row.len();
        let cur = self.tab();
        let i = row.iter().position(|t| *t == cur).unwrap_or(0);
        let next = row[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
        self.go_to_tab(next, window, cx);
    }

    pub(crate) fn on_split_open(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.active_picker_scope().is_none() {
            return;
        }
        let kind = self
            .list
            .read(cx)
            .delegate()
            .selected_item()
            .filter(|item| item.kind.can_split())
            .map(|item| item.kind.clone());
        if let Some(kind) = kind {
            cx.emit(SearchEvent::ConfirmSplit(kind));
        }
    }

    fn open_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.previewing = Item::active_theme_index(cx);
        let selected = self.previewing.map(IndexPath::new);
        self.push_picker(PickerScope::Themes, String::new(), selected, window, cx);
    }

    pub(super) fn on_edit_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_editing_tags() {
            if let Some(tags_page::TagPopoverRow::Existing(tag_id)) =
                self.selected_edit_tags_row(cx)
            {
                self.start_edit_tags_rename(tag_id, window, cx);
                return true;
            }
            return false;
        }
        let Some((scope, _)) = self.effective_picker_scope(cx) else {
            return false;
        };
        let selected_kind = self
            .list
            .read(cx)
            .delegate()
            .selected_item()
            .map(|item| item.kind.clone());
        match (scope, selected_kind) {
            (
                PickerScope::Tab(SearchTab::Tabs),
                Some(CommandKind::Pick(PickerTarget::GoToTab(tab_id))),
            ) => {
                let current = self
                    .catalog
                    .tab_labels
                    .get(&tab_id)
                    .cloned()
                    .unwrap_or_default();
                self.open_rename_tab_for(tab_id, current, true, window, cx);
                true
            }
            (
                PickerScope::Tab(SearchTab::Tags),
                Some(CommandKind::Pick(PickerTarget::SelectTag(Some(tag_id)))),
            ) => {
                if let Some((active_tab, _)) = self.catalog.active_tab_rename {
                    self.open_edit_tags_for(active_tab, Some(tag_id), true, window, cx);
                    return true;
                }
                false
            }
            (_, Some(kind)) => {
                if let Some(edit) = kind.edit_variant() {
                    cx.emit(SearchEvent::Confirm(edit));
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    pub(super) fn on_tags_gesture(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.effective_picker_scope(cx).map(|(s, _)| s)
            != Some(PickerScope::Tab(SearchTab::Tabs))
        {
            return false;
        }
        let selected_tab = self
            .list
            .read(cx)
            .delegate()
            .selected_item()
            .and_then(|item| match item.kind {
                CommandKind::Pick(PickerTarget::GoToTab(id)) => Some(id),
                _ => None,
            });
        if let Some(tab_id) = selected_tab {
            self.open_edit_tags_for(tab_id, None, true, window, cx);
            return true;
        }
        false
    }

    fn confirm_row(&mut self, ix: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let item = self.list.read(cx).delegate().item_at(ix).cloned();
        match item {
            Some(item) => {
                let record = |action: PaletteAction, cx: &mut Context<Self>| {
                    cx.emit(SearchEvent::RecordUsage(action.id()));
                };
                match item.kind {
                    CommandKind::Action(action @ PaletteAction::RenameTab) => {
                        record(action, cx);
                        if !self.open_rename_tab(true, window, cx) {
                            cx.emit(SearchEvent::Dismiss);
                        }
                    }
                    CommandKind::Action(action @ PaletteAction::EditTabTags) => {
                        record(action, cx);
                        if let Some((tab_id, _)) = self.catalog.active_tab_rename {
                            self.open_edit_tags_for(tab_id, None, true, window, cx);
                        } else {
                            cx.emit(SearchEvent::Dismiss);
                        }
                    }
                    CommandKind::Action(action @ PaletteAction::OpenThemePicker) => {
                        record(action, cx);
                        self.open_themes(window, cx);
                    }
                    CommandKind::Action(action @ PaletteAction::OpenTabPicker) => {
                        record(action, cx);
                        self.push_picker(
                            PickerScope::Tab(SearchTab::Tabs),
                            String::new(),
                            None,
                            window,
                            cx,
                        );
                    }
                    CommandKind::Action(action @ PaletteAction::OpenTagPicker) => {
                        record(action, cx);
                        self.push_picker(
                            PickerScope::Tab(SearchTab::Tags),
                            String::new(),
                            None,
                            window,
                            cx,
                        );
                    }
                    CommandKind::Action(action @ PaletteAction::SearchHosts) => {
                        record(action, cx);
                        self.push_picker(
                            PickerScope::Tab(SearchTab::Hosts),
                            String::new(),
                            None,
                            window,
                            cx,
                        );
                    }
                    CommandKind::Action(action @ PaletteAction::QuickOpenFile) => {
                        record(action, cx);
                        self.push_picker(
                            PickerScope::Tab(SearchTab::Files),
                            String::new(),
                            None,
                            window,
                            cx,
                        );
                        cx.emit(SearchEvent::EnsureFileIndex);
                    }
                    kind => cx.emit(SearchEvent::Confirm(kind)),
                }
            }
            None => cx.emit(SearchEvent::Dismiss),
        }
    }

    fn on_list_event(
        &mut self,
        list: &Entity<ListState<SearchDelegate>>,
        ev: &ListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ListEvent::Confirm(ix) => self.confirm_row(*ix, window, cx),
            ListEvent::Cancel => self.cancel_or_dismiss(window, cx),
            ListEvent::Select(ix) => match self.active_picker_scope() {
                Some(PickerScope::Tab(SearchTab::Symbols)) => {
                    if let Some(item) = list.read(cx).delegate().item_at(*ix) {
                        match &item.kind {
                            CommandKind::Pick(PickerTarget::GoToSymbol { line, column }) => {
                                if self.symbol_moved || self.symbol_initial.as_ref() != Some(&item.kind) {
                                    self.symbol_moved = true;
                                    cx.emit(SearchEvent::PreviewSymbol {
                                        line: *line,
                                        column: *column,
                                    });
                                }
                            }
                            CommandKind::Pick(PickerTarget::GoToLocation { path, line, column }) => {
                                if self.symbol_moved || self.symbol_initial.as_ref() != Some(&item.kind) {
                                    self.symbol_moved = true;
                                    cx.emit(SearchEvent::PreviewLocation {
                                        path: path.clone(),
                                        line: *line,
                                        column: *column,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(PickerScope::Tab(SearchTab::Locations)) => {
                    if let Some(item) = list.read(cx).delegate().item_at(*ix)
                        && let CommandKind::Pick(PickerTarget::GoToLocation { path, line, column }) =
                            &item.kind
                        && (self.symbol_moved || self.symbol_initial.as_ref() != Some(&item.kind))
                    {
                        self.symbol_moved = true;
                        cx.emit(SearchEvent::PreviewLocation {
                            path: path.clone(),
                            line: *line,
                            column: *column,
                        });
                    }
                }
                Some(PickerScope::Themes) => {
                    if let Some(item) = list.read(cx).delegate().item_at(*ix)
                        && let CommandKind::Pick(PickerTarget::SetTheme(i)) = item.kind
                        && self.previewing != Some(i)
                    {
                        self.previewing = Some(i);
                        cx.emit(SearchEvent::PreviewTheme(i));
                    }
                }
                _ => {}
            },
        }
    }
}

impl EventEmitter<SearchEvent> for SearchView {}
