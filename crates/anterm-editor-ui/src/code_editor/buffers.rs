//! Buffer registry accessors, file loading, untitled creation, and edit tracking.

use std::path::{Path, PathBuf};

use anterm_core::machine::TabId;
use anterm_editor::text::{self as editor_text, EditorConfig, Indent, TextFormat};
use anterm_i18n::{L10nKey, t_fmt};
use gpui::{Context, Window, prelude::*};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{EditorState, InputEditsEvent, InputEvent, Position};

use super::io::{EditorOpenError, Loaded, is_program, load_file, tab_size};
use super::{
    BarKind, BufferId, CURSOR_SCROLL_ATTEMPTS, Fingerprint, LocalFile, OpenFile, TabCode, gutter,
    language_for_path, place_cursor,
};
use crate::{CodeEditorEvent, CodeEditorWorkspace};
use anterm_ui_kit::host_ops::{HostId, HostOps, MTime, SharedHost};

impl CodeEditorWorkspace {
    pub fn tab_code(&self) -> Option<&TabCode> {
        self.active_tab_doc()?.code.as_deref()
    }

    pub fn tab_code_mut(&mut self) -> Option<&mut TabCode> {
        self.active_tab_doc_mut()?.code.as_deref_mut()
    }

    #[cfg(test)]
    pub fn tab_code_mut_or_init(&mut self) -> Option<&mut TabCode> {
        let tab = self.active_tab_doc_mut()?;
        Some(tab.code.get_or_insert_with(|| Box::new(TabCode::new())))
    }

    pub fn code_panel_visible(&self) -> bool {
        self.tab_code().is_some_and(|c| c.visible)
    }

    pub(super) fn buffer(&self, id: BufferId) -> Option<&OpenFile> {
        self.editor.buffers.iter().find(|b| b.id() == id)
    }

    pub(super) fn buffer_mut(&mut self, id: BufferId) -> Option<&mut OpenFile> {
        self.editor.buffers.iter_mut().find(|b| b.id() == id)
    }

    pub(super) fn buffer_at(&self, host: HostId, path: &Path) -> Option<BufferId> {
        self.editor
            .buffers
            .iter()
            .find(|b| b.is_at(host, path))
            .map(OpenFile::id)
    }

    /// The buffer in front of the active tab's editor.
    pub(super) fn active_buffer(&self) -> Option<&OpenFile> {
        self.buffer(self.tab_code()?.active_id()?)
    }

    /// Where the file in front of the active tab's editor lives, if it has
    /// been saved anywhere yet.
    pub fn editor_active_location(&self) -> Option<(HostId, &Path)> {
        let f = self.active_buffer()?;
        f.untitled
            .is_none()
            .then(|| (f.host.id(), f.path.as_path()))
    }

    /// How many places show this buffer: every tab showing it, twice for a
    /// tab whose split shows it in both groups.
    pub(super) fn buffer_refs(&self, id: BufferId) -> usize {
        self.tabs
            .values()
            .filter_map(|t| t.code.as_deref())
            .map(|c| c.views_of(id))
            .sum()
    }

    /// How many tabs show this buffer, in either group.
    pub(super) fn buffer_tabs(&self, id: BufferId) -> usize {
        self.tabs
            .values()
            .filter_map(|t| t.code.as_deref())
            .filter(|c| c.shows(id))
            .count()
    }

    /// Whether the file tree should mark this path as having unsaved edits.
    pub fn editor_dirty_paths(
        &self,
        host: HostId,
        cx: &gpui::App,
    ) -> std::collections::HashSet<PathBuf> {
        let mut out = std::collections::HashSet::new();
        for b in &self.editor.buffers {
            if b.dirty && b.host.id() == host {
                out.insert(b.path.clone());
                out.insert(self.project_spelling(&b.path, cx));
            }
        }
        out
    }

    /// Unsaved buffers that only `tab` shows — the ones closing it
    /// would lose. A buffer another tab also shows survives the close.
    pub fn editor_unsaved_in_tab(&self, tab: TabId) -> Vec<BufferId> {
        let Some(code) = self.tabs.get(&tab).and_then(|t| t.code.as_deref()) else {
            return Vec::new();
        };
        code.all_files()
            .into_iter()
            .filter(|id| self.buffer(*id).is_some_and(|b| b.dirty) && self.buffer_tabs(*id) == 1)
            .collect()
    }

    /// Every unsaved buffer in the window.
    pub fn editor_unsaved(&self) -> Vec<BufferId> {
        self.editor
            .buffers
            .iter()
            .filter(|b| b.dirty)
            .map(OpenFile::id)
            .collect()
    }

    /// Shows what a file link in the grid pointed at.
    ///
    /// A file opens in the editor, on the line the link named; the tree
    /// follows along so "what does it say?" and "where does it live?" are
    /// answered by the same click. A directory has no contents to show, so it
    /// is only ever the tree.
    pub fn open_linked_file(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_dir {
            cx.emit(CodeEditorEvent::ShowFileTree(path.to_path_buf()));
            return;
        }
        self.open_file_in_editor_at(path, line, column, window, cx);
        cx.emit(CodeEditorEvent::RevealInFileTree(path.to_path_buf()));
    }

    /// [`Self::open_file_in_editor`], landing the cursor on a line the caller
    /// already knows — what a `src/main.rs:120:3` in the grid was pointing at.
    pub fn open_file_in_editor_at(
        &mut self,
        path: &Path,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.pending_cursor =
            line.map(|line| (path.to_path_buf(), line, column.unwrap_or(1)));
        self.open_file_in_editor(path, window, cx);
    }

    /// Moves the cursor to the position a link asked for, if the file it asked
    /// about is the one that just opened. Anything else — a different file
    /// opened in between, a file that never arrived — drops the request rather
    /// than throwing the cursor somewhere it was never meant to go.
    fn apply_pending_cursor(
        &mut self,
        id: BufferId,
        requested: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Peeked before it is taken: another file opening in the meantime must
        // not swallow a request that was never about it.
        if self
            .editor
            .pending_cursor
            .as_ref()
            .is_none_or(|(wanted, ..)| wanted != requested)
        {
            return;
        }
        let Some((_, line, column)) = self.editor.pending_cursor.take() else {
            return;
        };
        let Some(file) = self.buffer_mut(id) else {
            return;
        };
        // A line to land on is a place in the source. A Markdown file that
        // would otherwise open rendered (the remembered preview) opens as
        // text here, or the cursor would be placed somewhere nobody can see.
        file.preview = false;
        let input = file.input.clone();
        // The grid counts from one and `Position` counts from zero, and a
        // compiler that says "line 1" means the first line either way.
        let position = Position {
            line: line.saturating_sub(1),
            character: column.saturating_sub(1),
        };
        place_cursor(input, position, CURSOR_SCROLL_ATTEMPTS, window, cx);
    }

    pub fn open_file_in_editor(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        self.editor_open_on_host(host, path, window, cx);
    }

    /// [`Self::open_file_in_editor`] against an explicit host — the SFTP
    /// browser's files live on a host that is never the active one.
    pub fn editor_open_on_host(
        &mut self,
        host: SharedHost,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.active_tab_id() else {
            return;
        };
        self.raise_code_overlay();
        if let Some(id) = self.buffer_at(host.id(), path) {
            self.editor_show_in_tab(tab, id, true, window, cx);
            self.apply_pending_cursor(id, path, window, cx);
            return;
        }
        let p = path.to_path_buf();
        let requested = p.clone();
        let host_id = host.id();
        HostOps::run_in(
            host.clone(),
            window,
            cx,
            move |h| load_file(h, p),
            move |app, opened, window, cx| match opened {
                Ok(loaded) => {
                    let id = app.editor_install(host, loaded, tab, true, window, cx);
                    // Against `requested`, not the loaded path: the host
                    // canonicalised it on the way through, and a link that
                    // named a symlink would otherwise lose the line it asked
                    // for.
                    app.apply_pending_cursor(id, &requested, window, cx);
                }
                Err(EditorOpenError::NotText(path)) => {
                    app.open_outside_the_editor(host_id, &path, window, cx);
                }
                Err(EditorOpenError::Message(message)) => {
                    window.push_notification(message, cx);
                }
            },
        );
    }

    /// What to do with a file the built-in editor cannot show.
    ///
    /// A click on a PNG or a `.zip` meant "open this", not "tell me it is not
    /// text", and on this machine the desktop knows how. A file on another
    /// machine has nobody here to hand it to, so that one gets the words.
    ///
    /// A program does not: `open` on a Mach-O binary runs it, and a build's
    /// output is full of paths to programs. Clicking a word in a terminal
    /// must not be a way to execute one, so those keep the words too.
    fn open_outside_the_editor(
        &mut self,
        host_id: HostId,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !host_id.is_local() || is_program(path) {
            window.push_notification(
                t_fmt(
                    L10nKey::EditorBinaryFile,
                    &[("path", &path.display().to_string())],
                ),
                cx,
            );
            return;
        }
        Self::open_with(path, anterm_ui_kit::platform::open::open_file_path, window, cx);
    }

    /// Runs one of the desktop openers on a local file, and says so when it
    /// fails to spawn — as any opener can (#542).
    pub(super) fn open_with(
        path: &Path,
        opener: fn(&Path) -> std::io::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = opener(path) {
            log::warn!("failed to open {}: {e}", path.display());
            window.push_notification(
                anterm_ui_kit::host_ops::failure(
                    t_fmt(
                        L10nKey::LinkFileOpenFailed,
                        &[
                            ("path", &path.display().to_string()),
                            ("error", &e.to_string()),
                        ],
                    ),
                    &e,
                ),
                cx,
            );
        }
    }

    /// Puts a buffer in a tab's strip and, when `front`, in front of its
    /// editor with the panel open and focused.
    pub(super) fn editor_show_in_tab(
        &mut self,
        tab: TabId,
        id: BufferId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_tab = if self.ctx.live_tabs.contains(&tab) || self.tabs.contains_key(&tab) {
            tab
        } else if let Some(active) = self.active_tab_id() {
            active
        } else {
            return;
        };
        let t = self.tab_doc_mut(target_tab);
        let code = t.code.get_or_insert_with(|| Box::new(TabCode::new()));
        if !front {
            // Listed, not brought forward: the one in front stays in front.
            code.append(id);
            cx.notify();
            return;
        }
        code.show(id);
        code.visible = true;
        if Some(target_tab) == self.active_tab_id() {
            self.editor.bar = None;
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    /// Makes a buffer out of a loaded file, or finds the one already open for
    /// it, and shows it in `tab`.
    pub(super) fn editor_install(
        &mut self,
        host: SharedHost,
        loaded: Loaded,
        tab: TabId,
        front: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BufferId {
        if let Some(id) = self.buffer_at(host.id(), &loaded.path) {
            self.editor_show_in_tab(tab, id, front, window, cx);
            return id;
        }
        let language = language_for_path(&loaded.path);
        let indent = loaded
            .config
            .indent(editor_text::detect_indent(&loaded.text, language));
        let id = self.editor_new_buffer(
            host,
            loaded.path,
            None,
            loaded.text,
            loaded.format,
            loaded.config,
            indent,
            loaded.mtime,
            window,
            cx,
        );
        self.editor_show_in_tab(tab, id, front, window, cx);
        self.editor_rebuild_watcher(cx);
        id
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn editor_new_buffer(
        &mut self,
        host: SharedHost,
        path: PathBuf,
        untitled: Option<u32>,
        text: String,
        format: TextFormat,
        config: EditorConfig,
        indent: Indent,
        mtime: Option<MTime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BufferId {
        let language = if untitled.is_some() {
            "text"
        } else {
            language_for_path(&path)
        };
        let (wrap, preview, helix_on) = {
            let cfg = cx.global::<anterm_ui_kit::config::Config>();
            (
                cfg.editor_soft_wrap,
                cfg.editor_markdown_preview && language == "markdown",
                cfg.helix_mode,
            )
        };
        let saved = Fingerprint::of_str(&text);
        let input = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .tab_size(tab_size(indent))
                .line_number(true)
                .searchable(true)
                .replaceable(true)
                .folding(true)
                .soft_wrap(wrap)
                // Ours is drawn by `editor_body_menu`, like every other menu
                // in the window; the built-in one is a native OS menu.
                .context_menu(false)
                .default_value(text)
        });
        let id = input.entity_id();
        let git_gutter = gutter::BufferGutter::attach(&input, id, cx);
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this: &mut CodeEditorWorkspace, _input, ev, _window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.editor_note_edit(id, cx);
                }
            },
        );
        let edits_sub = cx.subscribe_in(
            &input,
            window,
            move |this: &mut CodeEditorWorkspace, _input, ev: &InputEditsEvent, _window, _cx| {
                this.editor_nav_apply_edits(id, ev);
            },
        );
        let observe = cx.observe(&input, |_, _, cx| cx.notify());
        self.editor.buffers.push(OpenFile {
            path,
            untitled,
            host,
            input,
            dirty: false,
            saved,
            format,
            saved_format: format,
            config,
            indent,
            disk_mtime: mtime,
            saving: false,
            save_pending: false,
            save_then_close: false,
            reload_seq: 0,
            conflict: None,
            preview,
            wrap,
            preview_scroll: gpui::ScrollHandle::new(),
            gutter: git_gutter,
            helix: anterm_editor::helix::HelixState::new(if helix_on {
                anterm_editor::helix::HelixMode::Normal
            } else {
                anterm_editor::helix::HelixMode::Insert
            }),
            _sub: sub,
            _edits_sub: edits_sub,
            _observe: observe,
        });
        id
    }

    /// Synchronize all open editor buffers when `helix_mode` is toggled in Settings.
    pub fn sync_editor_helix_mode(&mut self, on: bool) {
        let mode = if on {
            anterm_editor::helix::HelixMode::Normal
        } else {
            anterm_editor::helix::HelixMode::Insert
        };
        for f in &mut self.editor.buffers {
            f.helix.reset(mode);
        }
    }

    /// Re-derives whether a buffer differs from what is on disk, after an
    /// edit of any kind — typing, undo, a reload, a save rule.
    pub(super) fn editor_note_edit(&mut self, id: BufferId, cx: &mut Context<Self>) {
        let Some(f) = self.buffer(id) else {
            return;
        };
        let state = f.input.read(cx);
        let text = state.text();
        let len = text.len();
        let same_text = len == f.saved.len && Fingerprint::of_chunks(len, text.chunks()) == f.saved;
        let dirty = !same_text || f.format != f.saved_format;
        if let Some(f) = self.buffer_mut(id) {
            f.dirty = dirty;
        }
        self.lsp_buffer_edited(id, cx);
        self.editor_gutter_note_edit(id, cx);
        self.editor_nav_note_edit(id, cx);
        cx.notify();
    }

    /// Saved files on this machine — what a language server can read
    /// (`ui::lsp`). Untitled buffers and files on other hosts are not.
    pub fn editor_local_files(&self) -> Vec<LocalFile> {
        self.editor
            .buffers
            .iter()
            .filter_map(OpenFile::local)
            .collect()
    }

    pub fn editor_local_file(&self, id: BufferId) -> Option<LocalFile> {
        self.buffer(id).and_then(OpenFile::local)
    }

    pub fn editor_active_local_file(&self) -> Option<LocalFile> {
        self.active_buffer().and_then(OpenFile::local)
    }

    /// Asks for a new name for the symbol at `offset`, in the bar above the
    /// text; `ui::lsp` does the renaming once it is answered.
    pub fn editor_open_rename_bar(
        &mut self,
        id: BufferId,
        offset: usize,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_open_bar(BarKind::Rename { id, offset }, current, window, cx);
    }

    /// A new, empty, never-saved buffer, on the window's own machine.
    pub fn editor_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.active_host(cx) else {
            return;
        };
        let Some(tab) = self.active_tab_id() else {
            return;
        };
        self.raise_code_overlay();
        let n = self.editor.next_untitled;
        self.editor.next_untitled += 1;
        let indent = editor_text::detect_indent("", "text");
        let id = self.editor_new_buffer(
            host,
            PathBuf::new(),
            Some(n),
            String::new(),
            TextFormat::default(),
            EditorConfig::default(),
            indent,
            None,
            window,
            cx,
        );
        self.editor_show_in_tab(tab, id, true, window, cx);
    }

}
