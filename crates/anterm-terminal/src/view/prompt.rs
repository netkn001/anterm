use super::*;

use gpui::Modifiers;

use anterm_core::shell_quote::quote_for_shell;
use crate::backend::hold::Verdict;
use crate::backend::typeahead::RawInput;
use crate::prompt::completion::{self, CandidateKind};
use crate::prompt::history::{HistoryFilter, RecordInput, Scope, ScopeSnapshot};
use crate::prompt::reverse_search;
use crate::prompt::store::{HistoryStore, HistoryWorker};

pub(super) struct PendingHistory {
    pub(super) line: String,
    pub(super) seq: u64,
    pub(super) req_id: u64,
    pub(super) started_at: std::time::Instant,
    pub(super) worker: HistoryWorker,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum HistoryNavPos {
    Session(usize),
    Host(usize),
}

#[derive(Clone, Debug)]
pub(super) struct HistoryNav {
    pub(super) pos: HistoryNavPos,
    pub(super) line: String,
    pub(super) snapshot_gen: u64,
}

pub(super) struct GhostCache {
    line: String,
    scope: Scope,
    snapshot_gen: u64,
    cwd: Option<std::path::PathBuf>,
    per_pane: bool,
    session_id: String,
    result: Option<String>,
}

pub(super) struct LastWordWalk {
    pub(super) entry: usize,
    pub(super) at: usize,
    pub(super) word: String,
}

pub(super) const HOLD_WINDOW: std::time::Duration = std::time::Duration::from_millis(150);

const MAX_HISTORY_BYTES: u64 = 4 << 20;

/// Where a remote scope's list is seeded from, besides what anterm itself
/// recorded for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum HistorySeedSource {
    /// Nothing to seed from: anterm's own record is all there is.
    None,
    /// The far end's shell history files, over the pane's own SSH connection.
    PaneSftp,
}

pub(super) fn history_seed_source(
    scope_is_local: bool,
    remote: Option<anterm_core::daemon::protocol::RemoteKind>,
) -> HistorySeedSource {
    use anterm_core::daemon::protocol::RemoteKind;
    if scope_is_local {
        return HistorySeedSource::None;
    }
    match remote {
        Some(RemoteKind::NativeSsh) => HistorySeedSource::PaneSftp,
        _ => HistorySeedSource::None,
    }
}

/// What a history load reads besides anterm's own record of the scope.
enum HistorySeedFetch {
    None,
    Sftp {
        route: anterm_ui_kit::daemon_rpc::SftpRoute,
        shell_hint: Option<String>,
    },
}

impl HistorySeedFetch {
    fn read(self) -> Vec<(String, Vec<u8>)> {
        match self {
            HistorySeedFetch::None => Vec::new(),
            HistorySeedFetch::Sftp { route, shell_hint } => {
                read_shell_history_over_sftp(&route, shell_hint.as_deref())
            }
        }
    }
}

fn read_shell_history_over_sftp(
    route: &anterm_ui_kit::daemon_rpc::SftpRoute,
    shell_hint: Option<&str>,
) -> Vec<(String, Vec<u8>)> {
    use anterm_core::daemon::protocol::{SftpOp, SftpOpResult};
    let home = match route.op(SftpOp::Realpath {
        path: ".".to_string(),
    }) {
        SftpOpResult::Link(home) if home.starts_with('/') => home,
        other => {
            log::debug!(target: "anterm::history", "no remote home to seed history from: {other:?}");
            return Vec::new();
        }
    };

    let fetch_one = |rel: &str| -> Option<(String, Vec<u8>)> {
        let path = anterm_core::daemon::ssh::sftp::remote_join(&home, rel);
        match route.op(SftpOp::ReadFile {
            path,
            max_bytes: MAX_HISTORY_BYTES,
        }) {
            SftpOpResult::File { bytes, .. } => {
                Some((anterm_core::daemon::ssh::sftp::remote_basename(rel), bytes))
            }
            _ => None,
        }
    };

    // Fast path: when we know the remote shell, try its history file first to
    // avoid 3 unnecessary SFTP round trips.
    if let Some(primary) =
        shell_hint.and_then(crate::prompt::history::shell_history_name_for_shell)
        && let Some(hit) = fetch_one(primary)
    {
        return vec![hit];
    }

    crate::prompt::history::shell_history_names()
        .into_iter()
        .filter_map(fetch_one)
        .collect()
}

pub(super) fn typeahead_boundary(key: &str, modifiers: &Modifiers) -> Option<RawInput<'static>> {
    if !modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    match key {
        "c" => Some(RawInput::Interrupt),
        "d" => Some(RawInput::EndOfInput),
        _ => None,
    }
}

fn sync_typeahead_owner_state(
    typeahead: &mut Typeahead,
    last_blocked: &mut bool,
    blocked: bool,
) -> bool {
    // Alternate-screen TUIs and known agents own their input. Never replay a
    // record across an ownership boundary as shell input.
    let changed = blocked != *last_blocked;
    if changed {
        typeahead.discard();
        *last_blocked = blocked;
    }
    changed
}

pub(super) fn observe_typeahead_for_owner(
    typeahead: &mut Typeahead,
    last_blocked: &mut bool,
    input: RawInput<'_>,
    blocked: bool,
) {
    if !sync_typeahead_owner_state(typeahead, last_blocked, blocked) {
        typeahead.observe(input, *last_blocked);
    }
}

fn word_start_of(line: &str, cursor: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut start = cursor.min(chars.len());
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    start
}

impl TerminalView {
    fn desired_history_scope(&self) -> crate::prompt::history::Scope {
        if let Some(ctx) = self.remote_context() {
            return crate::prompt::history::Scope::remote(&ctx.target);
        }
        if !self.session.host_id.is_local() {
            return crate::prompt::history::Scope::remote(&format!("host-{:016x}", self.session.host_id.0));
        }
        crate::prompt::history::Scope::Local
    }

    pub(super) fn invalidate_ghost_cache(&self) {
        *self.prompt.ghost_cache.borrow_mut() = None;
    }

    pub(super) fn follow_history_scope(&mut self, cx: &mut Context<Self>) {
        let scope = self.desired_history_scope();
        if scope == self.prompt.history_scope {
            self.seed_history_when_connected(cx);
            return;
        }
        self.flush_pending_history(cx);
        self.prompt.history_scope = scope.clone();
        self.prompt.history_nav = None;
        self.prompt.reverse_search = None;
        self.prompt.history_frecency_key = (u64::MAX, None, false);
        self.invalidate_ghost_cache();
        cx.notify();

        self.prompt.history_seed_pending = false;
        let fetch = match self.history_seed_fetch(cx) {
            HistorySeedFetch::Sftp { .. } if !self.ssh_connected() => {
                self.prompt.history_seed_pending = true;
                HistorySeedFetch::None
            }
            fetch => fetch,
        };
        self.spawn_history_load(scope, fetch, cx);
    }

    /// The far-end seed a native SSH pane deferred until its connection was
    /// up, fetched the first time it is.
    fn seed_history_when_connected(&mut self, cx: &mut Context<Self>) {
        if !self.prompt.history_seed_pending || !self.ssh_connected() {
            return;
        }
        self.prompt.history_seed_pending = false;
        let fetch = self.history_seed_fetch(cx);
        if matches!(fetch, HistorySeedFetch::None) {
            return;
        }
        self.spawn_history_load(self.prompt.history_scope.clone(), fetch, cx);
    }

    /// Load `scope`'s list off the UI thread via [`HistoryStore`] and notify
    /// this view once it lands.
    fn spawn_history_load(
        &mut self,
        scope: Scope,
        fetch: HistorySeedFetch,
        cx: &mut Context<Self>,
    ) {
        let has_sftp = matches!(fetch, HistorySeedFetch::Sftp { .. });
        let worker = {
            let store = cx.default_global::<HistoryStore>();
            if has_sftp {
                if !store.should_seed_remote(&scope) {
                    store.begin_scope_load(&scope, false)
                } else {
                    store.mark_remote_seeded(&scope);
                    store.begin_scope_load(&scope, true)
                }
            } else {
                store.begin_scope_load(&scope, false)
            }
        };
        let Some(worker) = worker else {
            return;
        };

        self.prompt.history_load_gen = self.prompt.history_load_gen.wrapping_add(1);
        let generation = self.prompt.history_load_gen;
        let loading = scope.clone();
        cx.spawn(async move |this, cx| {
            let shell_files = if has_sftp {
                cx.background_spawn(async move { fetch.read() }).await
            } else {
                Vec::new()
            };
            let rx = worker.request_scope_load(loading.clone(), shell_files);
            let Ok(loaded) = rx.recv().await else {
                return;
            };
            this.update(cx, |view, cx| {
                cx.default_global::<HistoryStore>()
                    .install_loaded_scope(&loading, loaded);
                if view.prompt.history_scope != scope || view.prompt.history_load_gen != generation {
                    return;
                }
                view.remap_active_history_nav(cx);
                view.invalidate_ghost_cache();
                if view.prompt.reverse_search.is_some() {
                    view.refresh_reverse_search(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Install a finished load into [`HistoryStore`] for the current scope,
    /// keeping anything run in-flight and preserving active navigation.
    #[cfg(test)]
    pub(super) fn install_loaded_history(&mut self, loaded: ScopeSnapshot, cx: &mut Context<Self>) {
        cx.default_global::<HistoryStore>()
            .install_loaded_scope(&self.prompt.history_scope, loaded);
        self.remap_active_history_nav(cx);
        self.invalidate_ghost_cache();
        if self.prompt.reverse_search.is_some() {
            self.refresh_reverse_search(cx);
        }
    }

    fn history_seed_fetch(&self, _cx: &mut Context<Self>) -> HistorySeedFetch {
        match history_seed_source(
            self.prompt.history_scope.is_local(),
            self.remote_context().map(|r| r.kind),
        ) {
            HistorySeedSource::None => HistorySeedFetch::None,
            HistorySeedSource::PaneSftp => HistorySeedFetch::Sftp {
                route: anterm_ui_kit::daemon_rpc::SftpRoute::new(self.pane_id),
                shell_hint: self.shell_program(),
            },
        }
    }

    pub(super) fn at_shell_prompt(&self) -> bool {
        self.terminal.at_prompt()
    }

    pub(super) fn cursor_cell(&self) -> Option<(usize, usize)> {
        let term = self.terminal.term.lock();
        let content = term.renderable_content();
        let row = content.cursor.point.line.0 + content.display_offset as i32;
        let col = content.cursor.point.column.0;
        (row >= 0).then_some((row as usize, col))
    }

    /// Where the IME should compose when the input bar has moved below the
    /// prompt (see [`input_start`]): the start of the bar's first row. `None`
    /// when the bar is beside the prompt, or not up at all — the prompt's own
    /// cursor cell is right then. Takes the cursor as the frame snapshot saw
    /// it rather than locking the terminal again mid-paint.
    pub(crate) fn input_ime_cell(
        &self,
        row: usize,
        col: usize,
        cols: usize,
    ) -> Option<(usize, usize)> {
        if !self.input_active() || self.prompt.reverse_search.is_some() {
            return None;
        }
        match input_start(col, cols.max(1)) {
            (0, _) => None,
            (rows, col) => Some((row + rows, col)),
        }
    }

    pub(crate) fn input_scroll_rows(&self) -> usize {
        if !self.input_active() || self.prompt.reverse_search.is_some() {
            return 0;
        }
        let Some((crow, ccol)) = self.cursor_cell() else {
            return 0;
        };
        let (rows, cols, offset) = {
            let term = self.terminal.term.lock();
            (
                term.screen_lines(),
                term.columns(),
                term.grid().display_offset(),
            )
        };
        if offset != 0 {
            return 0;
        }
        let chars: Vec<char> = self.prompt.cmd.text().chars().collect();
        let (visual_rows, caret_vrow) = input_overlay_rows(
            &chars,
            self.prompt.cmd.cursor(),
            &self.session.marked_text,
            input_start(ccol, cols.max(1)),
            cols.max(1),
        );
        input_overflow_shift(crow, caret_vrow, visual_rows, rows)
    }

    pub fn input_active(&self) -> bool {
        self.input_inactive_reason().is_none()
    }

    /// Moves this pane between anterm's inline editor and the shell's own line
    /// editor while it is live.
    ///
    /// A line the user already typed is not dropped on the floor: it is handed
    /// to the shell the same way an unknown chord hands one over, so the text
    /// is still on the prompt to finish under ZLE/readline. A multi-line draft
    /// has nowhere to go on a shell prompt, so it stays in the editor and comes
    /// back if the setting is turned on again.
    pub fn set_prompt_editor(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.prompt.prompt_editor == on {
            return;
        }
        if !on && self.input_active() && !self.prompt.cmd.text().is_empty() {
            self.handoff_line_to_shell(&[], cx);
        }
        self.prompt.prompt_editor = on;
        self.close_completion();
        self.prompt.reverse_search = None;
        cx.notify();
    }

    pub fn set_helix_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.prompt.helix_mode == on {
            return;
        }
        self.prompt.helix_mode = on;
        self.prompt.cmd.helix = anterm_editor::helix::HelixState::new(anterm_editor::helix::HelixMode::Insert);
        self.prompt.cmd.sync_to_helix();
        if let Some(c) = self.composer_state.active.as_mut() {
            c.helix = anterm_editor::helix::HelixState::new(anterm_editor::helix::HelixMode::Insert);
        }
        cx.notify();
    }

    fn input_inactive_reason(&self) -> Option<&'static str> {
        // The one gate for `prompt_editor: false`. Every path that could take a
        // prompt away from the shell — keys, IME commits, paste, Tab, the
        // completion and reverse-search menus, the input bar itself — asks this
        // first, so answering here is what makes the mode whole instead of a
        // special case per key.
        if !self.prompt.prompt_editor {
            return Some("the inline prompt editor is turned off");
        }
        if self.terminal.exited {
            return Some("the shell has exited");
        }
        if self.search_state.focused {
            return Some("the search field holds the keyboard");
        }
        // Check shell-prompt ownership before reading terminal mode: while a
        // coding agent or command is running, `at_shell_prompt()` is false and
        // short-circuits without touching `Term` or its mode mirror.
        if !self.at_shell_prompt() {
            return Some("the shell has not reported a prompt (no OSC 133)");
        }
        if self.on_alt_screen() {
            return Some("the pane is on the alternate screen");
        }
        if self.handoff_active() {
            return Some("this prompt's line was already handed to the shell");
        }
        None
    }

    fn link_inactive_reason(&self, cx: &gpui::App) -> Option<&'static str> {
        (!self.accepts_input(cx)).then_some("the remote link is not attached")
    }

    pub(super) fn handoff_active(&self) -> bool {
        self.prompt.editor_handoff == Some(self.terminal.prompt_cycle())
            && self
                .prompt
                .editor_handoff_interrupt_seq
                .is_none_or(|seq| self.terminal.prompt_seq() <= seq)
            && self.terminal.at_prompt()
            && !self.on_alt_screen()
    }

    pub(super) fn shell_owns_prompt(&self) -> bool {
        // With the prompt editor off the shell owns every prompt, always. That
        // is what keeps the gap hold and the typeahead record — both of which
        // exist to feed anterm's editor, and one of which erases the line with
        // ^U before doing it — away from a line only ZLE is editing.
        !self.prompt.prompt_editor || self.handoff_active()
    }

    pub fn on_alt_screen(&self) -> bool {
        self.terminal
            .mode_nonblocking()
            .contains(TermMode::ALT_SCREEN)
    }

    fn typeahead_blocked(&self) -> bool {
        self.terminal.foreground_agent().is_some()
            || self.terminal.agent_session().is_some()
            || self.on_alt_screen()
    }

    pub(super) fn sync_typeahead_owner(&mut self) {
        let blocked = self.typeahead_blocked();
        sync_typeahead_owner_state(
            &mut self.prompt.typeahead,
            &mut self.agent.last_typeahead_blocked,
            blocked,
        );
    }

    pub(super) fn observe_typeahead(&mut self, input: RawInput<'_>) {
        // The input that crosses an ownership boundary belongs to neither side.
        let blocked = self.typeahead_blocked();
        observe_typeahead_for_owner(
            &mut self.prompt.typeahead,
            &mut self.agent.last_typeahead_blocked,
            input,
            blocked,
        );
    }

    pub(super) fn flush_typeahead(&mut self) {
        let pasted = self.prompt.typeahead.pasted();
        let Some(seed) = self.prompt.typeahead.drain() else {
            return;
        };
        self.terminal.write(vec![0x15]);
        if !seed.is_empty() {
            self.prepend_into_editor(&seed, pasted);
        }
    }

    /// Take the record into the editor without paying the wipe yet. Every door
    /// into the editor opens with this.
    ///
    /// `at_prompt` comes back on the `D` mark, a whole prompt draw ahead of the
    /// `B` that arms `zle_reading`, and this editor is live for that whole
    /// window. Everything it offers rewrites the line — history recall and the
    /// ghost suggestion replace it wholesale, ⌃U empties it, completion filters
    /// on it — so the line has to be whole *before* those run, not stitched
    /// back together at submit time in front of whatever replaced it. Folding
    /// it in that late made `↑` then Enter run the recalled entry with the gap
    /// text glued to its front, and ⌃U then Enter bring back the text ⌃U had
    /// just cleared.
    ///
    /// The `^U` stays owed until `flush_typeahead`, which keeps it where it has
    /// always been on the wire: immediately before the line. Sending it here
    /// instead would put it out before the shell's own editor is reading.
    pub(super) fn adopt_typeahead(&mut self) {
        let pasted = self.prompt.typeahead.pasted();
        if let Some(seed) = self.prompt.typeahead.adopt() {
            self.prepend_into_editor(&seed, pasted);
        }
    }

    pub(super) fn wipe_pending_typeahead(&mut self) {
        if self.prompt.typeahead.drain().is_some() {
            self.terminal.write(vec![0x15]);
        }
    }

    pub(super) fn gap_holdable(&self) -> bool {
        self.terminal.shell_active() && !self.on_alt_screen() && !self.shell_owns_prompt()
    }

    /// `pasted` says the text came off the clipboard rather than the keyboard,
    /// so that a paste the hold keeps for the editor still reaches it marked.
    pub(super) fn write_gap_text(&mut self, text: &str, bytes: Vec<u8>, pasted: bool, cx: &mut Context<Self>) {
        if self.shell_owns_prompt() {
            self.release_hold();
            self.terminal.write(bytes);
            return;
        }
        if self.gap_holdable() && !text.chars().any(char::is_control) {
            let held = if pasted {
                self.prompt.hold.hold_pasted_text(text, &bytes)
            } else {
                self.prompt.hold.hold_text(text, &bytes)
            };
            match held {
                Verdict::Held(arm) => {
                    if let Some(epoch) = arm {
                        self.arm_hold_timer(epoch, cx);
                    }
                    return;
                }
                Verdict::Passthrough => {}
            }
        } else {
            self.release_hold();
        }
        self.terminal.write(bytes);
        self.observe_gap_text(text, pasted);
    }

    /// Move whatever the gap hold collected into the editor's buffer, keeping
    /// the paste mark with it.
    ///
    /// The hold is the one route into that buffer that does not run through
    /// the editor: text arriving before the prompt does is kept out here and
    /// prepended when the editor takes over. A paste that lost its provenance
    /// on the way would be submitted as typed (#660) — see
    /// [`CmdEditor::prepend_pasted`].
    pub(super) fn engage_hold_into_editor(&mut self) {
        let pasted = self.prompt.hold.pasted();
        let Some(net) = self.prompt.hold.engage() else {
            return;
        };
        self.prepend_into_editor(&net, pasted);
    }

    /// Put text in front of the editor's line that the editor did not receive
    /// through its own keys, keeping the provenance that decides how the line
    /// is submitted (#660). Both routes that do this — the gap hold and the
    /// typeahead record — carry a `pasted()` to read before they hand over.
    fn prepend_into_editor(&mut self, text: &str, pasted: bool) {
        if pasted {
            self.prompt.cmd.prepend_pasted(text);
        } else {
            self.prompt.cmd.prepend_str(text);
        }
    }

    /// A gap's text on its way to the record, which replays it into the editor
    /// later — so a paste has to be recorded as one.
    fn observe_gap_text(&mut self, text: &str, pasted: bool) {
        self.observe_typeahead(if pasted {
            RawInput::Pasted(text)
        } else {
            RawInput::Text(text)
        });
    }

    pub(super) fn release_hold(&mut self) {
        let pasted = self.prompt.hold.pasted();
        if let Some((net, bytes)) = self.prompt.hold.release() {
            self.terminal.write(bytes);
            self.observe_gap_text(&net, pasted);
        }
    }

    pub(super) fn arm_hold_timer(&mut self, epoch: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOLD_WINDOW).await;
            let _ = this.update(cx, |view, cx| view.dump_hold(epoch, cx));
        })
        .detach();
    }

    pub(super) fn dump_hold(&mut self, epoch: u64, cx: &mut Context<Self>) {
        if !self.accepts_input(cx) {
            let _ = self.prompt.hold.timeout(epoch);
            return;
        }
        let pasted = self.prompt.hold.pasted();
        if let Some((net, bytes)) = self.prompt.hold.timeout(epoch) {
            self.terminal.write(bytes);
            self.observe_gap_text(&net, pasted);
            cx.notify();
        }
    }

    pub(super) fn insert_newline_action(&mut self, cx: &mut Context<Self>) {
        if !self.input_active() || self.prompt.reverse_search.is_some() {
            cx.propagate();
            return;
        }
        self.jump_to_prompt();
        self.close_completion();
        self.render.cursor_visible = true;
        self.prompt.cmd.insert_str("\n");
        self.reset_history_nav();
        self.prompt.editor_goal_col = None;
        self.prompt.last_word_nav = None;
        cx.notify();
    }

    pub(super) fn insert_newline_fallback_action(&mut self, cx: &mut Context<Self>) {
        if self.input_active() {
            self.insert_newline_action(cx);
        } else if (self.search_state.active.is_some() && self.search_state.focused)
            || self.key_flags().kitty_active()
            || !self.accepts_input(cx)
        {
            cx.propagate();
        } else {
            self.send_shortcut_bytes(self.key_flags().legacy_newline_bytes(), "enter", cx);
        }
    }

    pub(super) fn accept_line(&mut self, cx: &mut Context<Self>) {
        if self
            .prompt
            .completion
            .as_ref()
            .is_some_and(|s| s.selected().is_some())
        {
            self.completion_accept(cx);
            return;
        }
        self.close_completion();
        self.submit_command(cx);
    }

    pub(super) fn submit_command(&mut self, cx: &mut Context<Self>) {
        if self.terminal.exited || !self.accepts_input(cx) {
            return;
        }
        self.engage_hold_into_editor();
        // The shell is still holding the recorded text on its own line, and the
        // ^U that erases it has not gone out yet: `at_prompt` comes back on the
        // `D` mark, before the prompt is even drawn, while the wipe waits for
        // `B`. The key that got here has folded the seed into the line already,
        // so this is usually just paying the wipe that left owed; where nothing
        // has, the drain still puts the seed back the way every other drain
        // does. Dropping it submitted only what was typed after the handover,
        // and an empty command when that was nothing, which is the blank line
        // #433 reports.
        self.flush_typeahead();
        let line = self.prompt.cmd.text();
        if !line.trim().is_empty() {
            self.record_history_entry(line.clone(), self.terminal.prompt_seq(), cx);
        }
        // The dedup above can shift entries down, so nothing may keep an
        // index into `history` across a submit.
        self.reset_history_nav();
        self.prompt.last_word_nav = None;
        self.close_completion();

        let bracketed = self
            .terminal
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        let pasted = self.prompt.cmd.pasted();
        self.terminal.write(submit_bytes(&line, bracketed, pasted));
        self.prompt.cmd.clear();
        self.render.cursor_visible = true;
        self.jump_to_prompt();
        cx.notify();
    }

    fn record_history_entry(&mut self, line: String, seq: u64, cx: &mut Context<Self>) {
        if crate::prompt::history::should_ignore_command(&line) {
            return;
        }
        let cwd = self.cwd();
        let now = unix_now();
        let cwd_str = cwd.as_ref().and_then(|p| p.to_str()).map(str::to_string);
        self.flush_pending_history(cx);

        let recorded = cx.default_global::<HistoryStore>().record(RecordInput {
            scope: self.prompt.history_scope.clone(),
            session_id: self.prompt.history_session_id.clone(),
            cmd: line.clone(),
            cwd: cwd_str,
            ts: now,
            duration_ms: None,
            exit: None,
        });
        if let Some((req_id, worker)) = recorded {
            self.prompt.pending_history = Some(PendingHistory {
                line,
                seq,
                req_id,
                started_at: std::time::Instant::now(),
                worker,
            });
        }
        self.prompt.ranked_cwd = cwd;
        self.invalidate_ghost_cache();
    }

    fn drain_submitted_commands(&mut self, cx: &mut Context<Self>) {
        for (raw_mark, seq) in self.terminal.take_submitted_commands() {
            if raw_mark.contains("%0A") || raw_mark.contains("%0a") {
                continue;
            }
            let line = unescape_mark_text(&raw_mark);
            if crate::prompt::history::should_ignore_command(&line) {
                continue;
            }
            if self.prompt.pending_history.as_ref().is_some_and(|p| {
                p.seq == seq
                    || p.line == line
                    || ((p.line.contains('\n') || p.line.len() > 500) && p.line.starts_with(&line))
            }) {
                continue;
            }
            self.record_history_entry(line, seq, cx);
            self.remap_active_history_nav(cx);
            self.prompt.last_word_nav = None;
        }
    }

    pub(super) fn sync_shared_history(&mut self, cx: &mut Context<Self>) {
        self.drain_submitted_commands(cx);
        self.remap_active_history_nav(cx);
    }

    pub(super) fn insert_last_word(&mut self, cx: &mut Context<Self>) {
        self.sync_shared_history(cx);
        let resumed = self.prompt.last_word_nav.take().filter(|walk| {
            let len = walk.word.chars().count();
            self.prompt.cmd.cursor() == walk.at + len
                && self.prompt.cmd.selection().is_none()
                && self
                    .prompt
                    .cmd
                    .text()
                    .chars()
                    .skip(walk.at)
                    .take(len)
                    .eq(walk.word.chars())
        });
        let found = {
            let store = cx.default_global::<HistoryStore>();
            let entries = &store.snapshot(&self.prompt.history_scope).entries;
            let start = match &resumed {
                Some(walk) => walk.entry.checked_sub(1),
                None => entries.len().checked_sub(1),
            };
            start.and_then(|mut entry| loop {
                if let Some(w) = entries
                    .get(entry)
                    .and_then(|s| s.split_whitespace().next_back())
                {
                    break Some((entry, w.to_string()));
                }
                entry = entry.checked_sub(1)?;
            })
        };
        let Some((entry, word)) = found else {
            self.prompt.last_word_nav = resumed;
            return;
        };

        if let Some(walk) = resumed {
            self.prompt.cmd.clear_selection();
            self.prompt.cmd.set_cursor(walk.at);
            self.prompt.cmd.extend_to(walk.at + walk.word.chars().count());
            self.prompt.cmd.delete_selection();
        }
        self.prompt.cmd.insert_str(&word);
        let at = self.prompt.cmd.cursor() - word.chars().count();
        self.prompt.last_word_nav = Some(LastWordWalk { entry, at, word });
        self.reset_history_nav();
        cx.notify();
    }

    pub(super) fn reset_history_nav(&mut self) {
        self.prompt.history_nav = None;
        self.prompt.history_stash.clear();
        self.prompt.history_prefix.clear();
    }

    /// Re-anchor active `↑`/`↓` navigation if the shared [`ScopeSnapshot`]
    /// mutated underneath us (e.g. another pane ran a command or an async DB
    /// load landed).
    fn remap_active_history_nav(&mut self, cx: &mut Context<Self>) {
        let Some(nav) = self.prompt.history_nav.as_ref() else {
            return;
        };
        let per_pane = cx.global::<Config>().per_pane_history;
        let store = cx.default_global::<HistoryStore>();
        let snap = store.snapshot(&self.prompt.history_scope);
        if nav.snapshot_gen == snap.generation {
            return;
        }
        if self.prompt.cmd.text() != nav.line {
            self.reset_history_nav();
            return;
        }
        let line = nav.line.clone();
        let pos = match nav.pos {
            HistoryNavPos::Session(_) if per_pane => snap
                .session_entries(&self.prompt.history_session_id)
                .iter()
                .rposition(|h| *h == line)
                .map(HistoryNavPos::Session)
                .or_else(|| {
                    snap.entries
                        .iter()
                        .rposition(|h| *h == line)
                        .map(HistoryNavPos::Host)
                }),
            _ => snap
                .entries
                .iter()
                .rposition(|h| *h == line)
                .map(HistoryNavPos::Host),
        };
        match pos {
            Some(pos) => {
                self.prompt.history_nav = Some(HistoryNav {
                    pos,
                    line,
                    snapshot_gen: snap.generation,
                });
            }
            None => self.reset_history_nav(),
        }
    }

    /// Walk older history, matching the prefix captured when navigation
    /// started — zsh's `up-line-or-beginning-search`, which macOS users get
    /// from ↑ and Ctrl+P.
    ///
    /// When `per_pane_history` is enabled, walks this pane's logical
    /// `session_id` commands first and then smoothly falls back to host-wide
    /// history without repeating commands already shown from this pane.
    pub(super) fn history_prev(&mut self, cx: &mut Context<Self>) {
        self.sync_shared_history(cx);
        let per_pane = cx.global::<Config>().per_pane_history;
        if self.prompt.history_nav.is_none() {
            let line = self.prompt.cmd.text();
            self.prompt.history_prefix = line[..self.prompt.cmd.cursor_byte()].to_string();
            self.prompt.history_stash = line;
        }

        let found = {
            let store = cx.default_global::<HistoryStore>();
            let snap = store.snapshot(&self.prompt.history_scope);
            let session_cmds = snap.session_entries(&self.prompt.history_session_id);
            let host_cmds = &snap.entries;
            let prefix = self.prompt.history_prefix.as_str();
            let in_session = |cmd: &str| -> bool {
                per_pane && session_cmds.iter().any(|s| s.as_str() == cmd)
            };

            let next = match self.prompt.history_nav.as_ref().map(|n| n.pos) {
                None => {
                    if per_pane && !session_cmds.is_empty() {
                        (0..session_cmds.len())
                            .rev()
                            .find(|&i| session_cmds[i].starts_with(prefix))
                            .map(|i| (HistoryNavPos::Session(i), session_cmds[i].clone()))
                            .or_else(|| {
                                (0..host_cmds.len())
                                    .rev()
                                    .find(|&i| {
                                        host_cmds[i].starts_with(prefix)
                                            && !in_session(&host_cmds[i])
                                    })
                                    .map(|i| (HistoryNavPos::Host(i), host_cmds[i].clone()))
                            })
                    } else {
                        (0..host_cmds.len())
                            .rev()
                            .find(|&i| host_cmds[i].starts_with(prefix))
                            .map(|i| (HistoryNavPos::Host(i), host_cmds[i].clone()))
                    }
                }
                Some(HistoryNavPos::Session(i)) => (0..i.min(session_cmds.len()))
                    .rev()
                    .find(|&j| session_cmds[j].starts_with(prefix))
                    .map(|j| (HistoryNavPos::Session(j), session_cmds[j].clone()))
                    .or_else(|| {
                        (0..host_cmds.len())
                            .rev()
                            .find(|&j| {
                                host_cmds[j].starts_with(prefix) && !in_session(&host_cmds[j])
                            })
                            .map(|j| (HistoryNavPos::Host(j), host_cmds[j].clone()))
                    }),
                Some(HistoryNavPos::Host(i)) => (0..i.min(host_cmds.len()))
                    .rev()
                    .find(|&j| host_cmds[j].starts_with(prefix) && !in_session(&host_cmds[j]))
                    .map(|j| (HistoryNavPos::Host(j), host_cmds[j].clone())),
            };
            next.map(|(pos, line)| (pos, line, snap.generation))
        };

        if let Some((pos, line, snapshot_gen)) = found {
            self.prompt.cmd.set(&line);
            self.prompt.history_nav = Some(HistoryNav {
                pos,
                line,
                snapshot_gen,
            });
            cx.notify();
        }
    }

    pub(super) fn history_next(&mut self, cx: &mut Context<Self>) {
        self.remap_active_history_nav(cx);
        let Some(cur_pos) = self.prompt.history_nav.as_ref().map(|n| n.pos) else {
            return;
        };
        let per_pane = cx.global::<Config>().per_pane_history;
        let found = {
            let store = cx.default_global::<HistoryStore>();
            let snap = store.snapshot(&self.prompt.history_scope);
            let session_cmds = snap.session_entries(&self.prompt.history_session_id);
            let host_cmds = &snap.entries;
            let prefix = self.prompt.history_prefix.as_str();
            let in_session = |cmd: &str| -> bool {
                per_pane && session_cmds.iter().any(|s| s.as_str() == cmd)
            };

            let next = match cur_pos {
                HistoryNavPos::Host(i) => (i + 1..host_cmds.len())
                    .find(|&j| host_cmds[j].starts_with(prefix) && !in_session(&host_cmds[j]))
                    .map(|j| (HistoryNavPos::Host(j), host_cmds[j].clone()))
                    .or_else(|| {
                        if per_pane && !session_cmds.is_empty() {
                            (0..session_cmds.len())
                                .find(|&j| session_cmds[j].starts_with(prefix))
                                .map(|j| (HistoryNavPos::Session(j), session_cmds[j].clone()))
                        } else {
                            None
                        }
                    }),
                HistoryNavPos::Session(i) => (i + 1..session_cmds.len())
                    .find(|&j| session_cmds[j].starts_with(prefix))
                    .map(|j| (HistoryNavPos::Session(j), session_cmds[j].clone())),
            };
            next.map(|(pos, line)| (pos, line, snap.generation))
        };

        if let Some((pos, line, snapshot_gen)) = found {
            self.prompt.cmd.set(&line);
            self.prompt.history_nav = Some(HistoryNav {
                pos,
                line,
                snapshot_gen,
            });
        } else {
            self.prompt.history_nav = None;
            self.prompt.history_prefix.clear();
            let stash = std::mem::take(&mut self.prompt.history_stash);
            self.prompt.cmd.set(&stash);
        }
        cx.notify();
    }

    pub(super) fn flush_pending_history(&mut self, cx: &mut Context<Self>) {
        let Some(p) = self.prompt.pending_history.take() else {
            return;
        };
        let finished = self.terminal.prompt_seq() > p.seq && self.terminal.at_prompt();
        if !finished {
            return;
        }
        let exit = self.terminal.last_exit_code();
        let duration_ms = Some(p.started_at.elapsed().as_millis() as u64);
        cx.default_global::<HistoryStore>().finish(
            p.req_id,
            &self.prompt.history_scope,
            &p.line,
            exit,
            duration_ms,
        );
        self.invalidate_ghost_cache();
    }

    pub(super) fn flush_pending_history_on_drop(&mut self) {
        let Some(p) = self.prompt.pending_history.take() else {
            return;
        };
        if self.terminal.prompt_seq() > p.seq && self.terminal.at_prompt() {
            let exit = self.terminal.last_exit_code();
            let duration_ms = Some(p.started_at.elapsed().as_millis() as u64);
            p.worker.send_finish(p.req_id, exit, duration_ms);
        }
    }

    /// The inline suggestion: the most recent entry that extends the line,
    /// preferring this pane (when `per_pane_history` is enabled), then the
    /// current directory, then the most recent anywhere on this host. Commands
    /// that last exited non-zero are never offered. Results are cached across
    /// cursor-blink repaints.
    pub(super) fn ghost_suggestion(&self, cx: &App) -> Option<String> {
        if self.prompt.cmd.is_empty() || self.prompt.cmd.cursor() != self.prompt.cmd.len() {
            return None;
        }
        let Some(store) = cx.try_global::<HistoryStore>() else {
            return None;
        };
        let snap = store.snapshot(&self.prompt.history_scope);
        let per_pane = cx
            .try_global::<Config>()
            .is_some_and(|c| c.per_pane_history);
        let line = self.prompt.cmd.text();
        if let Some(cache) = self.prompt.ghost_cache.borrow().as_ref()
            && cache.line == line
            && cache.scope == self.prompt.history_scope
            && cache.snapshot_gen == snap.generation
            && cache.cwd == self.prompt.ranked_cwd
            && cache.per_pane == per_pane
            && cache.session_id == self.prompt.history_session_id
        {
            return cache.result.clone();
        }

        let is_failed = |h: &str| {
            snap.meta
                .get(h)
                .and_then(|m| m.exit)
                .is_some_and(|code| code != 0)
        };

        let result = if per_pane {
            snap.session_entries(&self.prompt.history_session_id)
                .iter()
                .rev()
                .find(|h| h.len() > line.len() && h.starts_with(&line) && !is_failed(h))
                .cloned()
        } else {
            None
        }
        .or_else(|| {
            let cwd = self.prompt.ranked_cwd.as_deref().and_then(|p| p.to_str());
            let mut elsewhere = None;
            for h in snap.entries.iter().rev() {
                if h.len() <= line.len() || !h.starts_with(&line) {
                    continue;
                }
                if is_failed(h) {
                    continue;
                }
                let here = cwd.is_some_and(|dir| {
                    snap.cwds
                        .get(h)
                        .is_some_and(|dirs| dirs.contains(dir))
                });
                if here || cwd.is_none() {
                    return Some(h.clone());
                }
                elsewhere.get_or_insert(h);
            }
            elsewhere.cloned()
        });

        *self.prompt.ghost_cache.borrow_mut() = Some(GhostCache {
            line,
            scope: self.prompt.history_scope.clone(),
            snapshot_gen: snap.generation,
            cwd: self.prompt.ranked_cwd.clone(),
            per_pane,
            session_id: self.prompt.history_session_id.clone(),
            result: result.clone(),
        });
        result
    }

    pub(super) fn note_integration_gap(&mut self, cx: &mut Context<Self>) {
        if self.prompt.integration_notice_shown
            || self.terminal.shell_active()
            || self.on_alt_screen()
            || self.session.created_at.elapsed() < INTEGRATION_GRACE
        {
            return;
        }
        self.prompt.integration_notice_shown = true;
        self.prompt.integration_notice = Some(integration_notice_message(None));
        cx.notify();

        let pane_id = self.pane_id;
        let route = self.pane_route();
        cx.spawn(async move |this, cx| {
            let fg = cx
                .background_executor()
                .spawn(async move {
                    RemoteTerminal::list_panes_on(&route)
                        .into_iter()
                        .find(|p| p.pane_id == pane_id)
                        .map(|p| p.title)
                })
                .await;
            if let Some(shim) = fg.as_deref().and_then(known_pty_shim) {
                let _ = this.update(cx, |view, cx| {
                    if view.prompt.integration_notice.is_some() {
                        view.prompt.integration_notice = Some(integration_notice_message(Some(shim)));
                        cx.notify();
                    }
                });
            }
            cx.background_executor()
                .timer(INTEGRATION_NOTICE_TIMEOUT)
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.prompt.integration_notice.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Active history snapshot for the given reverse-search filter.
    pub(super) fn reverse_search_snapshot<'a>(
        &self,
        filter: HistoryFilter,
        store: &'a HistoryStore,
    ) -> &'a ScopeSnapshot {
        if filter == HistoryFilter::All
            && let Some(all) = store.all_snapshot()
        {
            return all;
        }
        store.snapshot(&self.prompt.history_scope)
    }

    fn ensure_frecency(&mut self, filter: HistoryFilter, cx: &mut App) {
        let is_all = filter == HistoryFilter::All;
        let per_pane = cx
            .try_global::<Config>()
            .is_some_and(|c| c.per_pane_history);
        let store = cx.default_global::<HistoryStore>();
        let snap = self.reverse_search_snapshot(filter, store);
        let key = (snap.generation, self.prompt.ranked_cwd.clone(), is_all);
        if self.prompt.history_frecency_key == key && self.prompt.history_frecency.len() == snap.entries.len() {
            return;
        }
        let cwd_str = self.prompt.ranked_cwd.as_deref().and_then(|p| p.to_str());
        let session_boost = (per_pane && !is_all).then_some(self.prompt.history_session_id.as_str());
        self.prompt.history_frecency = snap.frecency_scores(cwd_str, session_boost);
        self.prompt.history_frecency_key = key;
    }

    fn build_reverse_search_filter<'a>(
        filter: HistoryFilter,
        snap: &'a ScopeSnapshot,
        session_id: &str,
        cwd: Option<&'a str>,
    ) -> Box<dyn Fn(&str) -> bool + 'a> {
        match filter {
            HistoryFilter::Session => {
                let set: std::collections::HashSet<&'a str> = snap
                    .session_entries(session_id)
                    .iter()
                    .map(String::as_str)
                    .collect();
                Box::new(move |line| set.contains(line))
            }
            HistoryFilter::Directory => Box::new(move |line| {
                cwd.is_some_and(|dir| {
                    snap.cwds
                        .get(line)
                        .is_some_and(|dirs| dirs.contains(dir))
                })
            }),
            HistoryFilter::Host | HistoryFilter::All => Box::new(|_| true),
        }
    }

    pub(super) fn start_reverse_search(&mut self, cx: &mut Context<Self>) {
        if self.prompt.reverse_search.is_some() {
            return;
        }
        let filter = if cx.global::<Config>().per_pane_history {
            HistoryFilter::Session
        } else {
            HistoryFilter::Host
        };
        self.ensure_frecency(filter, cx);
        let cwd = self.prompt.ranked_cwd.clone();
        let cwd_str = cwd.as_deref().and_then(|p| p.to_str());
        let store = cx.default_global::<HistoryStore>();
        let snap = self.reverse_search_snapshot(filter, store);
        let include =
            Self::build_reverse_search_filter(filter, snap, &self.prompt.history_session_id, cwd_str);
        self.prompt.reverse_search = Some(ReverseSearch::new_with_filter(
            filter,
            &snap.entries,
            &self.prompt.history_frecency,
            include,
        ));
    }

    pub(super) fn refresh_reverse_search(&mut self, cx: &mut Context<Self>) {
        let Some(filter) = self.prompt.reverse_search.as_ref().map(|rs| rs.filter()) else {
            return;
        };
        self.ensure_frecency(filter, cx);
        let cwd = self.prompt.ranked_cwd.clone();
        let cwd_str = cwd.as_deref().and_then(|p| p.to_str());
        let store = cx.default_global::<HistoryStore>();
        let snap = self.reverse_search_snapshot(filter, store);
        let include =
            Self::build_reverse_search_filter(filter, snap, &self.prompt.history_session_id, cwd_str);
        if let Some(rs) = self.prompt.reverse_search.as_mut() {
            rs.update_with_filter(&snap.entries, &self.prompt.history_frecency, include);
        }
    }

    pub(super) fn push_reverse_search_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(filter) = self.prompt.reverse_search.as_ref().map(|rs| rs.filter()) else {
            return;
        };
        self.ensure_frecency(filter, cx);
        let cwd = self.prompt.ranked_cwd.clone();
        let cwd_str = cwd.as_deref().and_then(|p| p.to_str());
        let store = cx.default_global::<HistoryStore>();
        let snap = self.reverse_search_snapshot(filter, store);
        let include =
            Self::build_reverse_search_filter(filter, snap, &self.prompt.history_session_id, cwd_str);
        if let Some(rs) = self.prompt.reverse_search.as_mut() {
            rs.push_query_with_filter(text, &snap.entries, &self.prompt.history_frecency, include);
        }
    }

    pub(super) fn set_reverse_search_filter(
        &mut self,
        filter: HistoryFilter,
        cx: &mut Context<Self>,
    ) {
        if self.prompt.reverse_search.is_none() {
            return;
        }
        if filter == HistoryFilter::All {
            let maybe_worker = {
                let store = cx.default_global::<HistoryStore>();
                if store.all_snapshot().is_none() {
                    store.begin_all_load()
                } else {
                    None
                }
            };
            if let Some(worker) = maybe_worker {
                let rx = worker.request_all_load();
                cx.spawn(async move |this, cx| {
                    if let Ok(loaded) = rx.recv().await {
                        let _ = this.update(cx, |view, cx| {
                            cx.default_global::<HistoryStore>()
                                .install_all_snapshot(loaded);
                            view.refresh_reverse_search(cx);
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
        }
        self.ensure_frecency(filter, cx);
        {
            let cwd = self.prompt.ranked_cwd.clone();
            let cwd_str = cwd.as_deref().and_then(|p| p.to_str());
            let store = cx.default_global::<HistoryStore>();
            let snap = self.reverse_search_snapshot(filter, store);
            let include =
                Self::build_reverse_search_filter(filter, snap, &self.prompt.history_session_id, cwd_str);
            if let Some(rs) = self.prompt.reverse_search.as_mut() {
                rs.set_filter(filter, &snap.entries, &self.prompt.history_frecency, include);
            }
        }
        cx.notify();
    }

    pub(super) fn cycle_reverse_search_filter(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(cur) = self.prompt.reverse_search.as_ref().map(|rs| rs.filter()) else {
            return;
        };
        let next = if forward { cur.next() } else { cur.prev() };
        self.set_reverse_search_filter(next, cx);
    }

    fn delete_reverse_search_entry(&mut self, line: String, cx: &mut Context<Self>) {
        let is_all = self
            .prompt
            .reverse_search
            .as_ref()
            .is_some_and(|rs| rs.filter() == HistoryFilter::All);
        let scope = (!is_all).then_some(&self.prompt.history_scope);
        cx.default_global::<HistoryStore>().delete(scope, &line);
        self.invalidate_ghost_cache();
        self.refresh_reverse_search(cx);
    }

    pub(super) fn handle_reverse_search_key(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) {
        let m = &ks.modifiers;
        if !m.control && !m.platform && !m.alt {
            if let Some(ch) = ks.key_char.as_deref() {
                if !ch.is_empty() && ch.chars().all(|c| c >= '\u{20}' && c != '\u{7f}') {
                    self.push_reverse_search_query(ch, cx);
                    cx.notify();
                    return;
                }
            }
        }
        let Some(filter) = self.prompt.reverse_search.as_ref().map(|rs| rs.filter()) else {
            return;
        };
        self.ensure_frecency(filter, cx);
        let action = {
            let cwd = self.prompt.ranked_cwd.clone();
            let cwd_str = cwd.as_deref().and_then(|p| p.to_str());
            let store = cx.default_global::<HistoryStore>();
            let snap = self.reverse_search_snapshot(filter, store);
            let include =
                Self::build_reverse_search_filter(filter, snap, &self.prompt.history_session_id, cwd_str);
            self.prompt.reverse_search
                .as_mut()
                .map(|rs| rs.handle_key_with_filter(ks, &snap.entries, &self.prompt.history_frecency, include))
        };
        let Some(action) = action else {
            return;
        };
        match action {
            reverse_search::Action::Redraw => {}
            reverse_search::Action::Cancel => self.prompt.reverse_search = None,
            reverse_search::Action::CycleFilter(forward) => {
                self.cycle_reverse_search_filter(forward, cx);
                return;
            }
            reverse_search::Action::DeleteSelected(line) => {
                self.delete_reverse_search_entry(line, cx);
            }
            reverse_search::Action::Accept(line) => {
                self.prompt.reverse_search = None;
                if let Some(line) = line {
                    self.prompt.cmd.set(&line);
                    self.reset_history_nav();
                }
            }
            reverse_search::Action::Run(line) => {
                self.prompt.reverse_search = None;
                self.prompt.cmd.set(&line);
                self.submit_command(cx);
            }
        }
        cx.notify();
    }

    /// The menu's Cmd+Enter, which arrives as an action rather than a key:
    /// on macOS the chord is `ToggleFullscreen`, and a window-wide binding is
    /// matched before the pane's key handler ever sees the keystroke.
    pub(super) fn run_reverse_search_selection(&mut self, cx: &mut Context<Self>) {
        let Some(rs) = self.prompt.reverse_search.take() else {
            return;
        };
        let line = {
            let store = cx.default_global::<HistoryStore>();
            let snap = self.reverse_search_snapshot(rs.filter(), store);
            rs.selected_line(&snap.entries).map(str::to_string)
        };
        if let Some(line) = line {
            self.prompt.cmd.set(&line);
            self.submit_command(cx);
        }
        cx.notify();
    }

    pub(super) fn handoff_line_to_shell(&mut self, chord: &[u8], cx: &mut Context<Self>) {
        if !self.accepts_input(cx) {
            return;
        }
        self.engage_hold_into_editor();
        // Same reason as `submit_command`: what the record holds is on the
        // shell's own line, so it belongs in front of the line handed back.
        // The wipe waits until past the multi-line bail, which hands nothing
        // over and so must put nothing on the wire either.
        self.adopt_typeahead();
        let line = self.prompt.cmd.text();
        if line.contains('\n') {
            cx.notify();
            return;
        }
        self.close_completion();
        self.flush_typeahead();
        let tail = line.chars().count().saturating_sub(self.prompt.cmd.cursor());
        if !line.is_empty() {
            self.terminal.write(line.into_bytes());
            if tail > 0 {
                let left: &[u8] = if self.key_flags().app_cursor() {
                    b"\x1bOD"
                } else {
                    b"\x1b[D"
                };
                self.terminal.write(left.repeat(tail));
            }
        }
        self.prompt.cmd.clear();
        self.prompt.editor_handoff = Some(self.terminal.prompt_cycle());
        self.prompt.editor_handoff_interrupt_seq = None;
        self.send_to_pty(chord, cx);
    }

    pub(super) fn tab_pressed(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.search_state.focused {
            cx.propagate();
            return;
        }
        if let Some(reason) = self.link_inactive_reason(cx) {
            log::debug!(target: "anterm::completion", "Tab does nothing and the line stays: {reason}");
            return;
        }
        if let Some(reason) = self.input_inactive_reason() {
            log::debug!(target: "anterm::completion", "Tab goes straight to the PTY: {reason}");
            let bytes = self.tab_bytes(!forward);
            self.send_to_pty(&bytes, cx);
            return;
        }
        self.complete_tab(forward, cx);
    }

    fn handoff_tab_to_shell(&mut self, shift: bool, cx: &mut Context<Self>) {
        let bytes = self.tab_bytes(shift);
        self.handoff_line_to_shell(&bytes, cx);
    }

    pub(super) fn complete_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.prompt.reverse_search.is_some() {
            self.cycle_reverse_search_filter(forward, cx);
            return;
        }
        if !cx.global::<Config>().tab_completion {
            log::debug!(target: "anterm::completion", "handing the line to the shell: tab_completion is off");
            self.handoff_tab_to_shell(!forward, cx);
            return;
        }
        if self.prompt.completion.is_some() {
            self.completion_tab_step(forward, cx);
            return;
        }

        let cwd = self
            .paths_are_local()
            .then(|| self.local_cwd().or_else(|| std::env::current_dir().ok()))
            .flatten();
        let share_cwd = if cwd.is_none() {
            self.wsl_share_cwd()
        } else {
            None
        };
        let line = self.prompt.cmd.text();
        let cursor = self.prompt.cmd.cursor();
        let comp = match &share_cwd {
            Some(share) => crate::prompt::completion::complete_foreign(&line, cursor, share),
            None => crate::prompt::completion::complete(
                &line,
                cursor,
                cwd.as_deref(),
                self.shell_program().as_deref(),
            ),
        };
        let Some(comp) = comp else {
            if self.spawn_remote_path_completion(&line, cursor, forward, cx) {
                return;
            }
            log::debug!(
                target: "anterm::completion",
                "handing the line to the shell: no candidates for {line:?} at {cursor} \
                 (local cwd {cwd:?}, share cwd {share_cwd:?}, remote cwd {:?})",
                self.remote_ssh_cwd(),
            );
            self.handoff_tab_to_shell(!forward, cx);
            return;
        };

        let pending_generators = comp.pending.len();

        let (word_start, word_end) = match comp.candidates.first() {
            Some(c) => (c.start, c.end),
            None => (word_start_of(&line, cursor), cursor),
        };
        let Some(generation) = self.offer_candidates(
            &line,
            word_start,
            word_end,
            comp.candidates,
            pending_generators,
            cx,
        ) else {
            return;
        };

        let Some(cwd) = cwd else { return };
        for pending in comp.pending {
            let script = pending.script;
            let cwd = cwd.clone();
            cx.spawn(async move |this, cx| {
                let results = cx
                    .background_executor()
                    .spawn(async move { crate::prompt::generator::run(&script, &cwd) })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.completion_merge(generation, results, cx);
                });
            })
            .detach();
        }
    }

    pub(super) fn offer_candidates(
        &mut self,
        line: &str,
        word_start: usize,
        word_end: usize,
        cands: Vec<completion::Candidate>,
        pending_generators: usize,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let has_pending = pending_generators > 0;
        if !has_pending && cands.len() == 1 {
            let c = cands[0].clone();
            self.completion_insert(&c, c.start);
            self.render.cursor_visible = true;
            cx.notify();
            return None;
        }
        let word: String = line
            .chars()
            .skip(word_start)
            .take(word_end - word_start)
            .collect();
        let shell = self.shell_program();
        let s = CompletionSession::new(word_start, word.clone(), cands, pending_generators);
        if !has_pending
            && let Some(lcp) = s.common_prefix()
            && lcp.chars().count() > word.chars().count()
            && quote_for_shell(&lcp, shell.as_deref()) == lcp
        {
            self.apply_candidate(line, word_start, word_end, &lcp);
        }
        let generation = self.open_completion(s);
        self.render.cursor_visible = true;
        cx.notify();
        Some(generation)
    }

    fn wsl_share_cwd(&self) -> Option<std::path::PathBuf> {
        let distro = wsl_share_distro(
            self.terminal.remote_context().as_ref(),
            self.session.host_id.is_local(),
        )?;
        let cwd = self.cwd()?;
        wsl_share_path(&distro, &cwd.to_string_lossy())
    }

    pub(super) fn remote_ssh_cwd(&self) -> Option<String> {
        let owned = matches!(
            self.terminal.remote_context(),
            Some(remote) if remote.kind == anterm_core::daemon::protocol::RemoteKind::NativeSsh
        );
        if !owned {
            return None;
        }
        let cwd = self.cwd()?.to_string_lossy().into_owned();
        cwd.starts_with('/').then_some(cwd)
    }

    fn spawn_remote_path_completion(
        &mut self,
        line: &str,
        cursor: usize,
        forward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(cwd) = self.remote_ssh_cwd() else {
            return false;
        };
        let Some(req) = completion::remote_path_request(line, cursor, &cwd) else {
            log::debug!(
                target: "anterm::completion",
                "no remote listing to ask for: {line:?} at {cursor} against {cwd}"
            );
            return false;
        };
        if self.prompt.remote_completion_inflight {
            return true;
        }
        self.prompt.remote_completion_inflight = true;
        // The listing takes a network round-trip the menu says nothing about
        // — paint the "listing…" pill now, or a slow link reads as a broken
        // Tab key (#585).
        cx.notify();
        let route = anterm_ui_kit::daemon_rpc::SftpRoute::new(self.pane_id);
        let dir = req.dir.clone();
        let line = line.to_string();
        log::debug!(target: "anterm::completion", "listing {dir} over the remote's own connection");
        cx.spawn(async move |this, cx| {
            let listed = cx.background_spawn(async move { route.list(&dir) }).await;
            let (entries, failed) = match listed {
                Ok(entries) => (entries, None),
                Err(e) => {
                    // A failure is not an empty directory: the two used to
                    // end in the same silence (#585).
                    log::warn!(
                        target: "anterm::completion",
                        "remote listing failed, treating it as no candidates: {e}"
                    );
                    (Vec::new(), Some(e.to_string()))
                }
            };
            let _ = this.update(cx, |view, cx| {
                view.prompt.remote_completion_inflight = false;
                if let Some(error) = failed {
                    view.prompt.remote_completion_notice = Some(t_fmt(
                        L10nKey::CompletionRemoteListingFailed,
                        &[("error", &error)],
                    ));
                }
                view.remote_path_results(req, &line, cursor, entries, forward, cx);
                // An empty listing closes the menu without one — the pill
                // still has to come down.
                cx.notify();
            });
        })
        .detach();
        true
    }

    pub(super) fn remote_path_results(
        &mut self,
        req: completion::RemotePathRequest,
        line: &str,
        cursor: usize,
        listed: Vec<anterm_core::daemon::protocol::SftpEntry>,
        forward: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(reason) = self
            .link_inactive_reason(cx)
            .or_else(|| self.input_inactive_reason())
        {
            log::debug!(
                target: "anterm::completion",
                "dropping a remote listing for {line:?}: {reason}"
            );
            return;
        }
        if self.prompt.cmd.text() != line || self.prompt.cmd.cursor() != cursor {
            log::debug!(
                target: "anterm::completion",
                "dropping a remote listing for {line:?}: the line has moved on"
            );
            return;
        }
        let entries: Vec<completion::RemoteEntry> = listed
            .into_iter()
            .map(|e| completion::RemoteEntry {
                is_dir: e.kind == anterm_core::daemon::protocol::SftpEntryKind::Dir || e.target_is_dir,
                name: e.name,
            })
            .collect();
        let cands = completion::remote_path_candidates(&req, &entries);
        log::debug!(
            target: "anterm::completion",
            "{} entries in {}, {} match the word",
            entries.len(),
            req.dir,
            cands.len()
        );
        if cands.is_empty() {
            self.handoff_tab_to_shell(!forward, cx);
            return;
        }
        self.offer_candidates(line, req.word_start, req.cursor, cands, 0, cx);
    }

    pub(super) fn open_completion(&mut self, session: CompletionSession) -> u64 {
        self.prompt.completion = Some(session);
        self.prompt.completion_generation = self.prompt.completion_generation.wrapping_add(1);
        self.prompt.completion_generation
    }

    pub(super) fn close_completion(&mut self) {
        let _ = self.take_completion();
    }

    fn take_completion(&mut self) -> Option<CompletionSession> {
        let s = self.prompt.completion.take();
        if s.is_some() {
            self.prompt.completion_generation = self.prompt.completion_generation.wrapping_add(1);
        }
        s
    }

    pub(super) fn completion_merge(
        &mut self,
        generation: u64,
        results: Vec<crate::prompt::generator::Parsed>,
        cx: &mut Context<Self>,
    ) {
        if self.prompt.completion_generation != generation || self.prompt.completion.is_none() {
            return;
        }
        let word_start = self.prompt.completion.as_ref().map(|s| s.word_start).unwrap_or(0);
        let chars: Vec<char> = self.prompt.cmd.text().chars().collect();
        let cursor = self.prompt.cmd.cursor().min(chars.len());
        let end = cursor.max(word_start);
        let live_word: String = if cursor >= word_start {
            chars[word_start..cursor].iter().collect()
        } else {
            String::new()
        };
        let new: Vec<completion::Candidate> = results
            .into_iter()
            .map(|p| completion::Candidate {
                text: p.text,
                kind: CandidateKind::Value,
                start: word_start,
                end,
                description: p.description,
                icon: None,
            })
            .collect();
        let spent = match self.prompt.completion.as_mut() {
            Some(s) => {
                s.generator_answered();
                s.merge(new, &live_word);
                s.is_spent()
            }
            None => false,
        };
        if spent {
            self.close_completion();
        }
        cx.notify();
    }

    fn completion_tab_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if forward {
            let shell = self.shell_program();
            let Some(s) = self.prompt.completion.as_ref() else {
                return;
            };
            let (word_start, lcp, lone) = (s.word_start, s.common_prefix(), s.filtered.len() == 1);
            let line = self.prompt.cmd.text();
            let cursor = self.prompt.cmd.cursor().min(line.chars().count());
            if let Some(lcp) = lcp
                && lcp.chars().count() > cursor.saturating_sub(word_start)
            {
                if lone {
                    self.completion_accept(cx);
                    return;
                }
                if quote_for_shell(&lcp, shell.as_deref()) == lcp {
                    self.apply_candidate(&line, word_start, cursor, &lcp);
                    self.render.cursor_visible = true;
                    cx.notify();
                    return;
                }
            }
        }
        self.completion_select(forward, cx);
    }

    pub(super) fn completion_select(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(s) = self.prompt.completion.as_mut() {
            s.select(forward);
            self.render.cursor_visible = true;
            cx.notify();
        }
    }

    pub(super) fn completion_accept(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.take_completion() else {
            return;
        };
        if let Some(c) = s.selected().cloned() {
            self.completion_insert(&c, s.word_start);
        }
        self.render.cursor_visible = true;
        cx.notify();
    }

    pub(super) fn completion_insert(&mut self, cand: &completion::Candidate, start: usize) {
        let line = self.prompt.cmd.text();
        let len = line.chars().count();
        let cursor = self.prompt.cmd.cursor().min(len);
        let mut text = quote_for_shell(&cand.text, self.shell_program().as_deref());
        if cand.is_dir() {
            if !text.ends_with('/') {
                text.push('/');
            }
        } else if cursor == len {
            text.push(' ');
        }
        self.apply_candidate(&line, start, cursor, &text);
    }

    pub(super) fn completion_refilter(&mut self) {
        let Some(s) = self.prompt.completion.as_mut() else {
            return;
        };
        let chars: Vec<char> = self.prompt.cmd.text().chars().collect();
        let cursor = self.prompt.cmd.cursor().min(chars.len());
        let keep = cursor >= s.word_start
            && chars[s.word_start..cursor]
                .iter()
                .all(|c| !c.is_whitespace())
            && {
                let word: String = chars[s.word_start..cursor].iter().collect();
                s.refilter(&word)
            };
        if !keep {
            self.close_completion();
        }
    }

    fn apply_candidate(&mut self, orig: &str, start: usize, end: usize, text: &str) {
        let (line, cursor) = completion::Replacement {
            orig: orig.to_string(),
            start,
            end,
            text: text.to_string(),
        }
        .apply();
        self.prompt.cmd.set_with_cursor(&line, cursor);
        self.reset_history_nav();
    }
}
