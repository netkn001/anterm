//! Standalone deep-module entity owning all code editor buffers, per-tab
//! document state (`TabCode`, `DiffOverlayView`, `OverlayTop`, per-tab
//! `DocumentLayout`), split groups, navigation history, Problems pane, and
//! document-column rendering.
//!
//! `AntermApp` syncs a narrow [`EditorWorkspaceContext`] snapshot and subscribes
//! to [`CodeEditorEvent`] — zero reverse imports into `ui::workspace`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anterm_core::machine::TabId;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, Render, WeakEntity, Window, div, prelude::*,
};

use anterm_ui_kit::config::{Config, DocumentLayout};
use anterm_ui_kit::config_store::ConfigStore;
use anterm_ui_kit::search::SearchTab;
use crate::code_editor::{AfterUnsaved, EditorPanelState, TabCode};
use crate::document_column::{DocumentChrome, OverlayTop};
use anterm_files_ui::diff_overlay::DiffOverlayView;
use anterm_files_ui::file_tree::FileTreeView;
use anterm_ui_kit::host_ops::{HostId, SharedHost};

/// Per-tab document state owned by [`CodeEditorWorkspace`] keyed by [`TabId`].
#[derive(Default)]
pub struct TabDocumentState {
    pub code: Option<Box<TabCode>>,
    pub diff_overlay: Option<Entity<DiffOverlayView>>,
    pub overlay_top: OverlayTop,
    pub document_layout: Option<DocumentLayout>,
}

/// Narrow workspace snapshot pushed by the hosting window before rendering or
/// dispatching editor commands.
#[derive(Clone)]
pub struct EditorWorkspaceContext {
    pub active_tab: Option<TabId>,
    pub live_tabs: Vec<TabId>,
    pub active_tab_label: String,
    pub spawn_host: HostId,
    pub active_host: Option<SharedHost>,
    pub active_tree_roots: Vec<PathBuf>,
    pub left_panel_open: bool,
    pub right_panel_open: bool,
    pub sidebar_px: f32,
    pub right_panel_px: f32,
    pub file_tree: Option<WeakEntity<FileTreeView>>,
    pub open_search_tab: Option<SearchTab>,
}

impl Default for EditorWorkspaceContext {
    fn default() -> Self {
        Self {
            active_tab: None,
            live_tabs: Vec::new(),
            active_tab_label: String::new(),
            spawn_host: HostId::LOCAL,
            active_host: None,
            active_tree_roots: Vec::new(),
            left_panel_open: false,
            right_panel_open: false,
            sidebar_px: 0.,
            right_panel_px: 0.,
            file_tree: None,
            open_search_tab: None,
        }
    }
}

/// How [`CodeEditorWorkspace::render`] paints its active document surface on
/// the current frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DocumentRenderMode {
    #[default]
    Hidden,
    Docked {
        width: f32,
        chrome: DocumentChrome,
    },
    Fill,
}

/// Narrow upward events emitted from [`CodeEditorWorkspace`] to the hosting
/// workspace (`AntermApp`).
pub enum CodeEditorEvent {
    /// Return keyboard focus to the active terminal pane.
    FocusActiveTerminal,
    /// Record which terminal pane was focused before opening a document surface.
    RememberActivePane,
    /// Attach a file path (and optional `#L...` line suffix) to the active agent.
    AttachPathToAgent { path: PathBuf, suffix: String },
    /// Open the command/file/locations search modal on a specific tab.
    OpenSearch { tab: SearchTab, query: String },
    /// Open the search modal on the Locations tab and populate it atomically.
    OpenSearchLocations {
        heading: Option<&'static str>,
        items: Vec<anterm_ui_kit::search::Item>,
    },
    /// Open the search modal on the Symbols tab and populate it atomically.
    OpenSearchSymbols {
        symbols: Vec<(usize, anterm_ui_kit::search::Item)>,
        selected: Option<anterm_ui_kit::search::CommandKind>,
        live_query: Option<anterm_ui_kit::search::LiveQuery>,
    },
    /// Update project-wide LSP symbols inside an open Symbols search modal.
    SetProjectSymbols(Vec<anterm_ui_kit::search::Item>),
    /// Close the search modal if open.
    CloseSearch,
    /// Switch the right panel to Files and focus global content search.
    FocusGlobalSearch,
    /// Open the right panel on the file tree and focus `path` (e.g. directory link).
    ShowFileTree(PathBuf),
    /// Reveal a file path in the file tree if open.
    RevealInFileTree(PathBuf),
    /// Cycle focus to the next terminal pane (e.g. Helix `C-w w` when not split).
    CyclePane,
    /// Code panel became visible — refresh file-tree roots, and if `empty`
    /// ensure the Files panel is visible and focused.
    CodePanelShown { empty: bool },
    /// Deferred workspace close/quit after an unsaved-files prompt resolved.
    ContinueAfterUnsaved(AfterUnsaved),
}

/// Standalone GPUI entity for the code editor and docked/filled document column.
pub struct CodeEditorWorkspace {
    pub editor: EditorPanelState,
    pub tabs: HashMap<TabId, TabDocumentState>,
    pub ctx: EditorWorkspaceContext,
    pub render_mode: DocumentRenderMode,
}

impl EventEmitter<CodeEditorEvent> for CodeEditorWorkspace {}

impl CodeEditorWorkspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            editor: EditorPanelState::new(window, cx),
            tabs: HashMap::new(),
            ctx: EditorWorkspaceContext::default(),
            render_mode: DocumentRenderMode::Hidden,
        }
    }

    /// Replace the current workspace snapshot (active tab, host, side-panel
    /// geometry, sibling entity handles).
    pub fn set_workspace_context(&mut self, ctx: EditorWorkspaceContext) {
        self.ctx = ctx;
    }

    /// Update the render slot chosen by the hosting window for this frame.
    pub fn set_render_mode(&mut self, mode: DocumentRenderMode) {
        self.render_mode = mode;
    }

    pub fn active_tab_id(&self) -> Option<TabId> {
        self.ctx.active_tab
    }

    pub fn active_tab_doc(&self) -> Option<&TabDocumentState> {
        self.ctx.active_tab.and_then(|id| self.tabs.get(&id))
    }

    pub fn active_tab_doc_mut(&mut self) -> Option<&mut TabDocumentState> {
        let id = self.ctx.active_tab?;
        Some(self.tabs.entry(id).or_default())
    }

    pub fn tab_doc(&self, tab: TabId) -> Option<&TabDocumentState> {
        self.tabs.get(&tab)
    }

    pub fn tab_doc_mut(&mut self, tab: TabId) -> &mut TabDocumentState {
        self.tabs.entry(tab).or_default()
    }

    pub fn active_diff_overlay(&self) -> Option<Entity<DiffOverlayView>> {
        self.active_tab_doc()?.diff_overlay.clone()
    }

    pub fn active_has_diff_overlay(&self) -> bool {
        self.active_tab_doc()
            .is_some_and(|t| t.diff_overlay.is_some())
    }

    /// Whether the active tab has a document surface filling the workspace
    /// (which occludes the terminal panes beneath it).
    pub fn active_overlay_occludes(&self, cx: &App) -> bool {
        let Some(tab) = self.active_tab_doc() else {
            return false;
        };
        let has_doc = tab.diff_overlay.is_some()
            || tab.code.as_ref().is_some_and(|c| c.visible);
        if !has_doc {
            return false;
        }
        let layout = tab
            .document_layout
            .unwrap_or_else(|| cx.global::<Config>().document_layout);
        layout == DocumentLayout::Fill
    }

    /// Merge document state from `from` into `into` when a tab is dragged into
    /// another tab's split tree.
    pub fn merge_tab_documents(
        &mut self,
        into: TabId,
        from: TabId,
        cx: &mut Context<Self>,
    ) {
        let Some(moved) = self.tabs.remove(&from) else {
            self.editor_forget_tab(from, cx);
            return;
        };
        let host = self.tabs.entry(into).or_default();
        match (host.code.as_deref_mut(), moved.code) {
            (None, code) => host.code = code,
            (Some(code), Some(theirs)) => code.adopt(&theirs.all_files()),
            (Some(_), None) => {}
        }
        if host.diff_overlay.is_none() {
            host.diff_overlay = moved.diff_overlay;
        }
        self.editor_forget_tab(from, cx);
        cx.notify();
    }

    /// Remove a tab's document state after its panes/files have been closed.
    pub fn remove_tab_state(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.tabs.remove(&tab);
        self.editor_forget_tab(tab, cx);
        cx.notify();
    }

    pub fn spawn_host(&self, _cx: &App) -> HostId {
        self.ctx.spawn_host
    }

    pub fn active_host(&self, _cx: &App) -> Option<SharedHost> {
        self.ctx.active_host.clone()
    }

    pub fn active_tree_roots(&self, _cx: &App) -> Vec<PathBuf> {
        self.ctx.active_tree_roots.clone()
    }

    pub fn left_panel_open(&self, _cx: &App) -> bool {
        self.ctx.left_panel_open
    }

    pub fn right_panel_open(&self, _cx: &App) -> bool {
        self.ctx.right_panel_open
    }

    pub fn sidebar_px(&self, _window: &Window, _cx: &App) -> f32 {
        self.ctx.sidebar_px
    }

    pub fn right_panel_px(&self, _window: &Window, _cx: &App) -> f32 {
        self.ctx.right_panel_px
    }

    pub fn active_tab_label(&self) -> &str {
        &self.ctx.active_tab_label
    }

    pub fn project_spelling(&self, path: &Path, cx: &App) -> PathBuf {
        self.ctx
            .file_tree
            .as_ref()
            .and_then(|ft| ft.upgrade())
            .map(|ft| ft.read(cx).project_spelling(path))
            .unwrap_or_else(|| path.to_path_buf())
    }

    pub fn path_under_tree_root(&self, path: &Path, cx: &App) -> Option<PathBuf> {
        self.ctx
            .file_tree
            .as_ref()
            .and_then(|ft| ft.upgrade())
            .and_then(|ft| ft.read(cx).path_under_tree_root(path))
    }

    pub fn file_tree_contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.ctx
            .file_tree
            .as_ref()
            .and_then(|ft| ft.upgrade())
            .is_some_and(|ft| ft.read(cx).focus_handle_ref().contains_focused(window, cx))
    }

    pub fn file_tree_cancel_edit(&self, cx: &mut App) {
        if let Some(ft) = self.ctx.file_tree.as_ref().and_then(|ft| ft.upgrade()) {
            ft.update(cx, |ft, cx| ft.cancel_edit(cx));
        }
    }

    pub fn open_search_tab(&self) -> Option<SearchTab> {
        self.ctx.open_search_tab
    }

    pub fn update_config(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut Config),
    ) {
        let _ = ConfigStore::update(cx, mutate);
        cx.notify();
    }

    pub fn focus_active(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(CodeEditorEvent::FocusActiveTerminal);
    }

    pub fn remember_active_pane(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(CodeEditorEvent::RememberActivePane);
    }

    pub fn attach_path_to_agent(
        &mut self,
        path: &Path,
        suffix: &str,
        cx: &mut Context<Self>,
    ) {
        cx.emit(CodeEditorEvent::AttachPathToAgent {
            path: path.to_path_buf(),
            suffix: suffix.to_string(),
        });
    }

    pub fn open_search(
        &mut self,
        tab: SearchTab,
        query: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(CodeEditorEvent::OpenSearch {
            tab,
            query: query.to_string(),
        });
    }

    pub fn focus_global_search(&mut self, cx: &mut Context<Self>) {
        cx.emit(CodeEditorEvent::FocusGlobalSearch);
    }

    pub fn render_fill_overlays(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let diff_overlay = self.render_diff_overlay(DocumentChrome::Fill, window, cx);
        let code_overlay = self.render_code_overlay(DocumentChrome::Fill, window, cx);
        let mut pair = vec![
            (OverlayTop::Diff, diff_overlay),
            (OverlayTop::Code, code_overlay),
        ];
        if self
            .active_tab_doc()
            .is_some_and(|t| t.overlay_top == OverlayTop::Diff)
        {
            pair.reverse();
        }
        pair.into_iter().filter_map(|(_, el)| el).collect()
    }

    /// Whether the active tab has any visible document overlay.
    pub fn has_visible_document(&self) -> bool {
        self.document_front().is_some()
    }
}

impl Render for CodeEditorWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.render_mode {
            DocumentRenderMode::Hidden => div().into_any_element(),
            DocumentRenderMode::Docked { width, chrome } => self
                .render_document_column(width, chrome, window, cx)
                .unwrap_or_else(|| div().into_any_element()),
            DocumentRenderMode::Fill => {
                let overlays = self.render_fill_overlays(window, cx);
                div()
                    .relative()
                    .size_full()
                    .children(overlays)
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, px, size};

    /// Build a standalone `CodeEditorWorkspace` hosted under `gpui_kit::component::Root`
    /// with `tab_count` virtual tabs and a real `LocalHost` (no `AntermApp` or PTYs).
    pub fn harness_with_tabs(
        cx: &mut TestAppContext,
        tab_count: usize,
    ) -> (Entity<CodeEditorWorkspace>, Vec<TabId>, VisualTestContext) {
        anterm_ui_kit::config::pin_test_config_dir();
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            anterm_ui_kit::config_store::ConfigStore::init(Config::default(), cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
            anterm_ui_kit::keymap::init(cx);
        });
        let tab_ids: Vec<TabId> = (0..tab_count).map(|_| TabId::new()).collect();
        let initial_tabs = tab_ids.clone();
        let built: std::rc::Rc<std::cell::RefCell<Option<Entity<CodeEditorWorkspace>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let out = built.clone();
        let window = cx.add_window(move |window, cx| {
            let ws = cx.new(|cx| {
                let mut ws = CodeEditorWorkspace::new(window, cx);
                ws.set_workspace_context(EditorWorkspaceContext {
                    active_tab: initial_tabs.first().copied(),
                    live_tabs: initial_tabs.clone(),
                    spawn_host: HostId::LOCAL,
                    active_host: Some(anterm_core::host::local::LocalHost::shared()),
                    ..Default::default()
                });
                ws.set_render_mode(DocumentRenderMode::Fill);
                ws
            });
            *out.borrow_mut() = Some(ws.clone());
            gpui_kit::component::Root::new(ws, window, cx)
        });
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        let vcx = VisualTestContext::from_window(window.into(), cx);
        vcx.simulate_resize(size(px(1200.), px(800.)));
        vcx.run_until_parked();
        let ws = built.borrow().clone().expect("workspace built");
        (ws, tab_ids, vcx)
    }
}
