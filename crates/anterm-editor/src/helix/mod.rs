//! Shared Helix modal editing state machine, selection-first motions, text
//! objects, surround operations, register store, and `:command` parser.
//!
//! Used by the Code Editor (`ui::code_editor`), Inline Prompt Editor
//! (`terminal::cmd_editor`), Agent Composer (`terminal::composer`), and
//! read-only views (`ui::diff_overlay`, Markdown Preview).

mod motion;
mod ops;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;

/// Framework-agnostic keyboard event consumed by [`HelixState::handle_keystroke`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyInput<'a> {
    pub key: &'a str,
    pub key_char: Option<&'a str>,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

pub use motion::{
    clamp_char_boundary, extend_line_selection, find_char_motion, find_enclosing_pair,
    find_matching_bracket, line_end_offset, line_first_non_blank_offset, line_number_to_offset,
    line_start_offset, move_left, move_right, move_vertical, next_char_offset, surround_pair,
    text_object_range, word_next_end, word_next_start, word_prev_start,
};
pub use ops::{
    add_cursor_below_range, find_next_match, indent_lines, join_lines_in_range, outdent_lines,
    split_selection_on_pattern, toggle_case_str,
};

/// Primary modal state of a Helix-enabled surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelixMode {
    #[default]
    Normal,
    Insert,
    Select,
}

impl HelixMode {
    /// Three-letter badge shown in the status bar and composer pill.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Normal => "NOR",
            Self::Insert => "INS",
            Self::Select => "SEL",
        }
    }

    /// Whether the caret is drawn as a solid block (`Normal` / `Select`)
    /// rather than a vertical bar (`Insert`).
    pub fn is_block_cursor(self) -> bool {
        matches!(self, Self::Normal | Self::Select)
    }
}

/// Byte range `[head..next]` covered by the Helix block cursor in a UTF-8
/// string slice. Clamps `raw_head` to a valid UTF-8 character boundary and
/// advances over at most one non-newline code point.
pub fn block_cursor_byte_range_str(text: &str, raw_head: usize) -> Range<usize> {
    let head = clamp_char_boundary(text, raw_head);
    let next = match text[head..].chars().next() {
        Some(ch) if ch != '\n' && ch != '\r' => head + ch.len_utf8(),
        _ => head,
    };
    head..next
}

/// Pending prefix mode entered from `Normal` or `Select` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinorMode {
    /// `g` — Goto locations, buffers, or LSP symbols.
    Goto,
    /// `m` — Match brackets, text objects (`mi`/`ma`), or surround (`ms`/`md`/`mr`).
    Match,
    /// `mi` — Select inside text object (`w`, `W`, `()`, `[]`, `{}`, `<>`, quotes, `p`).
    MatchInside,
    /// `ma` — Select around text object (`w`, `W`, `()`, `[]`, `{}`, `<>`, quotes, `p`).
    MatchAround,
    /// `ms` — Wrap selection with surrounding character pair.
    SurroundAdd,
    /// `md` — Delete surrounding character pair.
    SurroundDelete,
    /// `mr` — Replace surrounding character pair (`None` awaiting `<from>`, `Some(ch)` awaiting `<to>`).
    SurroundReplace(Option<char>),
    /// `z` — View scroll alignment (`zz`, `zt`, `zb`).
    View,
    /// `Space` — Workspace pickers, LSP actions, and system clipboard shortcuts.
    Space,
    /// `Ctrl-w` / `Space w` — Window / split navigation and management.
    Window,
    /// `"` — Select register for the next yank, delete, change, or paste.
    Register,
    /// `f` / `t` / `F` / `T` — Find or till character on or across lines.
    FindChar { forward: bool, till: bool },
    /// `r` — Replace every character in selection with the next typed character.
    ReplaceChar,
    /// `[` — Previous navigation (`[c` git hunk, `[b` buffer).
    BracketLeft,
    /// `]` — Next navigation (`]c` git hunk, `]b` buffer).
    BracketRight,
}

impl MinorMode {
    /// Header title for the bottom-right Minor Mode Key Hints Popup.
    pub fn title(self) -> &'static str {
        match self {
            Self::Goto => "Goto (g)",
            Self::Match => "Match (m)",
            Self::MatchInside => "Match Inside (mi)",
            Self::MatchAround => "Match Around (ma)",
            Self::SurroundAdd => "Surround Add (ms)",
            Self::SurroundDelete => "Surround Delete (md)",
            Self::SurroundReplace(None) => "Surround Replace — from (mr)",
            Self::SurroundReplace(Some(_)) => "Surround Replace — to (mr)",
            Self::View => "View (z)",
            Self::Space => "Space",
            Self::Window => "Window (Ctrl-w)",
            Self::Register => "Register (\")",
            Self::FindChar {
                forward: true,
                till: false,
            } => "Find Char (f)",
            Self::FindChar {
                forward: true,
                till: true,
            } => "Till Char (t)",
            Self::FindChar {
                forward: false,
                till: false,
            } => "Find Char Backward (F)",
            Self::FindChar {
                forward: false,
                till: true,
            } => "Till Char Backward (T)",
            Self::ReplaceChar => "Replace Char (r)",
            Self::BracketLeft => "Previous ([)",
            Self::BracketRight => "Next (])",
        }
    }

    /// Compact key prefix displayed in the status bar while waiting for the next key.
    pub fn key_prefix(self) -> String {
        match self {
            Self::Goto => "g".to_string(),
            Self::Match => "m".to_string(),
            Self::MatchInside => "mi".to_string(),
            Self::MatchAround => "ma".to_string(),
            Self::SurroundAdd => "ms".to_string(),
            Self::SurroundDelete => "md".to_string(),
            Self::SurroundReplace(None) => "mr".to_string(),
            Self::SurroundReplace(Some(ch)) => format!("mr{ch}"),
            Self::View => "z".to_string(),
            Self::Space => "SPC".to_string(),
            Self::Window => "C-w".to_string(),
            Self::Register => "\"".to_string(),
            Self::FindChar {
                forward: true,
                till: false,
            } => "f".to_string(),
            Self::FindChar {
                forward: true,
                till: true,
            } => "t".to_string(),
            Self::FindChar {
                forward: false,
                till: false,
            } => "F".to_string(),
            Self::FindChar {
                forward: false,
                till: true,
            } => "T".to_string(),
            Self::ReplaceChar => "r".to_string(),
            Self::BracketLeft => "[".to_string(),
            Self::BracketRight => "]".to_string(),
        }
    }

    /// Key-description rows for the floating bottom-right Minor Mode Key Hints Popup.
    pub fn hints(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Goto => &[
                ("g", "Go to file start / line N"),
                ("e", "Go to file end"),
                ("h", "Go to line start"),
                ("l", "Go to line end"),
                ("s", "First non-whitespace char"),
                ("d", "Go to definition (LSP)"),
                ("r", "Find references (LSP)"),
                ("n / p", "Next / previous buffer"),
                ("c", "Toggle line comment"),
            ],
            Self::Match => &[
                ("m", "Go to matching bracket"),
                ("i", "Select inside object…"),
                ("a", "Select around object…"),
                ("s", "Surround selection with pair…"),
                ("d", "Delete surrounding pair…"),
                ("r", "Replace surrounding pair…"),
            ],
            Self::MatchInside | Self::MatchAround => &[
                ("w / W", "Word / WORD"),
                ("( ) / b", "Parentheses"),
                ("[ ]", "Square brackets"),
                ("{ } / B", "Curly braces"),
                ("< >", "Angle brackets"),
                ("\" ' `", "Quotes / backticks"),
                ("p", "Paragraph"),
            ],
            Self::SurroundAdd => &[
                ("( ) [ ] { } < >", "Wrap with bracket pair"),
                ("\" ' ` <char>", "Wrap with delimiter"),
            ],
            Self::SurroundDelete => &[
                ("( ) [ ] { } < >", "Remove enclosing brackets"),
                ("\" ' ` <char>", "Remove enclosing delimiter"),
            ],
            Self::SurroundReplace(None) => &[("<from>", "Delimiter to replace")],
            Self::SurroundReplace(Some(_)) => &[("<to>", "New replacement delimiter")],
            Self::View => &[
                ("z", "Center cursor line in viewport"),
                ("t", "Align cursor line to top"),
                ("b", "Align cursor line to bottom"),
            ],
            Self::Space => &[
                ("f", "Open file picker"),
                ("/", "Global project search"),
                ("s", "Go to symbol in file"),
                ("r", "Rename symbol (LSP)"),
                ("a", "Code actions (LSP)"),
                ("d", "Toggle diagnostics panel"),
                ("y / p", "Yank / paste system clipboard"),
                ("w", "Window / split mode"),
            ],
            Self::Window => &[
                ("w / h / l", "Focus other editor split / pane"),
                ("v / s", "Split editor right"),
                ("q", "Close active buffer / split"),
            ],
            Self::Register => &[
                ("a-z", "Named register (A-Z appends)"),
                ("\"", "Default yank register"),
                ("+ / *", "System clipboard"),
                ("_", "Blackhole register (no yank)"),
            ],
            Self::FindChar { .. } => &[("<char>", "Target character")],
            Self::ReplaceChar => &[("<char>", "Replacement character")],
            Self::BracketLeft => &[("c", "Previous git change"), ("b", "Previous buffer")],
            Self::BracketRight => &[("c", "Next git change"), ("b", "Next buffer")],
        }
    }
}

/// Shared register store across Helix surfaces.
#[derive(Debug, Clone, Default)]
pub struct RegisterStore {
    pub default: String,
    pub named: HashMap<char, String>,
}

#[cfg(not(test))]
fn global_registers() -> &'static std::sync::Mutex<RegisterStore> {
    static GLOBAL: std::sync::OnceLock<std::sync::Mutex<RegisterStore>> =
        std::sync::OnceLock::new();
    GLOBAL.get_or_init(|| std::sync::Mutex::new(RegisterStore::default()))
}

impl RegisterStore {
    /// Read the contents of `reg` (defaulting to `"`), using `system_clipboard`
    /// when `reg` is `'+'`, `'*'`, or `'"'`.
    pub fn read(&self, reg: Option<char>, system_clipboard: Option<&str>) -> String {
        match reg.unwrap_or('"') {
            '_' => String::new(),
            '+' | '*' | '"' => {
                if let Some(clip) = system_clipboard.filter(|s| !s.is_empty()) {
                    return clip.to_owned();
                }
                if !self.default.is_empty() {
                    return self.default.clone();
                }
                #[cfg(not(test))]
                if let Ok(g) = global_registers().lock()
                    && !g.default.is_empty()
                {
                    return g.default.clone();
                }
                String::new()
            }
            c if c.is_ascii_alphabetic() => {
                let key = c.to_ascii_lowercase();
                if let Some(val) = self.named.get(&key) {
                    return val.clone();
                }
                #[cfg(not(test))]
                if let Ok(g) = global_registers().lock()
                    && let Some(val) = g.named.get(&key)
                {
                    return val.clone();
                }
                String::new()
            }
            _ => self.default.clone(),
        }
    }

    /// Write `text` into `reg` (defaulting to `"`).
    ///
    /// Returns `Some(text)` when the caller should also write the string to the
    /// OS clipboard (default `"`, `'+'`, and `'*'` registers).
    pub fn write(&mut self, reg: Option<char>, text: String) -> Option<String> {
        if text.is_empty() {
            return None;
        }
        match reg.unwrap_or('"') {
            '_' => None,
            '+' | '*' | '"' => {
                self.default = text.clone();
                #[cfg(not(test))]
                if let Ok(mut g) = global_registers().lock() {
                    g.default = text.clone();
                }
                Some(text)
            }
            c if c.is_ascii_lowercase() => {
                self.named.insert(c, text.clone());
                #[cfg(not(test))]
                if let Ok(mut g) = global_registers().lock() {
                    g.named.insert(c, text);
                }
                None
            }
            c if c.is_ascii_uppercase() => {
                let key = c.to_ascii_lowercase();
                self.named.entry(key).or_default().push_str(&text);
                #[cfg(not(test))]
                if let Ok(mut g) = global_registers().lock() {
                    g.named.entry(key).or_default().push_str(&text);
                }
                None
            }
            _ => {
                self.default = text.clone();
                #[cfg(not(test))]
                if let Ok(mut g) = global_registers().lock() {
                    g.default = text.clone();
                }
                Some(text)
            }
        }
    }
}

/// Per-surface Helix modal state.
#[derive(Debug, Clone)]
pub struct HelixState {
    pub mode: HelixMode,
    pub count: Option<usize>,
    pub minor: Option<MinorMode>,
    pub selected_register: Option<char>,
    /// Selection anchor in UTF-8 byte offset.
    pub anchor: usize,
    /// Selection head (cursor) in UTF-8 byte offset.
    pub head: usize,
    /// Additional selections `(anchor, head)` for multi-cursor operations (`s`, `C`).
    pub extra_selections: Vec<(usize, usize)>,
    /// Current `/` or `*` search query.
    pub search_query: String,
    pub registers: RegisterStore,
}

impl Default for HelixState {
    fn default() -> Self {
        Self::new(HelixMode::Normal)
    }
}

impl HelixState {
    pub fn new(initial_mode: HelixMode) -> Self {
        Self {
            mode: initial_mode,
            count: None,
            minor: None,
            selected_register: None,
            anchor: 0,
            head: 0,
            extra_selections: Vec::new(),
            search_query: String::new(),
            registers: RegisterStore::default(),
        }
    }

    /// Clear transient minor mode, count, and register selection.
    pub fn clear_pending(&mut self) {
        self.count = None;
        self.minor = None;
        self.selected_register = None;
    }

    /// Reset modal state to `mode` and clear selection / pending prefixes.
    pub fn reset(&mut self, mode: HelixMode) {
        self.mode = mode;
        self.clear_pending();
        self.anchor = 0;
        self.head = 0;
        self.extra_selections.clear();
    }

    /// Set `(anchor, head)` byte offsets directly.
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        self.anchor = anchor;
        self.head = head;
    }

    /// Whether a count, register, or minor mode is waiting for another key.
    pub fn has_pending(&self) -> bool {
        self.count.is_some() || self.minor.is_some() || self.selected_register.is_some()
    }

    /// Pending sequence string for status bar display (e.g. `"3g"`, `"\"a"`, `"mi"`).
    pub fn pending_status(&self) -> String {
        let mut out = String::new();
        if let Some(c) = self.count {
            out.push_str(&c.to_string());
        }
        if let Some(reg) = self.selected_register {
            out.push('"');
            out.push(reg);
        }
        if let Some(minor) = self.minor {
            out.push_str(&minor.key_prefix());
        }
        out
    }

    /// Pending sequence string as `Option<String>` when non-empty.
    pub fn pending_display(&self) -> Option<String> {
        let s = self.pending_status();
        (!s.is_empty()).then_some(s)
    }

    /// Consume the accumulated count, defaulting to 1.
    pub fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1).max(1)
    }

    /// Sync `anchor` and `head` from the underlying editor's byte cursor and selection.
    pub fn sync_from_editor(&mut self, cursor: usize, selected: Range<usize>, text: &str) {
        let len = text.len();
        let start = clamp_char_boundary(text, selected.start.min(len));
        let end = clamp_char_boundary(text, selected.end.min(len));
        let cur = clamp_char_boundary(text, cursor.min(len));
        if start == end {
            self.anchor = cur;
            self.head = cur;
        } else if cur == start {
            self.anchor = end;
            self.head = start;
        } else {
            self.anchor = start;
            self.head = end;
        }
    }

    /// Ordered byte range of `anchor..head` (empty when `anchor == head`).
    pub fn selection_range(&self, text: &str) -> Range<usize> {
        let a = clamp_char_boundary(text, self.anchor);
        let h = clamp_char_boundary(text, self.head);
        a.min(h)..a.max(h)
    }

    /// Effective byte range operated on by Helix actions (`d`, `c`, `y`, `r`, `~`, etc.).
    ///
    /// In Helix, a bare cursor (`anchor == head`) selects the single character
    /// under `head` (or is empty at EOF).
    pub fn effective_range(&self, text: &str) -> Range<usize> {
        let a = clamp_char_boundary(text, self.anchor);
        let h = clamp_char_boundary(text, self.head);
        if a != h {
            a.min(h)..a.max(h)
        } else {
            h..next_char_offset(text, h)
        }
    }

    /// Move `head` to `new_head`, collapsing `anchor` in `Normal` mode or
    /// preserving `anchor` in `Select` mode.
    pub fn move_cursor(&mut self, text: &str, new_head: usize) {
        let pos = clamp_char_boundary(text, new_head);
        self.head = pos;
        if self.mode != HelixMode::Select {
            self.anchor = pos;
        }
    }

    /// Set selection span from `new_anchor` to `new_head` (used by word/find motions
    /// which select the traversed span in `Normal` mode and extend in `Select` mode).
    pub fn select_span(&mut self, text: &str, new_anchor: usize, new_head: usize) {
        let a = clamp_char_boundary(text, new_anchor);
        let h = clamp_char_boundary(text, new_head);
        if self.mode == HelixMode::Select {
            self.head = h;
        } else {
            self.anchor = a;
            self.head = h;
        }
    }
}

/// High-level outcome of processing a keystroke in Helix mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelixOutcome {
    /// Key was not handled by Helix (e.g. in `Insert` mode, let normal input process it).
    Ignored,
    /// Key updated internal Helix state / selection only; sync cursor/selection to view.
    Handled,
    /// Replace `range` (UTF-8 byte offsets) in the buffer with `new_text`, and
    /// place selection at `(new_anchor, new_head)`.
    Edit {
        range: Range<usize>,
        new_text: String,
        new_anchor: usize,
        new_head: usize,
        yank_to_clipboard: Option<String>,
    },
    /// Copy `String` to the OS clipboard (from `y` / `Space y`), without editing text.
    Yank { clipboard: Option<String> },
    /// Execute a host editor / workspace command.
    Host(HelixHostCommand),
}

/// Host-level commands triggered by Helix keys (`g`, `z`, `Space`, `Ctrl-w`, `:`, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelixHostCommand {
    Undo,
    Redo,
    ToggleComment,
    /// `zz` (Center), `zt` (Top), `zb` (Bottom).
    AlignView(ViewAlign),
    /// `Ctrl-d` / `Ctrl-u` / `Ctrl-f` / `Ctrl-b`: scroll + move cursor by `lines` (signed).
    ScrollPages {
        half_pages: isize,
    },
    /// `gd` — Go to definition.
    GoToDefinition,
    /// `gr` — Find references.
    FindReferences,
    /// `gn` / `gp` — Next / previous open buffer.
    CycleBuffer {
        next: bool,
    },
    /// `]c` / `[c` — Next / previous git change hunk.
    StepGitChange {
        next: bool,
    },
    /// `Ctrl-o` / `Ctrl-i` — Jump list back / forward.
    JumpHistory {
        forward: bool,
    },
    /// `Space f` — Quick open file picker.
    QuickOpenFile,
    /// `Space /` — Global project search (or reverse history search in `CmdEditor`).
    GlobalSearch,
    /// `Space s` — Symbol outline picker.
    GoToSymbol,
    /// `Space r` — LSP Rename symbol.
    RenameSymbol,
    /// `Space a` — LSP Code actions.
    CodeActions,
    /// `Space d` — Toggle diagnostics/problems panel.
    ToggleDiagnostics,
    /// `Ctrl-w w` / `Ctrl-w h` / `Ctrl-w l` — Switch split group or focus pane.
    WindowFocusOther,
    /// `Ctrl-w v` / `Ctrl-w s` — Split editor right.
    WindowSplitRight,
    /// `Ctrl-w q` — Close active buffer / split.
    WindowCloseActive,
    /// `:` — Open the Command Bar in the Code Editor.
    OpenCommandBar,
    /// `/` — Open the in-buffer search bar.
    OpenSearchBar,
    /// `s` — Open the split-selection regex prompt bar.
    OpenSplitSelectionBar,
    /// `Enter` in Normal mode of Prompt / Composer: submit the command/message.
    Submit,
    /// `Escape` in Normal mode of Composer / Read-only view when nothing is selected/pending.
    DismissSurface,
}

pub use HelixHostCommand as HostAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewAlign {
    Center,
    Top,
    Bottom,
}

/// Parsed `:command` from the Code Editor's Command Bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandBarAction {
    Empty,
    Write(Option<String>),
    Quit { force: bool },
    WriteQuit { force: bool, path: Option<String> },
    WriteAll,
    QuitAll { force: bool },
    WriteQuitAll,
    Reload,
    Edit(String),
    VSplit(Option<String>),
    GoToLine(usize),
    Unknown(String),
}

/// Parse a `:command` string (with or without a leading `:`).
pub fn parse_command_bar(raw: &str) -> CommandBarAction {
    let trimmed = raw.trim().strip_prefix(':').unwrap_or(raw.trim()).trim();
    if trimmed.is_empty() {
        return CommandBarAction::Empty;
    }
    if let Ok(line) = trimmed.parse::<usize>() {
        return CommandBarAction::GoToLine(line.max(1));
    }
    let (cmd, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((c, r)) => (c, Some(r.trim().to_string()).filter(|s| !s.is_empty())),
        None => (trimmed, None),
    };
    match cmd {
        "w" | "write" => CommandBarAction::Write(rest),
        "q" | "quit" => CommandBarAction::Quit { force: false },
        "q!" | "quit!" => CommandBarAction::Quit { force: true },
        "wq" | "x" => CommandBarAction::WriteQuit {
            force: false,
            path: rest,
        },
        "wq!" | "x!" => CommandBarAction::WriteQuit {
            force: true,
            path: rest,
        },
        "wa" | "wall" => CommandBarAction::WriteAll,
        "qa" | "qall" => CommandBarAction::QuitAll { force: false },
        "qa!" | "qall!" => CommandBarAction::QuitAll { force: true },
        "wqa" | "wqall" | "xa" | "xall" | "wqa!" | "xa!" => CommandBarAction::WriteQuitAll,
        "e" | "edit" => match rest {
            Some(path) => CommandBarAction::Edit(path),
            None => CommandBarAction::Reload,
        },
        "reload" | "e!" => CommandBarAction::Reload,
        "vs" | "vsplit" => CommandBarAction::VSplit(rest),
        other => CommandBarAction::Unknown(other.to_string()),
    }
}

// ---- Keystroke Dispatch Engine ---------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    CodeEditor,
    InlinePrompt,
    Composer,
}

impl HelixState {
    /// Process a key event for `text` on `surface`.
    ///
    /// Returns [`HelixOutcome::Ignored`] when the keystroke should pass through
    /// to standard input handling (e.g. printable typing in `Insert` mode, or
    /// platform shortcuts like `Cmd+C` / `Cmd+V`).
    pub fn handle_keystroke(
        &mut self,
        ks: KeyInput<'_>,
        text: &str,
        surface: SurfaceKind,
        system_clipboard: Option<&str>,
    ) -> HelixOutcome {
        let key = ks.key;
        let m = &ks;

        // Always let platform/secondary shortcuts (`Cmd+...` on macOS, `Ctrl+Shift+...`) pass through.
        if m.platform || (m.control && m.shift) {
            return HelixOutcome::Ignored;
        }

        // In Insert mode, only `Escape` transitions to `Normal` mode.
        if self.mode == HelixMode::Insert {
            if key == "escape" && !m.control && !m.alt && !m.shift {
                self.mode = HelixMode::Normal;
                self.clear_pending();
                // Collapse any selection at current head.
                self.anchor = self.head;
                return HelixOutcome::Handled;
            }
            return HelixOutcome::Ignored;
        }

        // Escape in Normal / Select / Minor mode:
        if key == "escape" {
            if self.has_pending() {
                self.clear_pending();
                return HelixOutcome::Handled;
            }
            if self.mode == HelixMode::Select {
                self.mode = HelixMode::Normal;
                return HelixOutcome::Handled;
            }
            if !self.extra_selections.is_empty() || self.anchor != self.head {
                self.extra_selections.clear();
                self.anchor = self.head;
                return HelixOutcome::Handled;
            }
            return match surface {
                SurfaceKind::Composer => HelixOutcome::Host(HelixHostCommand::DismissSurface),
                SurfaceKind::CodeEditor | SurfaceKind::InlinePrompt => HelixOutcome::Handled,
            };
        }

        // Handle active Minor Mode first.
        if let Some(minor) = self.minor.take() {
            return self.handle_minor_mode(minor, ks, text, surface, system_clipboard);
        }

        // Handle Control chords in Normal / Select mode (`Ctrl-d`, `Ctrl-u`, `Ctrl-f`, `Ctrl-b`, `Ctrl-o`, `Ctrl-i`, `Ctrl-w`, `Ctrl-c`).
        if m.control && !m.alt {
            return match key {
                "d" => {
                    let count = self.take_count() as isize;
                    HelixOutcome::Host(HelixHostCommand::ScrollPages { half_pages: count })
                }
                "u" => {
                    let count = self.take_count() as isize;
                    HelixOutcome::Host(HelixHostCommand::ScrollPages { half_pages: -count })
                }
                "f" => {
                    let count = self.take_count() as isize;
                    HelixOutcome::Host(HelixHostCommand::ScrollPages {
                        half_pages: count * 2,
                    })
                }
                "b" => {
                    let count = self.take_count() as isize;
                    HelixOutcome::Host(HelixHostCommand::ScrollPages {
                        half_pages: -count * 2,
                    })
                }
                "o" => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::JumpHistory { forward: false })
                }
                "i" => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::JumpHistory { forward: true })
                }
                "w" => {
                    self.minor = Some(MinorMode::Window);
                    HelixOutcome::Handled
                }
                "c" => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::ToggleComment)
                }
                _ => HelixOutcome::Ignored,
            };
        }

        let effective_char = resolved_key_char(ks);

        // Handle Alt chords (`Alt-;` flip selection, `Alt-d` / `Alt-c` blackhole delete/change, `Alt-`` uppercase).
        if m.alt && !m.control {
            match effective_char.as_deref().unwrap_or(key) {
                ";" => {
                    std::mem::swap(&mut self.anchor, &mut self.head);
                    return HelixOutcome::Handled;
                }
                "d" => return self.op_blackhole_delete(text),
                "c" => return self.op_blackhole_change(text),
                "`" | "~" => {
                    let range = self.effective_range(text);
                    self.clear_pending();
                    let upper = text[range.clone()].to_uppercase();
                    let new_end = range.start + upper.len();
                    return HelixOutcome::Edit {
                        range: range.clone(),
                        new_text: upper,
                        new_anchor: range.start,
                        new_head: new_end,
                        yank_to_clipboard: None,
                    };
                }
                _ => return HelixOutcome::Ignored,
            }
        }

        // Numeric count prefix (`1`..=`9`, or `0` when count is already accumulating).
        if let Some(s) = effective_char.as_deref()
            && s.len() == 1
            && let Some(ch) = s.chars().next()
        {
            if ch.is_ascii_digit() && (ch != '0' || self.count.is_some()) {
                let digit = (ch as u8 - b'0') as usize;
                let next = self
                    .count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(digit)
                    .min(100_000);
                self.count = Some(next);
                return HelixOutcome::Handled;
            }
        }

        // Named keys (`left`, `right`, `up`, `down`, `home`, `end`, `enter`, `space`, `backspace`).
        match key {
            "left" => {
                let count = self.take_count();
                let next = move_left(text, self.head, count);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "right" => {
                let count = self.take_count();
                let next = move_right(text, self.head, count);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "up" => {
                let count = self.take_count() as isize;
                let next = move_vertical(text, self.head, -count);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "down" => {
                let count = self.take_count() as isize;
                let next = move_vertical(text, self.head, count);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "home" => {
                self.clear_pending();
                let next = line_start_offset(text, self.head);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "end" => {
                self.clear_pending();
                let next = line_end_offset(text, self.head);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "enter" => {
                self.clear_pending();
                if matches!(surface, SurfaceKind::InlinePrompt | SurfaceKind::Composer) {
                    return HelixOutcome::Host(HelixHostCommand::Submit);
                }
                let next = move_vertical(text, self.head, 1);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            "space" => {
                self.minor = Some(MinorMode::Space);
                return HelixOutcome::Handled;
            }
            "backspace" => {
                let count = self.take_count();
                let next = move_left(text, self.head, count);
                self.move_cursor(text, next);
                return HelixOutcome::Handled;
            }
            _ => {}
        }

        let Some(ch_str) = effective_char else {
            return HelixOutcome::Ignored;
        };
        let mut chars = ch_str.chars();
        let Some(ch) = chars.next() else {
            return HelixOutcome::Ignored;
        };
        if chars.next().is_some() {
            return HelixOutcome::Ignored;
        }

        match ch {
            // Basic character & line motions
            'h' => {
                let count = self.take_count();
                let next = move_left(text, self.head, count);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            'l' => {
                let count = self.take_count();
                let next = move_right(text, self.head, count);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            'j' => {
                let count = self.take_count() as isize;
                let next = move_vertical(text, self.head, count);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            'k' => {
                let count = self.take_count() as isize;
                let next = move_vertical(text, self.head, -count);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            '0' => {
                let next = line_start_offset(text, self.head);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            '$' => {
                let next = line_end_offset(text, self.head);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            '^' => {
                let next = line_first_non_blank_offset(text, self.head);
                self.move_cursor(text, next);
                HelixOutcome::Handled
            }
            'G' => {
                let target = match self.count.take() {
                    Some(line) => line_number_to_offset(text, line),
                    None => line_start_offset(text, text.len()),
                };
                self.move_cursor(text, target);
                HelixOutcome::Handled
            }

            // Word motions (select span in Normal mode, extend in Select mode)
            'w' | 'W' => {
                let big = ch == 'W';
                let count = self.take_count();
                let mut start = self.head;
                let mut end = self.head;
                for _ in 0..count {
                    start = end;
                    end = word_next_start(text, end, big);
                }
                self.select_span(text, start, end);
                HelixOutcome::Handled
            }
            'e' | 'E' => {
                let big = ch == 'E';
                let count = self.take_count();
                let mut start = self.head;
                let mut end = self.head;
                for _ in 0..count {
                    start = end;
                    end = word_next_end(text, end, big);
                }
                self.select_span(text, start, end);
                HelixOutcome::Handled
            }
            'b' | 'B' => {
                let big = ch == 'B';
                let count = self.take_count();
                let mut start = self.head;
                let mut end = self.head;
                for _ in 0..count {
                    start = end;
                    end = word_prev_start(text, end, big);
                }
                self.select_span(text, start, end);
                HelixOutcome::Handled
            }

            // Find / Till char minor modes
            'f' => {
                self.minor = Some(MinorMode::FindChar {
                    forward: true,
                    till: false,
                });
                HelixOutcome::Handled
            }
            't' => {
                self.minor = Some(MinorMode::FindChar {
                    forward: true,
                    till: true,
                });
                HelixOutcome::Handled
            }
            'F' => {
                self.minor = Some(MinorMode::FindChar {
                    forward: false,
                    till: false,
                });
                HelixOutcome::Handled
            }
            'T' => {
                self.minor = Some(MinorMode::FindChar {
                    forward: false,
                    till: true,
                });
                HelixOutcome::Handled
            }

            // Selection manipulation (`v`, `x`, `%`, `;`, `,`, `C`, `s`)
            'v' => {
                self.clear_pending();
                self.mode = match self.mode {
                    HelixMode::Select => HelixMode::Normal,
                    _ => HelixMode::Select,
                };
                HelixOutcome::Handled
            }
            'x' => {
                let count = self.take_count();
                let (new_a, new_h) = extend_line_selection(text, self.anchor, self.head, count);
                self.anchor = new_a;
                self.head = new_h;
                HelixOutcome::Handled
            }
            '%' => {
                self.clear_pending();
                self.anchor = 0;
                self.head = text.len();
                HelixOutcome::Handled
            }
            ';' => {
                self.clear_pending();
                self.anchor = self.head;
                HelixOutcome::Handled
            }
            ',' => {
                self.clear_pending();
                self.extra_selections.clear();
                HelixOutcome::Handled
            }
            'C' => {
                let count = self.take_count();
                let mut last = self
                    .extra_selections
                    .last()
                    .map(|&(_, h)| h)
                    .unwrap_or(self.head);
                for _ in 0..count {
                    if let Some(next_pos) = add_cursor_below_range(text, last) {
                        self.extra_selections.push((next_pos, next_pos));
                        last = next_pos;
                    }
                }
                HelixOutcome::Handled
            }
            's' => {
                self.clear_pending();
                HelixOutcome::Host(HelixHostCommand::OpenSplitSelectionBar)
            }

            // Mode transitions (`i`, `a`, `I`, `A`, `o`, `O`)
            'i' => {
                self.clear_pending();
                let pos = self.anchor.min(self.head);
                self.anchor = pos;
                self.head = pos;
                self.mode = HelixMode::Insert;
                HelixOutcome::Handled
            }
            'a' => {
                self.clear_pending();
                let pos = if self.anchor == self.head {
                    let line_end = line_end_offset(text, self.head);
                    next_char_offset(text, self.head).min(line_end)
                } else {
                    self.anchor.max(self.head)
                };
                self.anchor = pos;
                self.head = pos;
                self.mode = HelixMode::Insert;
                HelixOutcome::Handled
            }
            'I' => {
                self.clear_pending();
                let pos = line_first_non_blank_offset(text, self.head);
                self.anchor = pos;
                self.head = pos;
                self.mode = HelixMode::Insert;
                HelixOutcome::Handled
            }
            'A' => {
                self.clear_pending();
                let pos = line_end_offset(text, self.head);
                self.anchor = pos;
                self.head = pos;
                self.mode = HelixMode::Insert;
                HelixOutcome::Handled
            }
            'o' => {
                self.clear_pending();
                let line_start = line_start_offset(text, self.head);
                let non_blank = line_first_non_blank_offset(text, self.head);
                let indent = &text[line_start..non_blank];
                let insert_at = line_end_offset(text, self.head);
                let new_text = format!("\n{indent}");
                let new_pos = insert_at + new_text.len();
                self.mode = HelixMode::Insert;
                HelixOutcome::Edit {
                    range: insert_at..insert_at,
                    new_text,
                    new_anchor: new_pos,
                    new_head: new_pos,
                    yank_to_clipboard: None,
                }
            }
            'O' => {
                self.clear_pending();
                let line_start = line_start_offset(text, self.head);
                let non_blank = line_first_non_blank_offset(text, self.head);
                let indent = &text[line_start..non_blank];
                let new_text = format!("{indent}\n");
                let new_pos = line_start + indent.len();
                self.mode = HelixMode::Insert;
                HelixOutcome::Edit {
                    range: line_start..line_start,
                    new_text,
                    new_anchor: new_pos,
                    new_head: new_pos,
                    yank_to_clipboard: None,
                }
            }

            // Delete, Change, Yank, Paste, Replace
            'd' => self.op_delete(text),
            'c' => self.op_change(text),
            'y' => self.op_yank(text),
            'p' | 'P' => self.op_paste(text, ch == 'p', system_clipboard),
            'r' => {
                self.minor = Some(MinorMode::ReplaceChar);
                HelixOutcome::Handled
            }

            // Undo / Redo (`u`, `U`)
            'u' => {
                self.clear_pending();
                HelixOutcome::Host(HelixHostCommand::Undo)
            }
            'U' => {
                self.clear_pending();
                HelixOutcome::Host(HelixHostCommand::Redo)
            }

            // Indent, Outdent, Join, Case transforms (`>`, `<`, `J`, `~`, `` ` ``)
            '>' => {
                let count = self.take_count();
                let eff = self.effective_range(text);
                let (range, new_text) = indent_lines(text, eff, count, "    ");
                let new_end = range.start + new_text.len();
                HelixOutcome::Edit {
                    new_anchor: range.start,
                    new_head: new_end,
                    range,
                    new_text,
                    yank_to_clipboard: None,
                }
            }
            '<' => {
                let count = self.take_count();
                let eff = self.effective_range(text);
                let (range, new_text) = outdent_lines(text, eff, count, 4);
                let new_end = range.start + new_text.len();
                HelixOutcome::Edit {
                    new_anchor: range.start,
                    new_head: new_end,
                    range,
                    new_text,
                    yank_to_clipboard: None,
                }
            }
            'J' => {
                self.clear_pending();
                let eff = self.effective_range(text);
                if let Some((range, new_text)) = join_lines_in_range(text, eff) {
                    let new_end = range.start + new_text.len();
                    HelixOutcome::Edit {
                        new_anchor: range.start,
                        new_head: new_end,
                        range,
                        new_text,
                        yank_to_clipboard: None,
                    }
                } else {
                    HelixOutcome::Handled
                }
            }
            '~' => {
                self.clear_pending();
                let range = self.effective_range(text);
                if range.is_empty() {
                    return HelixOutcome::Handled;
                }
                let toggled = toggle_case_str(&text[range.clone()]);
                let new_end = range.start + toggled.len();
                HelixOutcome::Edit {
                    new_anchor: range.start,
                    new_head: new_end,
                    range,
                    new_text: toggled,
                    yank_to_clipboard: None,
                }
            }
            '`' => {
                self.clear_pending();
                let range = self.effective_range(text);
                if range.is_empty() {
                    return HelixOutcome::Handled;
                }
                let lower = text[range.clone()].to_lowercase();
                let new_end = range.start + lower.len();
                HelixOutcome::Edit {
                    new_anchor: range.start,
                    new_head: new_end,
                    range,
                    new_text: lower,
                    yank_to_clipboard: None,
                }
            }

            // Minor mode prefixes (`g`, `m`, `z`, `"`, `:`, `/`, `*`, `n`, `N`)
            'g' => {
                self.minor = Some(MinorMode::Goto);
                HelixOutcome::Handled
            }
            'm' => {
                self.minor = Some(MinorMode::Match);
                HelixOutcome::Handled
            }
            'z' => {
                self.minor = Some(MinorMode::View);
                HelixOutcome::Handled
            }
            '[' => {
                self.minor = Some(MinorMode::BracketLeft);
                HelixOutcome::Handled
            }
            ']' => {
                self.minor = Some(MinorMode::BracketRight);
                HelixOutcome::Handled
            }
            '"' => {
                self.minor = Some(MinorMode::Register);
                HelixOutcome::Handled
            }
            ':' => {
                self.clear_pending();
                if surface == SurfaceKind::CodeEditor {
                    HelixOutcome::Host(HelixHostCommand::OpenCommandBar)
                } else {
                    HelixOutcome::Handled
                }
            }
            '/' => {
                self.clear_pending();
                HelixOutcome::Host(HelixHostCommand::OpenSearchBar)
            }
            '*' => {
                self.clear_pending();
                let range = if self.anchor != self.head {
                    self.selection_range(text)
                } else {
                    text_object_range(text, self.head, 'w', false)
                        .unwrap_or_else(|| self.effective_range(text))
                };
                if !range.is_empty() {
                    self.search_query = text[range.clone()].to_string();
                    self.anchor = range.start;
                    self.head = range.end;
                }
                HelixOutcome::Handled
            }
            'n' | 'N' => {
                let forward = ch == 'n';
                let count = self.take_count();
                if self.search_query.is_empty() {
                    return HelixOutcome::Handled;
                }
                let mut cur = if forward {
                    self.anchor.max(self.head)
                } else {
                    self.anchor.min(self.head)
                };
                let mut matched = None;
                for _ in 0..count {
                    if let Some(r) = find_next_match(text, &self.search_query, cur, forward) {
                        cur = if forward { r.end } else { r.start };
                        matched = Some(r);
                    } else {
                        break;
                    }
                }
                if let Some(r) = matched {
                    self.select_span(text, r.start, r.end);
                }
                HelixOutcome::Handled
            }

            _ => HelixOutcome::Handled,
        }
    }

    fn handle_minor_mode(
        &mut self,
        minor: MinorMode,
        ks: KeyInput<'_>,
        text: &str,
        _surface: SurfaceKind,
        system_clipboard: Option<&str>,
    ) -> HelixOutcome {
        let Some(ch_str) = resolved_key_char(ks) else {
            self.clear_pending();
            return HelixOutcome::Handled;
        };
        let mut chars = ch_str.chars();
        let Some(ch) = chars.next() else {
            self.clear_pending();
            return HelixOutcome::Handled;
        };

        match minor {
            MinorMode::Goto => match ch {
                'g' => {
                    let target = match self.count.take() {
                        Some(line) => line_number_to_offset(text, line),
                        None => 0,
                    };
                    self.move_cursor(text, target);
                    HelixOutcome::Handled
                }
                'e' => {
                    self.clear_pending();
                    self.move_cursor(text, text.len());
                    HelixOutcome::Handled
                }
                'h' => {
                    self.clear_pending();
                    let next = line_start_offset(text, self.head);
                    self.move_cursor(text, next);
                    HelixOutcome::Handled
                }
                'l' => {
                    self.clear_pending();
                    let next = line_end_offset(text, self.head);
                    self.move_cursor(text, next);
                    HelixOutcome::Handled
                }
                's' => {
                    self.clear_pending();
                    let next = line_first_non_blank_offset(text, self.head);
                    self.move_cursor(text, next);
                    HelixOutcome::Handled
                }
                'd' => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::GoToDefinition)
                }
                'r' => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::FindReferences)
                }
                'n' => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::CycleBuffer { next: true })
                }
                'p' => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::CycleBuffer { next: false })
                }
                'c' => {
                    self.clear_pending();
                    HelixOutcome::Host(HelixHostCommand::ToggleComment)
                }
                _ => {
                    self.clear_pending();
                    HelixOutcome::Handled
                }
            },
            MinorMode::Match => match ch {
                'm' => {
                    self.clear_pending();
                    if let Some(target) = find_matching_bracket(text, self.head) {
                        self.move_cursor(text, target);
                    }
                    HelixOutcome::Handled
                }
                'i' => {
                    self.minor = Some(MinorMode::MatchInside);
                    HelixOutcome::Handled
                }
                'a' => {
                    self.minor = Some(MinorMode::MatchAround);
                    HelixOutcome::Handled
                }
                's' => {
                    self.minor = Some(MinorMode::SurroundAdd);
                    HelixOutcome::Handled
                }
                'd' => {
                    self.minor = Some(MinorMode::SurroundDelete);
                    HelixOutcome::Handled
                }
                'r' => {
                    self.minor = Some(MinorMode::SurroundReplace(None));
                    HelixOutcome::Handled
                }
                _ => {
                    self.clear_pending();
                    HelixOutcome::Handled
                }
            },
            MinorMode::MatchInside | MinorMode::MatchAround => {
                let around = minor == MinorMode::MatchAround;
                self.clear_pending();
                if let Some(range) = text_object_range(text, self.head, ch, around) {
                    self.anchor = range.start;
                    self.head = range.end;
                }
                HelixOutcome::Handled
            }
            MinorMode::SurroundAdd => {
                self.clear_pending();
                let range = self.effective_range(text);
                let (open, close) = surround_pair(ch);
                let wrapped = format!("{open}{}{close}", &text[range.clone()]);
                let new_end = range.start + wrapped.len();
                HelixOutcome::Edit {
                    new_anchor: range.start,
                    new_head: new_end,
                    range,
                    new_text: wrapped,
                    yank_to_clipboard: None,
                }
            }
            MinorMode::SurroundDelete => {
                self.clear_pending();
                let (open, close) = surround_pair(ch);
                if let Some((outer, inner)) = find_enclosing_pair(text, self.head, open, close) {
                    let inner_text = text[inner].to_string();
                    let new_end = outer.start + inner_text.len();
                    HelixOutcome::Edit {
                        new_anchor: outer.start,
                        new_head: new_end,
                        range: outer,
                        new_text: inner_text,
                        yank_to_clipboard: None,
                    }
                } else {
                    HelixOutcome::Handled
                }
            }
            MinorMode::SurroundReplace(None) => {
                self.minor = Some(MinorMode::SurroundReplace(Some(ch)));
                HelixOutcome::Handled
            }
            MinorMode::SurroundReplace(Some(from_ch)) => {
                self.clear_pending();
                let (open_from, close_from) = surround_pair(from_ch);
                let (open_to, close_to) = surround_pair(ch);
                if let Some((outer, inner)) =
                    find_enclosing_pair(text, self.head, open_from, close_from)
                {
                    let replaced = format!("{open_to}{}{close_to}", &text[inner]);
                    let new_end = outer.start + replaced.len();
                    HelixOutcome::Edit {
                        new_anchor: outer.start,
                        new_head: new_end,
                        range: outer,
                        new_text: replaced,
                        yank_to_clipboard: None,
                    }
                } else {
                    HelixOutcome::Handled
                }
            }
            MinorMode::View => {
                self.clear_pending();
                match ch {
                    'z' | 'c' => HelixOutcome::Host(HelixHostCommand::AlignView(ViewAlign::Center)),
                    't' => HelixOutcome::Host(HelixHostCommand::AlignView(ViewAlign::Top)),
                    'b' => HelixOutcome::Host(HelixHostCommand::AlignView(ViewAlign::Bottom)),
                    _ => HelixOutcome::Handled,
                }
            }
            MinorMode::Space => {
                self.clear_pending();
                match ch {
                    'f' => HelixOutcome::Host(HelixHostCommand::QuickOpenFile),
                    '/' => HelixOutcome::Host(HelixHostCommand::GlobalSearch),
                    's' => HelixOutcome::Host(HelixHostCommand::GoToSymbol),
                    'r' => HelixOutcome::Host(HelixHostCommand::RenameSymbol),
                    'a' => HelixOutcome::Host(HelixHostCommand::CodeActions),
                    'd' => HelixOutcome::Host(HelixHostCommand::ToggleDiagnostics),
                    'w' => {
                        self.minor = Some(MinorMode::Window);
                        HelixOutcome::Handled
                    }
                    'y' => self.op_yank_clipboard(text),
                    'p' => self.op_paste_clipboard(text, system_clipboard),
                    _ => HelixOutcome::Handled,
                }
            }
            MinorMode::Window => {
                self.clear_pending();
                match ch {
                    'w' | 'h' | 'l' | 'j' | 'k' => {
                        HelixOutcome::Host(HelixHostCommand::WindowFocusOther)
                    }
                    'v' | 's' => HelixOutcome::Host(HelixHostCommand::WindowSplitRight),
                    'q' => HelixOutcome::Host(HelixHostCommand::WindowCloseActive),
                    _ => HelixOutcome::Handled,
                }
            }
            MinorMode::Register => {
                self.selected_register = Some(ch);
                HelixOutcome::Handled
            }
            MinorMode::FindChar { forward, till } => {
                let count = self.take_count();
                if let Some(target) = find_char_motion(text, self.head, ch, forward, till, count) {
                    self.select_span(text, self.head, target);
                }
                HelixOutcome::Handled
            }
            MinorMode::ReplaceChar => {
                self.clear_pending();
                let range = self.effective_range(text);
                if range.is_empty() {
                    return HelixOutcome::Handled;
                }
                let replaced: String = text[range.clone()]
                    .chars()
                    .map(|c| if c == '\n' || c == '\r' { c } else { ch })
                    .collect();
                let new_end = range.start + replaced.len();
                HelixOutcome::Edit {
                    new_anchor: range.start,
                    new_head: new_end,
                    range,
                    new_text: replaced,
                    yank_to_clipboard: None,
                }
            }
            MinorMode::BracketLeft | MinorMode::BracketRight => {
                let next = minor == MinorMode::BracketRight;
                self.clear_pending();
                match ch {
                    'c' | 'g' => HelixOutcome::Host(HelixHostCommand::StepGitChange { next }),
                    'b' => HelixOutcome::Host(HelixHostCommand::CycleBuffer { next }),
                    _ => HelixOutcome::Handled,
                }
            }
        }
    }
}

/// Extract the effective printable character string from a [`KeyInput`],
/// respecting `key_char` or `shift` on ASCII letters.
pub fn resolved_key_char(ks: KeyInput<'_>) -> Option<String> {
    if let Some(kc) = ks.key_char
        && !kc.is_empty()
        && !kc.chars().any(char::is_control)
    {
        return Some(kc.to_string());
    }
    if ks.key == "space" {
        return Some(" ".to_string());
    }
    if ks.key.len() == 1
        && let Some(ch) = ks.key.chars().next()
    {
        if ks.shift && ch.is_ascii_lowercase() {
            return Some(ch.to_ascii_uppercase().to_string());
        }
        return Some(ch.to_string());
    }
    None
}
