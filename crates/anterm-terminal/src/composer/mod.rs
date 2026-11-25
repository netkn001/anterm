//! The message composer: a real text box for writing a prompt to a coding
//! agent the way a chat box lets you — mouse selection, the platform's own
//! editing keys, an IME with its candidates in place, files attached as chips,
//! `/` and `@` menus — and handing it over whole.
//!
//! **Where it sits.** For an agent whose input area anterm can find on the grid
//! (Claude Code, Codex, Gemini) the box is laid *over* that area: the agent's
//! own input line and the status rows under it are covered, so the pane has
//! one input rather than two. Whenever the agent puts something else there —
//! a permission prompt, a model picker, a question — the area stops looking
//! like an input, the box steps aside, and the keyboard goes to the TUI until
//! the input comes back. Any other agent gets the box docked under the grid,
//! taking its rows from it.
//!
//! **What it sends.** Sending is typing, not an API: the text and then Enter,
//! written to the pty — see [`submit_plan`] for why each agent is spoken to
//! slightly differently. The toolbar's controls are the agent's own keys
//! (Shift+Tab, its `/model` and `/effort` commands), and what they show is
//! what the agent said: the mode off the status row the box covers, the model
//! and context off what its hooks report ([`anterm_core::cli_agent::AgentReadout`]). Nothing reported
//! means nothing shown.

mod agents;
mod plan;
mod render;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Rgb;
use gpui::{Context, Entity, Focusable as _, Pixels, Subscription, Window, prelude::*, px};
use gpui_kit::component::input::{InputEvent, RopeExt as _, TextareaState};

use agents::{
    builtin_commands, covers, custom_commands, file_items, fill_screen_rows, held_lines,
    in_shell_mode, makes_room, row_backdrop,
};
#[allow(unused_imports)]
pub(super) use agents::{
    InputArea, MenuItem, Trigger, command_items, input_area, model_label, rows_short, trigger_at,
};
use plan::{SETTLE, clear_input, interrupt_keys};
#[allow(unused_imports)]
pub(super) use plan::{
    Piece, Step, compose_message, mention, mention_pieces, opencode_mention_plan, submit_message,
    submit_plan,
};
use super::view::{GRID_PAD_Y, TerminalView};
use anterm_core::cli_agent::{AgentStatus, CLIAgent};
use anterm_ui_kit::host_ops::{HostId, HostOps};
use anterm_i18n::{L10nKey, t_fmt};
use anterm_ui_kit::search::files::{FileIndex, FileList, walk};

/// How tall the text may grow, in lines, before it scrolls instead.
const MAX_ROWS: usize = 8;

/// How long the agent's input area has to stay gone before the box steps
/// aside. An agent redrawing its screen can pass through a frame without the
/// input on it; a picker or a prompt stays.
pub(super) const STEP_ASIDE_AFTER: Duration = Duration::from_millis(250);

/// A file walk or command scan this recent answers the next menu as it is.
const SOURCES_FRESH_FOR: Duration = Duration::from_secs(30);

/// How far the box stands in from the pane's sides and bottom.
const BOX_INSET: f32 = 16.;

// ---------------------------------------------------------------------------
// State

/// What a pane's composer holds that outlives the view drawing it.
///
/// A view is rebuilt over the same daemon pane whenever its workspace is
/// switched out and back, and a half-written prompt must not be the price of
/// looking at another workspace. Kept by pane, for the life of the app.
#[derive(Default)]
struct ComposerMemory(HashMap<(HostId, u64), Remembered>);

impl gpui::Global for ComposerMemory {}

#[derive(Default, Clone)]
struct Remembered {
    draft: String,
    attached: Vec<String>,
    open: bool,
}

enum Files {
    Unwalked,
    Walking,
    Ready(Arc<FileIndex>, Instant),
    Failed(Instant),
}

/// How the box is on screen this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Presence {
    /// Not wanted, or no agent to talk to.
    Hidden,
    /// Under the grid, taking its rows.
    Docked,
    /// Over the agent's input area, which starts this many rows up from the
    /// bottom of the screen.
    Covering(usize),
    /// Wanted, but the agent is showing something else where its input goes —
    /// the box waits for it to come back.
    SteppedAside,
}

pub(super) struct Composer {
    pub(super) input: Entity<TextareaState>,
    /// Whether the user wants the box on this pane.
    open: bool,
    presence: Presence,
    /// The input area as last found, and since when it has been missing.
    area: Option<InputArea>,
    /// How many rows up from the bottom of the live screen that area starts,
    /// however far the view is scrolled back — what the grid's reserved rows
    /// are worked out from, so scrolling never resizes the pane.
    area_rows: usize,
    missing_since: Option<Instant>,
    /// The box had the keyboard when it stepped aside, and gets it back when
    /// it returns.
    refocus: bool,
    /// Files to send with the message, spelled the way the pane's host reads
    /// them — uploaded already, for a remote pane.
    attached: Vec<String>,
    /// Text typed at the grid while the box covers the input, and whether the
    /// box should take the keyboard with it. Taken in at the next draw, the
    /// first place with a window to edit and focus in.
    typed: String,
    grab: bool,
    /// Writes waiting their turn. Submissions queue rather than interleave, so
    /// a second message sent inside the first one's settle time cannot land
    /// its text between the first one's text and its Enter.
    queue: VecDeque<Step>,
    pumping: bool,
    /// Whose name the placeholder carries.
    named: Option<CLIAgent>,
    /// The row of the `/` or `@` menu the keyboard is on.
    highlighted: usize,
    /// The text as it stood when Esc closed the menu. The menu stays shut
    /// until the text moves on from there.
    dismissed: Option<String>,
    files: Files,
    commands: Option<(Vec<(String, String)>, Instant)>,
    /// The effort level last picked from the toolbar, with how many turns the
    /// agent had finished then. The next finished turn reports the level it
    /// ran at — the pick, or whatever the agent made of it, declined included
    /// — and from then on the report is the truth again.
    effort: Option<(&'static str, u64)>,
    /// The model alias last picked from the toolbar, and the turns finished
    /// then, the same way.
    model: Option<(&'static str, u64)>,
    picker: Option<Picker>,
    /// The agent session the picks above were made in. A pick says nothing
    /// about the next session, which starts from its own settings.
    session: Option<String>,
    /// The box's height as last drawn empty — nothing typed, nothing
    /// attached — which is what it needs of the pane at the least.
    frame_height: Rc<Cell<Option<Pixels>>>,
    /// The colour the agent paints the screen in around its input, when it
    /// paints one of its own — the layer covering the input takes it, so it
    /// is not a strip of another colour around the box — and how wide the
    /// grid's columns paint it.
    backdrop: Option<(Rgb, Pixels)>,
    /// Reusable line buffers for `fill_screen_rows` so scanning does not
    /// allocate `rows` fresh `String`s every frame.
    scratch_rows: Vec<String>,
    /// Grid revision, geometry, and scroll offset at the last screen scan.
    /// When unchanged, `sync_composer` skips locking `Term` and re-scanning.
    last_scanned_rev: u64,
    last_scanned_geom: (usize, usize, usize),
    cached_found: Option<InputArea>,
    cached_backdrop: Option<(Rgb, Pixels)>,
    cached_screen_lines: usize,
    /// Single-slot grace-period timer for `STEP_ASIDE_AFTER` (replaces spawning
    /// a new detached timer task on every frame while the input is absent).
    recheck_task: Option<gpui::Task<()>>,
    pub(super) helix: anterm_editor::helix::HelixState,
    _subs: Vec<Subscription>,
}

/// The toolbar's pop-up lists, each over its own button.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Model,
    Effort,
}

impl TerminalView {
    fn composer_key(&self) -> (HostId, u64) {
        (self.host_id(), self.pane_id)
    }

    fn remember_composer(&self, cx: &mut Context<Self>) {
        let Some(c) = self.composer_state.active.as_ref() else {
            return;
        };
        let entry = Remembered {
            draft: c.input.read(cx).value().to_string(),
            attached: c.attached.clone(),
            open: c.open,
        };
        let key = self.composer_key();
        let memory = cx.default_global::<ComposerMemory>();
        match entry.draft.is_empty() && entry.attached.is_empty() && !entry.open {
            true => memory.0.remove(&key),
            false => memory.0.insert(key, entry),
        };
    }

    fn ensure_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_state.active.is_some() {
            return;
        }
        let remembered = cx
            .try_global::<ComposerMemory>()
            .and_then(|m| m.0.get(&self.composer_key()))
            .cloned()
            .unwrap_or_default();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .submit_on_enter(true)
                .default_value(remembered.draft)
        });
        let subs = vec![cx.subscribe_in(&input, window, Self::on_composer_event)];
        self.composer_state.active = Some(Composer {
            input,
            open: remembered.open,
            presence: Presence::Hidden,
            area: None,
            area_rows: 0,
            missing_since: None,
            refocus: false,
            attached: remembered.attached,
            typed: String::new(),
            grab: false,
            queue: VecDeque::new(),
            pumping: false,
            named: None,
            highlighted: 0,
            dismissed: None,
            files: Files::Unwalked,
            commands: None,
            effort: None,
            model: None,
            picker: None,
            session: None,
            frame_height: Rc::new(Cell::new(None)),
            backdrop: None,
            scratch_rows: Vec::new(),
            last_scanned_rev: 0,
            last_scanned_geom: (0, 0, 0),
            cached_found: None,
            cached_backdrop: None,
            cached_screen_lines: 0,
            recheck_task: None,
            helix: anterm_editor::helix::HelixState::new(anterm_editor::helix::HelixMode::Insert),
            _subs: subs,
        });
    }

    /// Rows the grid leaves empty at its bottom so that the box, covering an
    /// input shorter than itself, covers nothing above it: Oh My Pi's input
    /// is a status line and a prompt line, where the box is some five rows
    /// tall, and would otherwise hide the last lines of the reply. The agent
    /// is told the pane is that much shorter and draws its input higher.
    ///
    /// Only for an agent whose input is that short ([`makes_room`]): the
    /// others' inputs, with their rules and status rows, about hold the box,
    /// and resizing their pane each time the box comes and goes would have
    /// them redraw for a row of padding.
    pub(super) fn composer_reserved_rows(&self) -> usize {
        let Some(c) = self
            .composer_state
            .active
            .as_ref()
            .filter(|c| matches!(c.presence, Presence::Covering(_)))
        else {
            return 0;
        };
        if !self.agent().is_some_and(makes_room) {
            return 0;
        }
        let Some(frame) = c.frame_height.get() else {
            return 0;
        };
        // The input's rows on the live screen, not the ones left in view: a
        // view scrolled back shows fewer of them, and taking that for an
        // input shrinking under the box would resize the pane per line
        // scrolled.
        rows_short(
            frame + px(BOX_INSET),
            self.render.line_height,
            c.area_rows,
            self.render.grid_slack + px(GRID_PAD_Y),
        )
    }

    pub(super) fn presence(&self) -> Presence {
        self.composer_state.active
            .as_ref()
            .map_or(Presence::Hidden, |c| c.presence)
    }

    /// Whether the box is on screen.
    pub(super) fn composer_is_open(&self) -> bool {
        self.composer_state.active.as_ref().is_some_and(|c| c.open)
    }

    pub(super) fn composer_shown(&self) -> bool {
        matches!(self.presence(), Presence::Docked | Presence::Covering(_))
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.composer_state.active.as_ref() else {
            return;
        };
        let focus = c.input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        // Said here as well as by the input's own Focus event, which only
        // comes round on the next frame: what is about to land — a drop, a
        // picked file — has to find the box already holding the keyboard.
        self.composer_state.focused = true;
    }

    /// Open the box and put the caret in it; from inside it, close it; with
    /// it open but the terminal focused, go back into it.
    ///
    /// A pane with no agent in the foreground has nobody to compose for, so
    /// the chord does nothing there.
    pub fn toggle_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent().is_none() {
            return;
        }
        self.ensure_composer(window, cx);
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        let focused = c.input.read(cx).focus_handle(cx).is_focused(window);
        match (c.open, focused) {
            (true, true) => {
                c.open = false;
                c.refocus = false;
                window.focus(&self.focus_handle, cx);
            }
            _ => {
                // Focused here and now. `refocus` is for a box coming back
                // from stepping aside; set here it would outlive this focus
                // and pull the keyboard back from the next click elsewhere.
                c.open = true;
                c.refocus = false;
                self.focus_composer(window, cx);
            }
        }
        self.remember_composer(cx);
        cx.notify();
    }

    /// Esc in the box. Over the agent's input it is the agent's Esc — the one
    /// that interrupts a turn — since the box is standing in for that input;
    /// for an agent whose Esc does not, its own interrupt key
    /// ([`interrupt_keys`]).
    /// Docked, it hands the keyboard back to the terminal and leaves the box
    /// where it is, so the next Esc is the agent's.
    pub(super) fn composer_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.presence() {
            Presence::Covering(_) => {
                let Some(agent) = self.agent() else { return };
                if let Some(c) = self.composer_state.active.as_mut() {
                    c.queue.extend(interrupt_keys(Some(agent)));
                }
                self.pump_composer(agent, cx);
            }
            _ => {
                window.focus(&self.focus_handle, cx);
                cx.notify();
            }
        }
    }

    /// Ctrl+C in the box: clear what is written, or — with nothing written —
    /// the agent's Ctrl+C.
    pub(super) fn composer_interrupt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        let empty = c.input.read(cx).value().is_empty() && c.attached.is_empty();
        if empty {
            self.send_to_pty(b"\x03", cx);
            return;
        }
        c.attached.clear();
        c.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.remember_composer(cx);
    }

    /// Typing at the grid while the box covers the agent's input: the input
    /// the keys were meant for is under the box, so they go into the box.
    ///
    /// So does text handed to the terminal while the box has the keyboard —
    /// a file tree's "attach to agent", say. The box's own typing never comes
    /// this way, and written to the pty it would land in the agent's input
    /// under the box, unseen, and run into the next message.
    pub(super) fn composer_takes_typing(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if !matches!(self.presence(), Presence::Covering(_)) {
            return false;
        }
        let focused = self.composer_state.focused;
        let Some(c) = self.composer_state.active.as_mut() else {
            return false;
        };
        c.typed.push_str(text);
        c.grab |= !focused;
        cx.notify();
        true
    }

    /// Whether files pasted now become the composer's attachments: it has
    /// the keyboard, or covers the input they would be pasted into.
    pub(super) fn composer_takes_files(&self) -> bool {
        let covering = matches!(self.presence(), Presence::Covering(_));
        self.composer_state.focused && self.composer_shown() || covering
    }

    /// Files arriving by way of the terminal's paste path — dropped on the
    /// box, a copied file or a screenshot pasted into it, an upload to a
    /// remote pane landing — become attachments while the box has the
    /// keyboard, or covers the input they would otherwise be pasted into
    /// (taking the keyboard with them, as typing there does). Hands them
    /// back otherwise, for the terminal to paste.
    pub(super) fn composer_takes_paths(
        &mut self,
        spelled: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<Vec<String>> {
        if !self.composer_takes_files() {
            return Some(spelled);
        }
        let focused = self.composer_state.focused;
        let c = self.composer_state.active.as_mut()?;
        c.grab |= !focused;
        for path in spelled {
            if !c.attached.contains(&path) {
                c.attached.push(path);
            }
        }
        self.remember_composer(cx);
        cx.notify();
        None
    }

    fn detach(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.composer_state.active.as_mut()
            && index < c.attached.len()
        {
            c.attached.remove(index);
        }
        self.remember_composer(cx);
        cx.notify();
    }

    /// The attach button: pick files on this computer, which then go the way
    /// dropped files do — uploaded first for a remote pane.
    fn pick_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_composer(window, cx);
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |view, window, cx| {
                    view.focus_composer(window, cx);
                    view.paste_local_paths(paths, cx);
                });
            }
        })
        .detach();
    }

    /// Send one of the agent's own keys from the toolbar, keeping the caret in
    /// the box.
    fn toolbar_key(&mut self, bytes: &'static [u8], window: &mut Window, cx: &mut Context<Self>) {
        self.send_to_pty(bytes, cx);
        self.focus_composer(window, cx);
    }

    fn toggle_picker(&mut self, picker: Picker, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = self.composer_state.active.as_mut() {
            c.picker = (c.picker != Some(picker)).then_some(picker);
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Pick from a toolbar list the way the agent takes it: its `/model` or
    /// `/effort` command, sent like any message.
    fn pick(
        &mut self,
        picker: Picker,
        value: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = self.agent() else {
            return;
        };
        let bracketed = self
            .terminal
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        let turns = self.agent_session().map_or(0, |s| s.turns);
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        let command = match picker {
            Picker::Model => {
                c.model = Some((value, turns));
                format!("/model {value}")
            }
            Picker::Effort => {
                c.effort = Some((value, turns));
                format!("/effort {value}")
            }
        };
        c.picker = None;
        c.queue.extend(submit_plan(agent, &command, bracketed));
        self.pump_composer(agent, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Per-frame upkeep: restore a box the pane had open before this view
    /// existed, find the agent's input area, decide where the box goes, and
    /// move the keyboard with it.
    pub(super) fn sync_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent = self.agent();
        if self.composer_state.active.is_none()
            && agent.is_some()
            && cx
                .try_global::<ComposerMemory>()
                .is_some_and(|m| m.0.contains_key(&self.composer_key()))
        {
            self.ensure_composer(window, cx);
        }
        // A pane whose box was never asked for, or is shut, has no input area
        // to look for: the scan below reads the whole screen, every frame.
        let Some(open) = self.composer_state.active.as_ref().map(|c| c.open) else {
            return;
        };
        let asking = self.agent_is_asking();
        let session = agent
            .and_then(|_| self.agent_session())
            .and_then(|s| s.session_id);
        let (found, backdrop, screen_lines, offset) = if !open {
            if let Some(c) = self.composer_state.active.as_mut() {
                c.last_scanned_rev = 0;
                c.cached_found = None;
                c.cached_backdrop = None;
                c.recheck_task = None;
            }
            (None, None, 0, 0)
        } else {
            let rev = self.terminal.grid_revision();
            let offset = self.terminal.display_offset_nonblocking();
            let size = self.terminal.size();
            let geom_sig = (size.cols, size.rows, offset);
            if let Some(c) = self.composer_state.active.as_ref().filter(|c| {
                c.last_scanned_rev != 0
                    && c.last_scanned_rev == rev
                    && c.last_scanned_geom == geom_sig
            }) {
                (
                    c.cached_found.clone(),
                    c.cached_backdrop,
                    c.cached_screen_lines,
                    offset,
                )
            } else if let Some(term) = self.terminal.term.try_lock_unfair() {
                let mut palette = self.terminal.palette;
                if let Some(active) =
                    cx.try_global::<crate::render::palette::ActivePalette>()
                {
                    palette[..16].copy_from_slice(&active.ansi16);
                }
                let mut scratch = self
                    .composer_state
                    .active
                    .as_mut()
                    .map(|c| std::mem::take(&mut c.scratch_rows))
                    .unwrap_or_default();
                let found = agent.filter(|a| covers(*a)).and_then(|a| {
                    fill_screen_rows(&term, &mut scratch);
                    input_area(a, &scratch, term.columns())
                });
                let backdrop = found
                    .as_ref()
                    .and_then(|area| row_backdrop(&term, area.top.saturating_sub(1), &palette))
                    .map(|rgb| (rgb, self.render.cell_width * term.columns() as f32));
                let screen_lines = term.screen_lines();
                let live_offset = term.grid().display_offset();
                let live_geom = (term.columns(), screen_lines, live_offset);
                drop(term);
                if let Some(c) = self.composer_state.active.as_mut() {
                    c.scratch_rows = scratch;
                    c.last_scanned_rev = rev;
                    c.last_scanned_geom = live_geom;
                    c.cached_found = found.clone();
                    c.cached_backdrop = backdrop;
                    c.cached_screen_lines = screen_lines;
                }
                (found, backdrop, screen_lines, live_offset)
            } else {
                // Reader thread holds the lock mid-batch: keep the previous
                // scan result rather than stalling the UI thread.
                let Some(c) = self.composer_state.active.as_ref() else {
                    return;
                };
                (
                    c.cached_found.clone(),
                    c.cached_backdrop,
                    c.cached_screen_lines,
                    offset,
                )
            }
        };
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        if c.session != session {
            c.session = session;
            c.model = None;
            c.effort = None;
        }

        // The area, with a grace period before it counts as gone.
        let now = Instant::now();
        let mut recheck = None;
        match found {
            // Shut, nothing was looked for: what was found before is stale.
            _ if !open => {
                c.area = None;
                c.missing_since = None;
                c.recheck_task = None;
            }
            Some(area) => {
                c.area = Some(area);
                c.backdrop = backdrop;
                c.missing_since = None;
                c.recheck_task = None;
            }
            None => {
                let since = *c.missing_since.get_or_insert(now);
                if now.duration_since(since) >= STEP_ASIDE_AFTER {
                    c.area = None;
                    c.recheck_task = None;
                } else {
                    recheck = Some(STEP_ASIDE_AFTER - now.duration_since(since));
                }
            }
        }

        c.presence = match agent {
            _ if !c.open => Presence::Hidden,
            None => Presence::Hidden,
            Some(a) if !covers(a) => Presence::Docked,
            Some(_) if asking => Presence::SteppedAside,
            Some(_) => match &c.area {
                Some(area) => {
                    c.area_rows = screen_lines.saturating_sub(area.top);
                    // When the user scrolls up past the agent's input box to
                    // read earlier output, step the floating composer aside so
                    // it does not cover the bottom rows of historical output.
                    if offset > 0 && offset >= c.area_rows {
                        Presence::SteppedAside
                    } else {
                        Presence::Covering(c.area_rows.saturating_sub(offset))
                    }
                }
                None => Presence::SteppedAside,
            },
        };

        if let Some(agent) = agent
            && c.named != Some(agent)
        {
            c.named = Some(agent);
            let key = match builtin_commands(agent).is_empty() {
                true => L10nKey::ComposerPlaceholderFiles,
                false => L10nKey::ComposerPlaceholder,
            };
            let placeholder = t_fmt(key, &[("agent", agent.display_name())]);
            c.input.update(cx, |state, cx| {
                state.set_placeholder(placeholder, window, cx)
            });
        }

        let shown = matches!(c.presence, Presence::Docked | Presence::Covering(_));
        if !shown && self.composer_state.focused {
            c.refocus = c.presence == Presence::SteppedAside;
            self.composer_state.focused = false;
            window.focus(&self.focus_handle, cx);
        } else if shown {
            let take_focus = (c.refocus || c.grab) && !self.composer_state.focused;
            if take_focus {
                c.refocus = false;
                c.grab = false;
            }
            let typed = std::mem::take(&mut c.typed);
            let input = c.input.clone();
            if !typed.is_empty() {
                input.update(cx, |state, cx| state.insert(typed, window, cx));
            }
            if take_focus {
                self.focus_composer(window, cx);
            }
        }

        if let Some(wait) = recheck {
            if self.composer_state.active.as_ref().is_some_and(|c| c.recheck_task.is_none()) {
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |view, cx| {
                        if let Some(c) = view.composer_state.active.as_mut() {
                            c.recheck_task = None;
                        }
                        cx.notify();
                    });
                });
                if let Some(c) = self.composer_state.active.as_mut() {
                    c.recheck_task = Some(task);
                }
            }
        }
    }

    fn on_composer_event(
        &mut self,
        _input: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                if let Some(c) = self.composer_state.active.as_mut() {
                    c.highlighted = 0;
                }
                self.remember_composer(cx);
                self.warm_menu_sources(cx);
            }
            InputEvent::PressEnter { shift: false, .. } => self.submit_composer(window, cx),
            InputEvent::PressEnter { .. } => {}
            InputEvent::Focus => {
                self.composer_state.focused = true;
                cx.notify();
            }
            InputEvent::Blur => {
                self.composer_state.focused = false;
                cx.notify();
            }
        }
    }

    fn composer_trigger(&self, cx: &gpui::App) -> Option<Trigger> {
        let c = self.composer_state.active.as_ref()?;
        let state = c.input.read(cx);
        let text = state.value();
        if c.dismissed.as_deref() == Some(text.as_ref()) {
            return None;
        }
        trigger_at(&text, state.cursor())
    }

    /// Start what the menu about to open needs: the file walk for `@`, the
    /// custom command list for `/`. Both are kept a while, so typing through
    /// a query does not walk the project once per keystroke.
    fn warm_menu_sources(&mut self, cx: &mut Context<Self>) {
        let Some(trigger) = self.composer_trigger(cx) else {
            return;
        };
        let host_id = self.host_id();
        let local = host_id.is_local();
        let cwd = self.files_cwd();
        let agent = self.agent();
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        match trigger.sigil {
            '/' => {
                if c.commands
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() < SOURCES_FRESH_FOR)
                {
                    return;
                }
                let host = local
                    .then(|| anterm_ui_kit::host_registry::HostRegistry::lookup(cx, host_id))
                    .flatten();
                let custom = match (host, agent) {
                    (Some(host), Some(CLIAgent::Claude)) => {
                        let home = std::env::var_os("HOME").map(PathBuf::from);
                        custom_commands(&*host, cwd.as_deref(), home.as_deref())
                    }
                    _ => Vec::new(),
                };
                c.commands = Some((custom, Instant::now()));
            }
            '@' => {
                let stale = match &c.files {
                    Files::Unwalked => true,
                    Files::Walking => false,
                    Files::Ready(_, at) | Files::Failed(at) => at.elapsed() > SOURCES_FRESH_FOR,
                };
                let Some(cwd) = cwd.filter(|_| stale) else {
                    return;
                };
                let Some(host) = anterm_ui_kit::host_registry::HostRegistry::lookup(cx, host_id) else {
                    return;
                };
                if !matches!(c.files, Files::Ready(..)) {
                    c.files = Files::Walking;
                }
                let home = local
                    .then(|| std::env::var_os("HOME").map(PathBuf::from))
                    .flatten();
                HostOps::run(
                    host,
                    cx,
                    move |h| walk(h, &[cwd], home.as_deref()),
                    |view: &mut TerminalView, list, cx| {
                        let Some(c) = view.composer_state.active.as_mut() else {
                            return;
                        };
                        c.files = match list {
                            FileList::Ready(index) => Files::Ready(index, Instant::now()),
                            _ => Files::Failed(Instant::now()),
                        };
                        cx.notify();
                    },
                );
            }
            _ => {}
        }
    }

    /// The `/` or `@` menu as it stands at the caret, if one is open.
    fn composer_menu(&self, cx: &gpui::App) -> Option<(Trigger, Vec<MenuItem>)> {
        let trigger = self.composer_trigger(cx)?;
        let c = self.composer_state.active.as_ref()?;
        let items = match trigger.sigil {
            '/' => {
                let agent = self.agent()?;
                let mut all: Vec<(String, String)> = builtin_commands(agent)
                    .iter()
                    .map(|(n, d)| (n.to_string(), d.to_string()))
                    .collect();
                if let Some((custom, _)) = &c.commands {
                    for (name, detail) in custom {
                        if !all.iter().any(|(n, _)| n == name) {
                            all.push((name.clone(), detail.clone()));
                        }
                    }
                }
                command_items(&all, &trigger.query)
            }
            _ => match &c.files {
                Files::Ready(index, _) => file_items(
                    index,
                    &trigger.query,
                    self.files_cwd().as_deref(),
                    self.agent(),
                    self.shell_program().as_deref(),
                ),
                _ => Vec::new(),
            },
        };
        (!items.is_empty()).then_some((trigger, items))
    }

    /// Replace the word being typed with the picked row, and a space after it
    /// so the next word starts clean.
    fn pick_menu_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((trigger, items)) = self.composer_menu(cx) else {
            return;
        };
        let Some(item) = items.get(index) else {
            return;
        };
        let Some(c) = self.composer_state.active.as_ref() else {
            return;
        };
        let input = c.input.clone();
        input.update(cx, |state, cx| {
            let text = state.value().to_string();
            let cursor = state.cursor().min(text.len());
            let inserted = format!("{} ", item.insert);
            let next = format!("{}{inserted}{}", &text[..trigger.start], &text[cursor..]);
            let caret = trigger.start + inserted.len();
            state.set_value(next, window, cx);
            let position = state.text().offset_to_position(caret);
            state.set_cursor_position(position, window, cx);
        });
    }

    fn step_menu(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some((_, items)) = self.composer_menu(cx) else {
            return false;
        };
        let Some(c) = self.composer_state.active.as_mut() else {
            return false;
        };
        let n = items.len();
        let at = c.highlighted.min(n - 1);
        c.highlighted = match forward {
            true => (at + 1) % n,
            false => (at + n - 1) % n,
        };
        cx.notify();
        true
    }

    /// The agent is asking a question only its TUI can put — a permission
    /// prompt, a choice. Whatever the box sent would be taken as the answer,
    /// so nothing is sent until the question is gone.
    ///
    /// Only a hook can say so. An agent without them — Amp — is marked
    /// waiting by any desktop notification it raises, "Agent is ready" when
    /// a turn ends included, and stays so; read as a question, that kept the
    /// box aside, with nothing sendable, from the first finished turn on.
    /// Its own dialogs it puts in its input's place, and the box steps aside
    /// for those off the screen.
    fn agent_is_asking(&self) -> bool {
        self.agent_session()
            .is_some_and(|s| s.rich && s.status == AgentStatus::Waiting)
    }

    fn composer_can_send(&self, cx: &gpui::App) -> bool {
        self.composer_state.active
            .as_ref()
            .is_some_and(|c| !c.attached.is_empty() || !c.input.read(cx).value().trim().is_empty())
            && !self.agent_is_asking()
    }

    pub(super) fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.agent() else {
            return;
        };
        if !self.composer_can_send(cx) {
            return;
        }
        let shell = self.shell_program();
        // Where OpenCode's mentions are looked up, to be picked from its list.
        let mentions_from = match agent == CLIAgent::OpenCode && self.host_id().is_local() {
            true => self.files_cwd(),
            false => None,
        };
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        let bracketed = self
            .terminal
            .mode_nonblocking()
            .contains(TermMode::BRACKETED_PASTE);
        let (held, shell_mode) = match c.presence {
            Presence::Covering(_) => {
                let term = self.terminal.term.lock();
                fill_screen_rows(&term, &mut c.scratch_rows);
                (
                    held_lines(agent, &c.scratch_rows, term.columns()),
                    in_shell_mode(agent, &c.scratch_rows, term.columns()),
                )
            }
            _ => (0, false),
        };
        let typed = c.input.read(cx).value().to_string();
        // Already in its shell mode, the `!` would take the agent out of it.
        let message = match shell_mode {
            true => typed.strip_prefix('!').unwrap_or(&typed),
            false => &typed,
        };
        let picked = mentions_from.as_deref().and_then(|cwd| {
            let exists = |p: &str| Path::new(p).is_relative() && cwd.join(p).exists();
            opencode_mention_plan(&mention_pieces(message, exists), &c.attached, bracketed)
        });
        let mut steps = match picked {
            Some(steps) => steps,
            None => submit_message(agent, message, &c.attached, shell.as_deref(), bracketed),
        };
        let mut before = Vec::new();
        // Text left in the agent's own input under the box would run into the
        // message: clear it first.
        if let Some(clear) = clear_input(agent, held) {
            before.extend(clear);
        }
        // The shell mode a `!` message left on would take this one for a
        // command: leave it first.
        if shell_mode && !typed.starts_with('!') {
            before.push(Step {
                delay: match before.is_empty() {
                    true => Duration::ZERO,
                    false => SETTLE,
                },
                bytes: b"\x1b".to_vec(),
            });
        }
        if !before.is_empty()
            && let Some(first) = steps.first_mut()
        {
            first.delay = first.delay.max(SETTLE);
            steps.splice(0..0, before);
        }
        c.attached.clear();
        c.dismissed = None;
        c.helix = anterm_editor::helix::HelixState::new(anterm_editor::helix::HelixMode::Insert);
        c.queue.extend(steps);
        c.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.remember_composer(cx);
        self.pump_composer(agent, cx);
    }

    fn composer_sync_helix_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.composer_state.active.as_ref() else {
            return;
        };
        let anchor = c.helix.anchor;
        let head = c.helix.head;
        let mode = c.helix.mode;
        let input = c.input.clone();
        input.update(cx, |state, cx| {
            let text_len = state.value().len();
            let a = anchor.min(text_len);
            let h = head.min(text_len);
            let pos = state.text().offset_to_position(h);
            state.set_cursor_position(pos, window, cx);
            let primary = a.min(h)..a.max(h);
            if mode == anterm_editor::helix::HelixMode::Insert && primary.is_empty() {
                return;
            }
            state.set_selected_range(primary, cx);
        });
    }

    fn composer_handle_helix_key(
        &mut self,
        ks: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use anterm_editor::helix::{HelixOutcome, HostAction, SurfaceKind};
        use gpui::EntityInputHandler as _;

        let Some(c) = self.composer_state.active.as_mut() else {
            return false;
        };
        let (text, sel, cursor) = {
            let state = c.input.read(cx);
            (
                state.value().to_string(),
                state.selected_range(),
                state.cursor(),
            )
        };
        if c.helix.anchor == 0 && c.helix.head == 0 && cursor > 0 {
            c.helix
                .set_selection(sel.start.min(text.len()), sel.end.min(text.len()));
        }
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let outcome = c.helix.handle_keystroke(
            anterm_ui_kit::helix_bridge::key_input(ks),
            &text,
            SurfaceKind::Composer,
            clipboard.as_deref(),
        );

        match outcome {
            HelixOutcome::Ignored => false,
            HelixOutcome::Handled => {
                self.composer_sync_helix_selection(window, cx);
                cx.notify();
                true
            }
            HelixOutcome::Yank { clipboard } => {
                if let Some(yanked) = clipboard {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(yanked));
                }
                self.composer_sync_helix_selection(window, cx);
                cx.notify();
                true
            }
            HelixOutcome::Edit {
                range,
                new_text,
                new_anchor,
                new_head,
                yank_to_clipboard,
            } => {
                if let Some(yanked) = yank_to_clipboard {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(yanked));
                }
                if let Some(c) = self.composer_state.active.as_ref() {
                    let input = c.input.clone();
                    input.update(cx, |state, cx| {
                        let current = state.value().to_string();
                        let start = range.start.min(current.len());
                        let end = range.end.min(current.len()).max(start);
                        let start_u16 = state.text().offset_to_offset_utf16(start);
                        let end_u16 = state.text().offset_to_offset_utf16(end);
                        state.replace_text_in_range(
                            Some(start_u16..end_u16),
                            &new_text,
                            window,
                            cx,
                        );
                    });
                }
                if let Some(c) = self.composer_state.active.as_mut() {
                    c.helix.set_selection(new_anchor, new_head);
                }
                self.composer_sync_helix_selection(window, cx);
                cx.notify();
                true
            }
            HelixOutcome::Host(action) => match action {
                HostAction::Submit => {
                    self.submit_composer(window, cx);
                    true
                }
                HostAction::DismissSurface => {
                    self.composer_escape(window, cx);
                    true
                }
                HostAction::Undo => {
                    window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
                    if let Some(c) = self.composer_state.active.as_mut() {
                        let pos = c.input.read(cx).cursor();
                        c.helix.set_selection(pos, pos);
                    }
                    cx.notify();
                    true
                }
                HostAction::Redo => {
                    window.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx);
                    if let Some(c) = self.composer_state.active.as_mut() {
                        let pos = c.input.read(cx).cursor();
                        c.helix.set_selection(pos, pos);
                    }
                    cx.notify();
                    true
                }
                _ => {
                    self.composer_sync_helix_selection(window, cx);
                    cx.notify();
                    true
                }
            },
        }
    }

    /// Write queued steps one at a time, each after its own delay.
    ///
    /// Every write after a wait checks that `agent` is still what the pane is
    /// running: an agent that quit inside the wait has handed the pty back to
    /// the shell, and the rest of the message — its Enter above all — would
    /// run there as a command.
    fn pump_composer(&mut self, agent: CLIAgent, cx: &mut Context<Self>) {
        let Some(c) = self.composer_state.active.as_mut() else {
            return;
        };
        if c.pumping {
            return;
        }
        c.pumping = true;
        cx.spawn(async move |this, cx| {
            loop {
                // Popping the last step and standing the pump down are one
                // update, so a submission can never find it still "pumping"
                // after it has stopped looking at the queue.
                let next = this
                    .update(cx, |view, _| {
                        let c = view.composer_state.active.as_mut()?;
                        let step = c.queue.pop_front();
                        c.pumping = step.is_some();
                        step
                    })
                    .ok()
                    .flatten();
                let Some(step) = next else { return };
                if !step.delay.is_zero() {
                    cx.background_executor().timer(step.delay).await;
                }
                let sent = this.update(cx, |view, cx| {
                    if view.agent() != Some(agent) {
                        if let Some(c) = view.composer_state.active.as_mut() {
                            c.queue.clear();
                        }
                        return;
                    }
                    view.send_to_pty(&step.bytes, cx);
                });
                if sent.is_err() {
                    return;
                }
            }
        })
        .detach();
    }
}
