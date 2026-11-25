use gpui::{App, Axis, Context, Entity, Window, div, prelude::*};
use gpui_kit::component::ActiveTheme as _;

use crate::core::config::Config;
use crate::terminal::view::TerminalView;
use crate::ui::chrome::search::{
    Catalog, ChromeState, CommandGroup, CommandKind, Item, PaletteAction, PickerTarget,
    SearchEvent, SearchTab, SearchView,
};
use crate::ui::chrome::settings::SettingsSection;
use crate::ui::files::scm::ScmIntent;
use crate::ui::workspace::pane::Dir;
use anterm_i18n::{L10nKey, t, t_fmt, t_plural};

use super::session_spawn::mru_order;
use super::{AntermApp, DISCORD_URL, DOCS_URL, ForkPlacement, ISSUES_URL, SpawnWhere};

impl AntermApp {
    /// Everything the command palette and its focused pickers search, gathered
    /// once when the modal opens.
    pub(crate) fn search_catalog(&self, window: &Window, cx: &App) -> Catalog {
        let mut catalog = Catalog::new(
            self.search_actions(window, cx),
            self.search_shells(cx),
            crate::ui::chrome::search::host_items(cx),
        );
        catalog.tabs = self.search_tabs(window, cx);
        catalog.tags = self.search_tags();
        catalog.files = self.file_list_now(cx);
        catalog.active_tab_rename = self.tabs.get(self.active).map(|tab| {
            (
                tab.tree_id.get(),
                self.tab_label(tab, self.active, Some(window), cx),
            )
        });
        catalog.tab_labels = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| (tab.tree_id.get(), self.tab_label(tab, i, Some(window), cx)))
            .collect();
        catalog.sidebar_tags = self.sidebar_tags.clone();
        catalog.tab_tags = self
            .tabs
            .iter()
            .map(|tab| (tab.tree_id.get(), tab.tags.borrow().clone()))
            .collect();
        catalog
    }

    /// Open tabs for `Switch Tab…`: most recently used other tabs first so
    /// pressing Return immediately flips back to the previous tab, with the
    /// active tab last and marked with `✓`.
    pub(crate) fn search_tabs(&self, window: &Window, cx: &App) -> Vec<Item> {
        let stamps: Vec<u64> = self.tabs.iter().map(|t| t.last_used.get()).collect();
        let mut order = mru_order(&stamps, self.active);
        if order.len() > 1 && order.first() == Some(&self.active) {
            let current = order.remove(0);
            order.push(current);
        }
        order
            .into_iter()
            .map(|i| {
                let tab = &self.tabs[i];
                let title = self
                    .full_tab_label(tab, Some(window), cx)
                    .unwrap_or_else(|| self.tab_label(tab, i, Some(window), cx));
                let (view, home) = tab.label_view(Some(window), cx);

                let tag_names: Vec<String> = tab
                    .tags
                    .borrow()
                    .iter()
                    .filter_map(|id| self.sidebar_tags.get(*id).map(|def| def.name.clone()))
                    .collect();

                let mut meta: Vec<String> = Vec::new();
                if !tag_names.is_empty() {
                    meta.push(tag_names.join(", "));
                }
                if let Some(cwd) = view.cwd.as_deref() {
                    let short =
                        crate::ui::workspace::tab_strip::abbreviate_home(cwd, home.as_deref());
                    if !short.is_empty() && short.as_ref() != title {
                        meta.push(short.into_owned());
                    }
                }
                if view.panes > 1 {
                    meta.push(t_fmt(
                        L10nKey::SearchTabPanesCount,
                        &[("count", &view.panes.to_string())],
                    ));
                }

                let mut item = Item::new(title, PickerTarget::GoToTab(tab.tree_id.get()));
                for tag in tag_names {
                    item = item.with_alias(tag);
                }
                if !meta.is_empty() {
                    item = item.with_subtitle(meta.join(" · "));
                }
                if i == self.active {
                    item = item.with_note("✓");
                }
                item
            })
            .collect()
    }

    /// Sidebar tags plus `All` for `Switch Tag…`: inactive targets lead so
    /// pressing Return switches immediately; the currently active filter sits
    /// at the bottom marked with `✓`.
    pub(crate) fn search_tags(&self) -> Vec<Item> {
        if self.sidebar_tags.is_empty() {
            return Vec::new();
        }
        let active_filter = self
            .active_tag_filter
            .filter(|id| self.sidebar_tags.contains(*id));

        let build_all = || {
            let mut item = Item::new(t(L10nKey::SidebarTagsAll), PickerTarget::SelectTag(None))
                .with_aliases_of(L10nKey::SidebarTagsAll)
                .with_aliases_of(L10nKey::SidebarTagShowAll)
                .with_alias("all")
                .with_alias("clear")
                .with_subtitle(t_plural(L10nKey::SearchTagTabsCount, self.tabs.len(), &[]));
            if active_filter.is_none() {
                item = item.with_note("✓");
            }
            item
        };

        let build_tag = |tag: &crate::core::tag::TagDef| {
            let count = self
                .tabs
                .iter()
                .filter(|t| t.tags.borrow().contains(&tag.id))
                .count();
            let mut item = Item::new(tag.name.clone(), PickerTarget::SelectTag(Some(tag.id)))
                .with_tag_color(tag.color)
                .with_subtitle(t_plural(L10nKey::SearchTagTabsCount, count, &[]));
            if active_filter == Some(tag.id) {
                item = item.with_note("✓");
            }
            item
        };

        let mut out = Vec::with_capacity(self.sidebar_tags.tags.len() + 1);
        match active_filter {
            Some(active_id) => {
                out.push(build_all());
                let mut current = None;
                for tag in &self.sidebar_tags.tags {
                    if tag.id == active_id {
                        current = Some(build_tag(tag));
                    } else {
                        out.push(build_tag(tag));
                    }
                }
                if let Some(current) = current {
                    out.push(current);
                }
            }
            None => {
                for tag in &self.sidebar_tags.tags {
                    out.push(build_tag(tag));
                }
                out.push(build_all());
            }
        }
        out
    }

    /// Open `Switch Tab…` — or close it when it is already showing.
    pub(crate) fn toggle_switch_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.clone() {
            if view.read(cx).tab() == SearchTab::Tabs {
                self.close_search(window, cx);
            } else {
                view.update(cx, |view, cx| {
                    view.set_tab(SearchTab::Tabs, Some(""), window, cx)
                });
            }
            return;
        }
        self.open_search(SearchTab::Tabs, "", window, cx);
    }

    /// Open `Switch Tag…` — or close it when it is already showing.
    pub(crate) fn toggle_switch_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.search.clone() {
            if view.read(cx).tab() == SearchTab::Tags {
                self.close_search(window, cx);
            } else {
                view.update(cx, |view, cx| {
                    view.set_tab(SearchTab::Tags, Some(""), window, cx)
                });
            }
            return;
        }
        self.open_search(SearchTab::Tags, "", window, cx);
    }

    /// The Actions tab: the fixed set, plus the rows only this window can
    /// judge are worth offering.
    pub(crate) fn search_actions(&self, window: &Window, cx: &App) -> Vec<Item> {
        let focused = self.focused_leaf(window, cx);
        let has_ssh_session = focused
            .as_ref()
            .is_some_and(|v| v.read(cx).ssh_spec().is_some());
        let has_git_repo = self.right_panel.read(cx).scm.read(cx).active_repo().is_some()
            || focused
                .as_ref()
                .is_some_and(|v| v.read(cx).git_status_cwd().is_some());
        let (document_filled, code_panel_open, editor_palette, editor_can_dispatch) = {
            let ed = self.editor.read(cx);
            (
                ed.document_layout(cx) == crate::core::config::DocumentLayout::Fill,
                ed.code_panel_visible() || ed.active_has_diff_overlay(),
                ed.editor_palette_items(),
                ed.editor_can_dispatch(),
            )
        };
        let mut actions = Item::actions(
            cx,
            ChromeState {
                rail_collapsed: self.sidebar_collapsed,
                right_panel_visible: self.right_panel.read(cx).visible,
                document_filled,
                code_panel_open,
                has_git_repo,
                has_ssh_session,
                has_running_agent: self.agent_target_leaf(cx).is_some(),
            },
        );

        // The editor's navigation, while there is a file in front of it.
        actions.extend(editor_palette);

        // The editor's text commands, while there is a buffer to run them on.
        if editor_can_dispatch {
            actions.extend(
                [
                    Item::localized(
                        L10nKey::CmdEditorTransformUppercase,
                        PaletteAction::EditorTransformUppercase,
                    ),
                    Item::localized(
                        L10nKey::CmdEditorTransformLowercase,
                        PaletteAction::EditorTransformLowercase,
                    ),
                    Item::localized(
                        L10nKey::CmdEditorTransformTitleCase,
                        PaletteAction::EditorTransformTitleCase,
                    ),
                    Item::localized(
                        L10nKey::CmdEditorTrimTrailingWhitespace,
                        PaletteAction::EditorTrimTrailingWhitespace,
                    ),
                    Item::localized(L10nKey::CmdEditorJoinLines, PaletteAction::EditorJoinLines),
                    Item::localized(
                        L10nKey::CmdEditorRemoveSurroundingBrackets,
                        PaletteAction::EditorRemoveSurroundingBrackets,
                    ),
                ]
                .into_iter()
                .map(|item| item.in_group(CommandGroup::View)),
            );
        }

        // Offered only where it would do something. A connection opened from a
        // saved host has nothing to save, and a pane that is not an SSH one has
        // no connection at all — either would be a row that quietly did nothing
        // (#549).
        if self.unsaved_ssh_session(window, cx).is_some() {
            actions.push(
                Item::localized(
                    L10nKey::CmdSshSaveConnection,
                    PaletteAction::SaveSshSessionAsHost,
                )
                .with_subtitle(t(L10nKey::CmdSshSaveConnectionSubtitle))
                .in_group(CommandGroup::Ssh),
            );
        }
        actions
    }

    /// All detected shells on this machine for the `+` menu's `Other Shells…`
    /// picker, default first and then by frecency.
    pub(crate) fn search_shells(&self, cx: &App) -> Vec<Item> {
        let default_shell = self.default_shell_label(cx);
        let usage = &cx.global::<Config>().shell_frecency;
        let now = crate::core::config::unix_now();
        crate::ui::workspace::tab_strip::shells_by_frecency(
            &self.shells.shells,
            &default_shell,
            usage,
            now,
        )
        .into_iter()
        .map(|s| {
            let mut item = Item::new(s.label.clone(), PickerTarget::OpenShell(s.label.clone()))
                .with_alias(s.program.clone());
            if s.label == default_shell {
                item = item.with_subtitle(t(L10nKey::ShellDefault));
            }
            item
        })
        .collect()
    }

    /// Open the command palette directly in the in-place `Edit Tab Tags` page
    /// targeting tab `index`.
    pub(crate) fn open_tag_popover(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.tabs.get(index).map(|t| t.tree_id.get()) else {
            return;
        };
        if self.search.is_some() {
            self.close_search(window, cx);
        }
        self.open_search(SearchTab::Actions, "", window, cx);
        if let Some(view) = self.search.clone() {
            view.update(cx, |view, cx| {
                view.open_edit_tags_for(tab_id, None, false, window, cx);
            });
        }
    }

    fn sync_open_search_tags(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.search.clone() else {
            return;
        };
        let sidebar_tags = self.sidebar_tags.clone();
        let tab_tags = self
            .tabs
            .iter()
            .map(|tab| (tab.tree_id.get(), tab.tags.borrow().clone()))
            .collect();
        let tabs_items = self.search_tabs(window, cx);
        let tags_items = self.search_tags();
        view.update(cx, |view, cx| {
            view.refresh_tag_state(sidebar_tags, tab_tags, tabs_items, tags_items, window, cx);
        });
    }

    /// Open the command palette directly in the in-place `Rename Tab` prompt
    /// targeting the active tab.
    pub(crate) fn open_rename_tab_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        if self.search.is_some() {
            self.close_search(window, cx);
        }
        self.open_search(SearchTab::Actions, "", window, cx);
        if let Some(view) = self.search.clone() {
            view.update(cx, |view, cx| {
                view.open_rename_tab(false, window, cx);
            });
        }
    }

    pub(crate) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            self.close_search(window, cx);
            return;
        }
        self.open_search(SearchTab::Actions, "", window, cx);
    }

    /// Open the command palette or a focused picker on `tab` with `query`.
    pub(crate) fn open_search(
        &mut self,
        tab: SearchTab,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let catalog = self.search_catalog(window, cx);
        let view = cx.new(|cx| SearchView::new(catalog, tab, query, window, cx));
        if tab.in_editor_row() {
            let tabs = self.editor.read(cx).editor_search_tabs();
            view.update(cx, |view, cx| view.set_editor_tabs(tabs, cx));
        }
        cx.subscribe_in(&view, window, Self::on_search_event)
            .detach();
        self.search = Some(view);
        self.sync_editor_context(Some(window), cx);
        if tab == SearchTab::Files {
            self.refresh_file_index(window, cx);
        }
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        _view: &Entity<SearchView>,
        ev: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            SearchEvent::Confirm(kind) => {
                let kind = kind.clone();
                // The picker is already showing this theme; keep it through the
                // close instead of reverting and re-applying it.
                if matches!(kind, CommandKind::Pick(PickerTarget::SetTheme(_))) {
                    self.theme_preview_restore = None;
                }
                // Likewise the symbol Go to Symbol has the caret on.
                if matches!(
                    kind,
                    CommandKind::Pick(
                        PickerTarget::GoToSymbol { .. } | PickerTarget::GoToLocation { .. }
                    )
                ) {
                    self.editor
                        .update(cx, |ed, _| ed.editor_symbol_preview_commit());
                }
                self.close_search(window, cx);
                self.run_command(kind, window, cx);
            }
            SearchEvent::RecordUsage(id) => self.bump_command_frecency_id(id, cx),
            SearchEvent::EnsureFileIndex => self.refresh_file_index(window, cx),
            SearchEvent::RenameTab {
                tab,
                value,
                prefill,
            } => {
                let (tab, value, prefill) = (*tab, value.clone(), prefill.clone());
                self.close_search(window, cx);
                self.apply_tab_rename(tab, &value, &prefill, window, cx);
            }
            SearchEvent::ToggleTabTag { tab, tag } => {
                let (tab, tag) = (*tab, *tag);
                if let Some(idx) = self.tabs.iter().position(|t| t.tree_id.get() == tab) {
                    self.toggle_tab_tag(idx, tag, cx);
                    self.sync_open_search_tags(window, cx);
                }
            }
            SearchEvent::CreateAndAttachTag { tab, name, color } => {
                let (tab, name, color) = (*tab, name.clone(), *color);
                if let Some(idx) = self.tabs.iter().position(|t| t.tree_id.get() == tab) {
                    self.create_and_attach_tag(idx, &name, color, cx);
                    self.sync_open_search_tags(window, cx);
                }
            }
            SearchEvent::SetTagColor { tag, color } => {
                let (tag, color) = (*tag, *color);
                self.set_tag_color(tag, color, cx);
                self.sync_open_search_tags(window, cx);
            }
            SearchEvent::RenameSidebarTag { tag, name } => {
                let (tag, name) = (*tag, name.clone());
                self.rename_tag(tag, &name, cx);
                self.sync_open_search_tags(window, cx);
            }
            SearchEvent::DeleteSidebarTag { tag } => {
                let tag = *tag;
                self.delete_tag(tag, cx);
                self.sync_open_search_tags(window, cx);
            }
            SearchEvent::ConfirmSplit(kind) => {
                let kind = kind.clone();
                self.close_search(window, cx);
                match kind {
                    CommandKind::Pick(PickerTarget::OpenShell(label)) => {
                        self.open_listed_shell(&label, SpawnWhere::Split, window, cx)
                    }
                    CommandKind::Pick(PickerTarget::ConnectSavedProfile(id)) => {
                        self.connect_ssh_profile_at(id, SpawnWhere::Split, window, cx)
                    }
                    other => self.run_command(other, window, cx),
                }
            }
            SearchEvent::Dismiss => self.close_search(window, cx),
            SearchEvent::PreviewTheme(i) => {
                if let Some(id) = crate::ui::chrome::presets::all(cx).get(*i).map(|t| t.id.clone()) {
                    self.preview_preset(&id, window, cx);
                }
            }
            SearchEvent::CancelThemePreview => self.cancel_preset_preview(window, cx),
            SearchEvent::PreviewSymbol { line, column } => {
                self.editor
                    .update(cx, |ed, cx| ed.editor_symbol_preview(*line, *column, cx));
            }
            SearchEvent::PreviewLocation { path, line, column } => {
                self.editor.update(cx, |ed, cx| {
                    ed.lsp_preview_location(path, *line, *column, cx)
                });
            }
            // Each editor tab is opened the way its own chord opens it, so
            // it arrives set up: the outline around the caret, the server
            // to ask. Closing first puts back a caret Go to Symbol moved.
            SearchEvent::SwitchEditorTab(tab) => {
                self.close_search(window, cx);
                match tab {
                    SearchTab::Symbols => {
                        self.sync_editor_context(Some(window), cx);
                        self.editor
                            .update(cx, |ed, cx| ed.editor_go_to_symbol(window, cx));
                    }
                    _ => self.open_search(SearchTab::Files, "", window, cx),
                }
            }
        }
    }

    pub(crate) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.sync_editor_context(Some(window), cx);
        // A previewed theme was never persisted: closing the search any way
        // other than confirming the pick puts the old one back.
        self.cancel_preset_preview(window, cx);
        self.focus_active(window, cx);
        // Nor was a symbol Go to Symbol previewed: the caret goes back, and
        // the keyboard with it to the editor it came from.
        self.editor
            .update(cx, |ed, cx| ed.editor_symbol_preview_cancel(window, cx));
        cx.notify();
    }

    pub(super) fn focused_leaf(&self, window: &Window, cx: &App) -> Option<Entity<TerminalView>> {
        self.tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))
    }

    fn bump_command_frecency_id(&mut self, id: &str, cx: &mut Context<Self>) {
        self.update_config(cx, |cfg| {
            let entry = cfg.command_frecency.entry(id.to_string()).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.last_used = crate::core::config::unix_now();
        });
    }

    fn bump_command_frecency(&mut self, kind: &CommandKind, cx: &mut Context<Self>) {
        let Some(id) = kind.id() else { return };
        self.bump_command_frecency_id(id, cx);
    }

    fn run_command(&mut self, kind: CommandKind, window: &mut Window, cx: &mut Context<Self>) {
        self.bump_command_frecency(&kind, cx);
        match kind {
            CommandKind::Action(action) => self.run_palette_action(action, window, cx),
            CommandKind::Pick(target) => self.run_picker_target(target, window, cx),
        }
    }

    fn run_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use PaletteAction::*;
        match action {
            NewTab => self.new_tab(window, cx),
            NewWindow => self.new_window(cx),
            CloseWindow => self.close_window(window, cx),
            SplitRight => self.split_in(Axis::Horizontal, false, window, cx),
            SplitLeft => self.split_in(Axis::Horizontal, true, window, cx),
            SplitDown => self.split_in(Axis::Vertical, false, window, cx),
            SplitUp => self.split_in(Axis::Vertical, true, window, cx),
            ClosePane => self.close_pane(window, cx),
            NextPane => self.cycle_pane(true, window, cx),
            PrevPane => self.cycle_pane(false, window, cx),
            FocusPaneLeft => self.focus_pane_dir(Dir::Left, window, cx),
            FocusPaneRight => self.focus_pane_dir(Dir::Right, window, cx),
            FocusPaneUp => self.focus_pane_dir(Dir::Up, window, cx),
            FocusPaneDown => self.focus_pane_dir(Dir::Down, window, cx),
            ResizePaneLeft => self.resize_pane(Dir::Left, window, cx),
            ResizePaneRight => self.resize_pane(Dir::Right, window, cx),
            ResizePaneUp => self.resize_pane(Dir::Up, window, cx),
            ResizePaneDown => self.resize_pane(Dir::Down, window, cx),
            SwapPaneNext => self.swap_pane(true, window, cx),
            SwapPanePrev => self.swap_pane(false, window, cx),
            SelectNextTab => self.cycle_tab(true, window, cx),
            SelectPrevTab => self.cycle_tab(false, window, cx),
            MoveTabLeft => self.move_tab(false, cx),
            MoveTabRight => self.move_tab(true, cx),
            ToggleMaximizePane => self.toggle_maximize(window, cx),
            ToggleFullscreen => self.toggle_fullscreen(window, cx),
            ToggleTabSidebar => self.toggle_tab_sidebar(cx),
            ToggleLeftPanel => self.toggle_left_panel(cx),
            ToggleRightPanel => self.toggle_right_panel(cx),
            ShowRightPanel(tab) => self.set_right_panel_tab(tab, cx),
            ResetFontSize => self.reset_font_size(cx),
            FindInTerminal => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.open_search(window, cx));
                }
            }
            ToggleComposer => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.toggle_composer(window, cx));
                }
            }
            FindNext => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.find_step(true, cx));
                }
            }
            FindPrevious => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.find_step(false, cx));
                }
            }
            ClearTerminal => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.clear_scrollback(cx));
                }
            }
            CopyText => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| {
                        view.copy_contextual(false, false, cx);
                    });
                }
            }
            CopyRaw => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| {
                        view.copy_contextual(false, true, cx);
                    });
                }
            }
            CutText => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| {
                        view.cut_contextual(cx);
                    });
                }
            }
            PasteText => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.paste_from_clipboard(cx));
                }
            }
            SelectAllText => {
                if let Some(leaf) = self.focused_leaf(window, cx) {
                    leaf.update(cx, |view, cx| view.select_all_contextual(cx));
                }
            }
            ReopenClosedTab => self.reopen_closed_tab(window, cx),
            RenameTab => self.open_rename_tab_prompt(window, cx),
            EditTabTags => self.open_tag_popover(self.active, window, cx),
            CloseOtherTabs => self.close_other_tabs(self.active, window, cx),
            CloseTabsToTheRight => self.close_tabs_right_of(self.active, window, cx),
            CopyWorkingDirectory => self.copy_active_cwd(window, cx),
            MarkTabUnread => self.mark_tab_unread(self.active, cx),
            HibernateTab => self.hibernate_tab(self.active, window, cx),
            ForkAgentSession => self.fork_active_pane_session(ForkPlacement::NewTab, window, cx),
            FocusNextAgentNeedingAttention => self.focus_next_agent_needing_attention(window, cx),
            CopyAgentSessionId => self.copy_agent_session_id(self.active, window, cx),
            OpenSettings => self.toggle_settings(window, cx),
            ShowKeyboardShortcuts => {
                self.open_settings_section(SettingsSection::Keybindings, window, cx)
            }
            About => self.open_settings_section(SettingsSection::About, window, cx),
            CheckForUpdates => self.check_for_updates_now(window, cx),
            OpenDocumentation => cx.open_url(DOCS_URL),
            OpenDiscord => cx.open_url(DISCORD_URL),
            ReportIssue => cx.open_url(ISSUES_URL),
            // Same exit as the tray's: confirm, then stop the server with the
            // app. A bare quit would leave the daemon orphaned behind a dead
            // tray icon.
            Quit => self.quit_stop_sessions(window, cx),
            RestartDaemon => self.restart_window_daemon(window, cx),
            UpdateLocalServer => self.update_local_server(window, cx),
            ToggleSftp => self.toggle_sftp(window, cx),
            ShowSshForwards => self.show_ssh_forwards(window, cx),
            ToggleCodePanel => self.toggle_code_panel(window, cx),
            ToggleDocumentFill => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.toggle_document_fill(cx))
            }
            DocumentWidthThird => {
                self.sync_editor_context(Some(window), cx);
                self.editor.update(cx, |ed, cx| {
                    ed.set_document_ratio(crate::core::config::DOCUMENT_RATIO_THIRD, cx)
                })
            }
            DocumentWidthHalf => {
                self.sync_editor_context(Some(window), cx);
                self.editor.update(cx, |ed, cx| {
                    ed.set_document_ratio(crate::core::config::DOCUMENT_RATIO_HALF, cx)
                })
            }
            DocumentWidthTwoThirds => {
                self.sync_editor_context(Some(window), cx);
                self.editor.update(cx, |ed, cx| {
                    ed.set_document_ratio(crate::core::config::DOCUMENT_RATIO_TWO_THIRDS, cx)
                })
            }
            ToggleDocumentPreview => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.toggle_document_preview(cx))
            }
            ToggleDocumentWrap => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.toggle_document_wrap(window, cx))
            }
            EditorTransformUppercase => self
                .editor
                .update(cx, |ed, cx| ed.editor_transform_uppercase(window, cx)),
            EditorTransformLowercase => self
                .editor
                .update(cx, |ed, cx| ed.editor_transform_lowercase(window, cx)),
            EditorTransformTitleCase => self
                .editor
                .update(cx, |ed, cx| ed.editor_transform_title_case(window, cx)),
            EditorTrimTrailingWhitespace => self
                .editor
                .update(cx, |ed, cx| ed.editor_trim_trailing_whitespace(window, cx)),
            EditorJoinLines => self
                .editor
                .update(cx, |ed, cx| ed.editor_join_lines(window, cx)),
            EditorRemoveSurroundingBrackets => self.editor.update(cx, |ed, cx| {
                ed.editor_remove_surrounding_brackets(window, cx)
            }),
            EditorGoToSymbol => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.editor_go_to_symbol(window, cx))
            }
            EditorNavigateBack => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.editor_navigate(false, window, cx))
            }
            EditorNavigateForward => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.editor_navigate(true, window, cx))
            }
            EditorSplitRight => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| {
                        ed.editor_split(window, cx);
                    });
            }
            RestartSshSession => self.restart_ssh_session(window, cx),
            SaveSshSessionAsHost => self.save_ssh_session_as_host(window, cx),
            OpenSshProfiles => self.open_settings_section(SettingsSection::Ssh, window, cx),
            SendSelectionToAgent => self.send_selection_to_agent(window, cx),
            SendGitDiffToAgent => self.send_git_diff_to_agent(window, cx),
            ScmCommit => self.run_scm_action(ScmIntent::Commit, window, cx),
            ScmStageAll => self.run_scm_action(ScmIntent::StageAll, window, cx),
            ScmUnstageAll => self.run_scm_action(ScmIntent::UnstageAll, window, cx),
            ScmDiscardAll => self.run_scm_action(ScmIntent::DiscardAll, window, cx),
            ScmPush => self.run_scm_action(ScmIntent::Push, window, cx),
            ScmPull => self.run_scm_action(ScmIntent::Pull, window, cx),
            ScmFetch => self.run_scm_action(ScmIntent::Fetch, window, cx),
            ScmSync => self.run_scm_action(ScmIntent::Sync, window, cx),
            ScmCreateBranch => self.run_scm_action(ScmIntent::CreateBranch, window, cx),
            OpenBranchPicker => self.run_scm_action(ScmIntent::CheckoutBranch, window, cx),
            ToggleDiffViewMode => self.toggle_diff_view_mode(cx),
            OpenThemePicker => {}
            OpenTabPicker => self.open_search(SearchTab::Tabs, "", window, cx),
            OpenTagPicker => self.open_search(SearchTab::Tags, "", window, cx),
            SearchHosts => self.open_search(SearchTab::Hosts, "", window, cx),
            QuickOpenFile => self.open_search(SearchTab::Files, "", window, cx),
        }
    }

    fn run_picker_target(
        &mut self,
        target: PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use PickerTarget::*;
        match target {
            SetTheme(i) => {
                if let Some(id) = crate::ui::chrome::presets::all(cx).get(i).map(|t| t.id.clone()) {
                    self.set_preset(&id, window, cx);
                }
            }
            OpenSshConnect(input) => self.open_typed_ssh_connect(&input, window, cx),
            ConnectSavedProfile(id) => self.connect_ssh_profile(id, window, cx),
            // ⌥ held as the row is taken splits, as it does on the menu's row.
            OpenShell(label) => {
                let at = SpawnWhere::from_modifiers(window.modifiers());
                self.open_listed_shell(&label, at, window, cx)
            }
            EditSavedProfile(id) => self.open_ssh_profile_in_settings(id, window, cx),
            QuickConnect(target) => {
                if let Some(qc) = crate::core::ssh_profile::parse_quick_connect(&target) {
                    self.quick_connect(qc, window, cx);
                }
            }
            SaveQuickConnect(target) => self.open_ssh_profile_new_from_target(target, window, cx),
            GoToTab(id) => {
                if let Some(i) = self.tabs.iter().position(|t| t.tree_id.get() == id) {
                    if let Some(filter) = self.active_tag_filter
                        && !self.tabs[i].tags.borrow().contains(&filter)
                    {
                        self.active_tag_filter = None;
                    }
                    self.activate(i, window, cx);
                }
            }
            SelectTag(tag) => self.switch_tag(tag, window, cx),
            OpenFile { path, line, column } => {
                self.open_indexed_file(&path, line, column, window, cx)
            }
            GoToSymbol { line, column } => self
                .editor
                .update(cx, |ed, cx| ed.editor_go_to_position(line, column, window, cx)),
            GoToLocation { path, line, column } => {
                self.open_file_in_editor_at(&path, Some(line + 1), Some(column + 1), window, cx)
            }
        }
    }

    pub(crate) fn agent_target_leaf(&self, cx: &App) -> Option<Entity<TerminalView>> {
        let runs_agent = |leaf: &Entity<TerminalView>| leaf.read(cx).agent().is_some();
        if let Some(tab) = self.tabs.get(self.active)
            && let Some(leaf) = tab.pane.terminals().into_iter().find(runs_agent)
        {
            return Some(leaf);
        }
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active)
            .flat_map(|(_, t)| t.pane.terminals())
            .find(runs_agent)
    }

    fn deliver_agent_prompt(&mut self, prompt: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.agent_target_leaf(cx) else {
            crate::platform::notify::notify_desktop(Some("anterm"), t(L10nKey::AppNoRunningCodingAgent));
            return;
        };
        target.read(cx).send_agent_prompt(prompt);
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.pane.terminals().contains(&target))
        {
            self.activate(i, window, cx);
        }
    }

    fn send_selection_to_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx));
        let (selection, cwd) = match &source {
            Some(view) => (view.read(cx).selection_text(), view.read(cx).cwd()),
            None => (None, None),
        };
        let Some(selection) = selection else {
            crate::platform::notify::notify_desktop(Some("anterm"), t(L10nKey::AppNothingSelected));
            return;
        };
        let cwd = cwd.map(|c| c.to_string_lossy().into_owned());
        if let Some(prompt) =
            crate::core::agent_prompt::build_selection_prompt(&selection, cwd.as_deref())
        {
            self.deliver_agent_prompt(&prompt, window, cx);
        }
    }

    fn send_git_diff_to_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx));
        let target = pane.and_then(|view| {
            let view = view.read(cx);
            Some((view.host(cx)?, view.host_cwd()?))
        });
        let Some((host, cwd)) = target else {
            crate::platform::notify::notify_desktop(Some("anterm"), t(L10nKey::AppPaneNoKnownDirectory));
            return;
        };
        crate::ui::remote::host_ops::HostOps::run_in(
            host,
            window,
            cx,
            move |h| {
                let run = |args: &[&str]| {
                    h.git(&cwd, args)
                        .ok()
                        .filter(|o| o.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                        .unwrap_or_default()
                };
                let diff = format!("{}{}", run(&["diff"]), run(&["diff", "--cached"]));
                (diff, cwd.to_string_lossy().into_owned())
            },
            move |this, (diff, cwd_s), window, cx| {
                match crate::core::agent_prompt::build_diff_review_prompt(&diff, Some(&cwd_s)) {
                    Some(prompt) => this.deliver_agent_prompt(&prompt, window, cx),
                    None => crate::platform::notify::notify_desktop(
                        Some("anterm"),
                        &t_fmt(L10nKey::AppNoUncommittedChanges, &[("cwd", &cwd_s)]),
                    ),
                }
            },
        );
    }

    /// One pill per pane in the active tab that a phone is running at its own
    /// size. The window keeps its grid meanwhile and shows the pane's output
    /// laid out for the phone, so it says why, and offers the pane back.
    pub(super) fn render_lease_notices(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        use gpui_kit::component::Sizable as _;
        use gpui_kit::component::button::ButtonVariants as _;
        let Some(tab) = self.tabs.get(self.active) else {
            return Vec::new();
        };
        let theme = cx.theme();
        let (info, foreground, popover) = (theme.info, theme.foreground, theme.popover);
        tab.pane
            .terminals()
            .into_iter()
            .filter_map(|leaf| {
                let by = leaf.read(cx).terminal.leased_by()?;
                let id = leaf.entity_id().as_u64();
                Some(
                    crate::ui::chrome::notice::pill(info, cx)
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(foreground)
                                .child(anterm_i18n::t_fmt(
                                    anterm_i18n::L10nKey::PaneLeasedBy,
                                    &[("by", &by)],
                                )),
                        )
                        .child(
                            gpui_kit::component::button::Button::new(gpui::SharedString::from(format!(
                                "lease-take-back-{id}"
                            )))
                            .label(anterm_i18n::t(
                                anterm_i18n::L10nKey::RemoteActionTakeBack,
                            ))
                            .custom(crate::ui::chrome::theme::inverted_button(popover, cx))
                            .small()
                            .on_click(move |_, _, cx| leaf.read(cx).terminal.take_back()),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}
