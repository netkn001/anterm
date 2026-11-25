use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anterm_core::machine::TabId;
use gpui::{Context, Entity, EntityInputHandler as _, SharedString, Subscription, Window, px};
use gpui_kit::component::input::{EditorState, InputState, Position};

use crate::CodeEditorWorkspace;
use crate::editor_session::TabEditor;
use anterm_editor::text::{EditorConfig, Indent, TextFormat};
use anterm_ui_kit::host_ops::{HostId, MTime, SharedHost, WatchSub};
use anterm_i18n::{L10nKey, t_fmt};

mod actions;
mod buffers;
mod commands;
mod gutter;
mod helix;
mod io;
mod menus;
mod nav;
pub mod outline;
mod problems;
mod session;
mod split;
mod strip;
pub mod text_actions;
#[cfg(test)]
mod tests;


pub(super) const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

const RELOAD_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(200);

/// How far up the tree to look for `.editorconfig` files. Each level is one
/// round trip on a remote host, and a project nested deeper than this without
/// a `root = true` somewhere above it is not one worth stalling an open for.
pub(super) const EDITORCONFIG_DEPTH: usize = 16;

/// An open buffer is named by its input's entity id: unique, stable for the
/// buffer's life, and already what every async landing looks it up by.
pub type BufferId = gpui::EntityId;

/// What a buffer's text looked like the last time it matched the disk.
///
/// Dirtiness is a comparison against this rather than a flag set by the first
/// keystroke, so typing a character and deleting it again — or undoing back to
/// the saved text — leaves the file clean, the way every other editor does.
/// The length is checked first: it is free, and it settles almost every
/// keystroke without hashing the whole file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Fingerprint {
    len: usize,
    hash: u64,
}

impl Fingerprint {
    pub(super) fn of_str(text: &str) -> Self {
        Self::of_chunks(text.len(), std::iter::once(text))
    }

    pub(super) fn of_chunks<'a>(len: usize, chunks: impl Iterator<Item = &'a str>) -> Self {
        use std::hash::Hasher as _;
        // SipHash buffers its input, so the same bytes hash the same however
        // the rope happens to have split them into chunks.
        let mut hasher = std::hash::DefaultHasher::new();
        for chunk in chunks {
            hasher.write(chunk.as_bytes());
        }
        Self {
            len,
            hash: hasher.finish(),
        }
    }
}

/// Why the buffer and the file on disk no longer agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskConflict {
    /// Something else wrote the file while the buffer had edits of its own.
    /// Carries the modification time that was seen, so "Keep mine" can accept
    /// exactly that version as the one a save may replace.
    Changed(Option<MTime>),
    /// The file is gone. The buffer is all that is left of it.
    Deleted,
}

/// One open file, shared by every tab that shows it.
///
/// Buffers used to belong to a tab, which meant the same file open in two tabs
/// was two independent copies: edits in one were invisible to the other, and
/// saving either silently threw the other's away. Now a tab only lists which
/// buffers it shows, and the text lives here once.
pub struct OpenFile {
    pub path: PathBuf,
    /// Set for a file that has never been saved; `path` is empty until then.
    pub(super) untitled: Option<u32>,
    /// The machine `path` lives on, held rather than looked up. Saves,
    /// reloads and duplicate detection all key on its id — an SFTP file and a
    /// local file can share the string `/etc/hosts` without being the same
    /// file — and saving goes straight through this handle, so a buffer stays
    /// saveable however the window's own machine has changed underneath it.
    pub host: SharedHost,
    pub input: Entity<EditorState>,
    pub dirty: bool,
    pub(super) saved: Fingerprint,
    /// How the bytes on disk are encoded, so a save writes them back the same
    /// way — a CRLF file stays CRLF, a GB18030 file stays GB18030.
    pub(super) format: TextFormat,
    pub(super) saved_format: TextFormat,
    pub(super) config: EditorConfig,
    pub(super) indent: Indent,
    pub(super) disk_mtime: Option<MTime>,
    pub(super) saving: bool,
    pub(super) save_pending: bool,
    pub(super) save_then_close: bool,
    pub(super) reload_seq: u64,
    pub conflict: Option<DiskConflict>,
    pub preview: bool,
    pub wrap: bool,
    /// The rendered-Markdown pane's own scroll. Per file, so switching away
    /// and back lands where you were reading — and so the pane can carry the
    /// scrollbar every other scrolling surface in anterm has. The editor itself
    /// gets one from `Input`.
    pub preview_scroll: gpui::ScrollHandle,
    /// Git change markers — see `gutter`.
    pub(super) gutter: gutter::BufferGutter,
    /// Per-buffer Helix modal state (`NOR`, `INS`, `SEL`, minor mode, counts, registers).
    pub helix: anterm_editor::helix::HelixState,
    pub(super) _sub: Subscription,
    pub(super) _edits_sub: Subscription,
    pub(super) _observe: Subscription,
}

impl OpenFile {
    pub(super) fn id(&self) -> BufferId {
        self.input.entity_id()
    }

    pub(super) fn label(&self) -> SharedString {
        if let Some(n) = self.untitled {
            return t_fmt(L10nKey::EditorUntitled, &[("n", &n.to_string())]).into();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
            .into()
    }

    pub(super) fn is_at(&self, host: HostId, path: &Path) -> bool {
        self.untitled.is_none() && self.host.id() == host && self.path == path
    }

    pub(super) fn language(&self) -> &'static str {
        if self.untitled.is_some() {
            return "text";
        }
        language_for_path(&self.path)
    }

    pub(super) fn local(&self) -> Option<LocalFile> {
        (self.untitled.is_none() && self.host.id().is_local()).then(|| LocalFile {
            id: self.id(),
            input: self.input.clone(),
            path: self.path.clone(),
            language: self.language(),
            indent: self.indent,
        })
    }
}

/// A saved file on this machine, as the language servers see it.
pub struct LocalFile {
    pub id: BufferId,
    pub input: Entity<EditorState>,
    pub path: PathBuf,
    pub language: &'static str,
    pub indent: Indent,
}

/// One tab's view of the editor: which buffers it shows, in the order its
/// strip draws them, and which one is in front.
pub struct TabCode {
    pub visible: bool,
    pub files: Vec<BufferId>,
    pub active: usize,
    /// The second editor group, when the editor is split — the one without
    /// the focus. `files` and `active` above are always the focused group's
    /// (see `split`).
    pub split: Option<split::OtherGroup>,
    /// Buffers in the order they were last in front, most recent first —
    /// what decides which files keep a place in the header's strip when
    /// it cannot hold them all (see `strip`).
    pub recent: Vec<BufferId>,
}

impl TabCode {
    pub fn new() -> Self {
        Self {
            visible: false,
            files: Vec::new(),
            active: 0,
            split: None,
            recent: Vec::new(),
        }
    }

    pub fn active_id(&self) -> Option<BufferId> {
        self.files.get(self.active).copied()
    }

    /// Adds a buffer just right of the one in front, the way a browser opens
    /// a tab, and brings it forward. One the tab already shows only comes
    /// forward — it keeps its place in the strip.
    pub(super) fn show(&mut self, id: BufferId) {
        if let Some(pos) = self.files.iter().position(|f| *f == id) {
            self.active = pos;
            return;
        }
        let at = if self.files.is_empty() {
            0
        } else {
            (self.active + 1).min(self.files.len())
        };
        self.files.insert(at, id);
        self.active = at;
    }

    /// Lists a buffer at the end of the strip without bringing it forward —
    /// for files arriving in the background: a restore, a merge, a rescue.
    pub(super) fn append(&mut self, id: BufferId) {
        if !self.files.contains(&id) {
            self.files.push(id);
        }
    }

    /// Lists another strip's files after this one's, in their order.
    pub fn adopt(&mut self, ids: &[BufferId]) {
        for id in ids {
            self.append(*id);
        }
    }

    /// Takes a buffer out of the strip. The neighbour that slides into its
    /// place comes forward, so closing tabs one after another walks along the
    /// strip rather than jumping about.
    pub(super) fn forget(&mut self, id: BufferId) -> bool {
        let Some(pos) = self.files.iter().position(|f| *f == id) else {
            return false;
        };
        self.files.remove(pos);
        if self.active > pos || self.active >= self.files.len() {
            self.active = self.active.saturating_sub(1);
        }
        true
    }
}

/// What a question about unsaved files was standing in the way of.
#[derive(Clone, Debug)]
pub enum AfterUnsaved {
    CloseTab(TabId),
    /// Take these files out of that tab's strip — Close Others and Close to
    /// the Right.
    CloseFiles(TabId, Vec<BufferId>),
    ClosePane,
    CloseWindow,
    Quit,
}

/// Saves that something is waiting on: once every buffer in `ids` has landed
/// clean, `then` runs. Any one of them failing drops the wait, and with it
/// the close it would have carried out.
pub(super) struct SaveWaiter {
    pub(super) ids: Vec<BufferId>,
    pub(super) then: AfterUnsaved,
}

pub(super) enum BarKind {
    GoToLine,
    SaveAs {
        id: BufferId,
        then_close: bool,
    },
    /// A new name for the symbol at `offset` (`ui::lsp`).
    Rename {
        id: BufferId,
        offset: usize,
    },
    /// Helix `:` command bar (`:w`, `:q`, `:wq`, `:e`, `:vs`, `:<line>`).
    Command,
    /// Helix `/` search bar.
    HelixSearch,
    /// Helix `s` split-selection bar.
    HelixSplitSelect,
}

/// The one-line prompt that sits above the text: go to line, or name a file
/// to save to on a machine the native save panel cannot browse.
pub(super) struct EditorBar {
    pub(super) kind: BarKind,
    pub(super) input: Entity<InputState>,
    _sub: Subscription,
}

pub struct EditorPanelState {
    /// Every open buffer in this window. Tabs refer to these by id.
    pub(super) buffers: Vec<OpenFile>,
    pub(super) next_untitled: u32,
    /// Where to put the cursor once a particular file is on screen, for a
    /// `file.rs:120:3` that has to load first. Carried rather than applied at
    /// the call site because opening is asynchronous: the click is long over
    /// by the time there is a buffer to put a cursor in.
    pub(super) pending_cursor: Option<(PathBuf, u32, u32)>,
    pub(super) bar: Option<EditorBar>,
    pub(super) waiters: Vec<SaveWaiter>,
    pub(super) unsaved_prompt_open: bool,
    /// Tabs whose remembered files have been reopened (or found to have none).
    pub(super) restored: HashSet<TabId>,
    /// What was last written to the session store for each tab, so an
    /// unchanged tab costs a comparison per frame and nothing more.
    pub(super) recorded: HashMap<TabId, TabEditor>,
    pub(super) watch: Option<Arc<WatchSub>>,
    pub(super) watch_host: Option<SharedHost>,
    pub(super) watch_opening: bool,
    pub(super) watch_busy: bool,
    pub(super) watch_dirty: bool,
    pub(super) watched_dirs: HashSet<PathBuf>,
    pub(super) watched_files: HashSet<PathBuf>,
    pub(super) events_tx: smol::channel::Sender<Vec<PathBuf>>,
    /// Back/forward history, outlines and Go to Symbol's preview.
    pub(super) nav: nav::EditorNav,
    /// The Problems list at the foot of the panel — see `problems`.
    pub(super) problems: problems::ProblemsPane,
    /// The header's list of every open file, while it is open.
    pub(super) strip_picker: Option<strip::StripPicker>,
    /// The focus of a panel with no file in it. Without one there is nothing
    /// in the panel to hold the focus, so closing the last file handed it to
    /// the terminal and the next ⌘W closed the terminal instead of the panel.
    pub(super) empty_focus: gpui::FocusHandle,
    pub(super) had_focus: Option<TabId>,
    /// The docked document column's share of the terminal column, live.
    pub document_ratio: std::rc::Rc<std::cell::Cell<f32>>,
    pub document_dragging: std::rc::Rc<std::cell::Cell<bool>>,
}

impl EditorPanelState {
    pub fn new(window: &mut Window, cx: &mut Context<CodeEditorWorkspace>) -> Self {
        let (tx, rx) = smol::channel::unbounded::<Vec<PathBuf>>();
        cx.spawn_in(window, async move |app, cx| {
            while let Ok(first) = rx.recv().await {
                cx.background_executor().timer(RELOAD_DEBOUNCE).await;
                let mut changed: HashSet<PathBuf> = first.into_iter().collect();
                while let Ok(more) = rx.try_recv() {
                    changed.extend(more);
                }
                let ok = app.update_in(cx, |app, window, cx| {
                    for path in changed {
                        if app.editor.watched_files.contains(&path) {
                            app.editor_handle_external_change(&path, window, cx);
                        }
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            buffers: Vec::new(),
            next_untitled: 1,
            pending_cursor: None,
            bar: None,
            waiters: Vec::new(),
            unsaved_prompt_open: false,
            restored: HashSet::new(),
            recorded: HashMap::new(),
            watch: None,
            watch_host: None,
            watch_opening: false,
            watch_busy: false,
            watch_dirty: false,
            watched_dirs: HashSet::new(),
            watched_files: HashSet::new(),
            events_tx: tx,
            nav: nav::EditorNav::new(cx),
            problems: Default::default(),
            strip_picker: None,
            empty_focus: cx.focus_handle(),
            had_focus: None,
            document_ratio: std::rc::Rc::new(std::cell::Cell::new(
                cx.global::<anterm_ui_kit::config::Config>().document_ratio,
            )),
            document_dragging: std::rc::Rc::new(std::cell::Cell::new(false)),
        }
    }
}

pub fn language_for_path(path: &Path) -> &'static str {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let lowered = name.to_ascii_lowercase();
        match lowered.as_str() {
            "makefile" | "gnumakefile" => return "make",
            "cmakelists.txt" => return "cmake",
            _ => {}
        }
        if lowered.starts_with('.') && (lowered.contains("shrc") || lowered.ends_with("profile")) {
            return "bash";
        }
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return "text";
    };
    match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "html" | "htm" => "html",
        "css" => "css",
        "md" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "bash",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "lua" => "lua",
        "rb" => "ruby",
        "php" => "php",
        "sql" => "sql",
        "swift" => "swift",
        "scala" => "scala",
        "zig" => "zig",
        "proto" => "proto",
        "diff" | "patch" => "diff",
        "ex" | "exs" => "elixir",
        "erb" => "erb",
        "ejs" => "ejs",
        "svelte" => "svelte",
        "astro" => "astro",
        "graphql" | "gql" => "graphql",
        "cs" => "csharp",
        "cmake" | "mk" => "cmake",
        _ => "text",
    }
}

/// What the status bar calls a language id from [`language_for_path`].
/// `None` for plain text, which the bar leaves unsaid.
pub(super) fn language_label(id: &str) -> Option<&'static str> {
    Some(match id {
        "rust" => "Rust",
        "go" => "Go",
        "python" => "Python",
        "javascript" => "JavaScript",
        "typescript" => "TypeScript",
        "tsx" => "TypeScript JSX",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" => "YAML",
        "html" => "HTML",
        "css" => "CSS",
        "markdown" => "Markdown",
        "bash" => "Shell",
        "c" => "C",
        "cpp" => "C++",
        "java" => "Java",
        "kotlin" => "Kotlin",
        "lua" => "Lua",
        "ruby" => "Ruby",
        "php" => "PHP",
        "sql" => "SQL",
        "swift" => "Swift",
        "scala" => "Scala",
        "zig" => "Zig",
        "proto" => "Protocol Buffers",
        "diff" => "Diff",
        "elixir" => "Elixir",
        "erb" => "ERB",
        "ejs" => "EJS",
        "svelte" => "Svelte",
        "astro" => "Astro",
        "graphql" => "GraphQL",
        "csharp" => "C#",
        "cmake" => "CMake",
        "make" => "Makefile",
        _ => return None,
    })
}

/// How many frames a jump-to-line may wait for the editor to be laid out.
///
/// `InputState::scroll_to` gives up when the buffer has never been painted,
/// and that is exactly the state a file that just opened is in: the cursor
/// lands on the right line and the view stays at the top of the file, which
/// is the one thing a `:120` link exists to avoid. Three frames is the same
/// bounded-retry shape `prefill::select_all_when_drawn` uses against the same
/// class of problem.
pub(super) const CURSOR_SCROLL_ATTEMPTS: u8 = 3;

/// Puts the cursor at `position`, re-trying on later frames until the scroll
/// that follows it can actually be computed.
///
/// Applied immediately as well as on the retry: the cursor itself lands with
/// no layout, so the position is right even for a pane that never paints.
pub(super) fn place_cursor(
    input: Entity<EditorState>,
    position: Position,
    left: u8,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    input.update(cx, |state, cx| {
        state.set_cursor_position(position, window, cx);
    });
    // A target near the top of the file scrolls nowhere and is already done;
    // so is one that has landed. Either way this stops.
    if left <= 1 || input.read(cx).scroll_offset().y != px(0.) {
        return;
    }
    window.on_next_frame(move |window, cx| {
        place_cursor(input, position, left - 1, window, cx);
    });
    // Registering a callback does not by itself ask for a frame, and an
    // overlay that has finished drawing has no other reason to produce one.
    window.refresh();
}

/// Replaces a buffer's text with `new` as one ordinary edit.
///
/// `InputState::set_value` would be simpler, and it clears the undo history —
/// so a file reloaded because an agent or a `git checkout` touched it could no
/// longer be undone past that moment. Replacing only the span that differs is
/// an edit like any other: it goes on the undo stack, and the cursor and
/// scroll stay where they were for everything outside it.
pub(super) fn replace_buffer_text(
    input: &Entity<EditorState>,
    new: &str,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let old = input.read(cx).text().to_string();
    let Some((start, old_end, new_end)) = differing_span(&old, new) else {
        return;
    };
    let start16 = old[..start].encode_utf16().count();
    let end16 = start16 + old[start..old_end].encode_utf16().count();
    let replacement = &new[start..new_end];
    input.update(cx, |state, cx| {
        let cursor = state.cursor_position();
        state.replace_text_in_range(Some(start16..end16), replacement, window, cx);
        state.set_cursor_position(cursor, window, cx);
    });
}

/// The byte span where `old` and `new` differ: `(start, end in old, end in
/// new)`, on character boundaries in both. `None` when they are equal.
pub(super) fn differing_span(old: &str, new: &str) -> Option<(usize, usize, usize)> {
    if old == new {
        return None;
    }
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let max_suffix = old.len().min(new.len()) - start;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some((start, old.len() - suffix, new.len() - suffix))
}

/// Parses what the go-to-line bar was given: `120`, `120:4` or `:120`.
/// One-based, as every compiler and the status bar count.
pub(super) fn parse_line_target(text: &str) -> Option<(u32, u32)> {
    let text = text.trim().trim_start_matches(':');
    let mut parts = text.splitn(2, [':', ',']);
    let line: u32 = parts.next()?.trim().parse().ok()?;
    let column = match parts.next() {
        Some(c) if !c.trim().is_empty() => c.trim().parse().ok()?,
        _ => 1,
    };
    (line > 0).then_some((line, column.max(1)))
}

