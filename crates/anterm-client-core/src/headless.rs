//! Framework-free headless terminal session wrapping `alacritty_terminal::Term`.
//!
//! Consumes `DaemonMsg` frames (supporting `ObserveSince` delta replay via
//! `ReplayCursor` + monotonic `consumed_offset`), maintains atomic mirror state,
//! emits flat 16-byte/cell binary frames for mobile GPU renderers, and supports
//! zero-PC-impact client-side reflow.

use std::path::PathBuf;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use anterm_protocol::cli_agent::{AgentSessionState, AgentStatus, CLIAgent};
use anterm_protocol::pane::{DaemonMsg, ReplayMode, WinSize};

use crate::frame::{PaletteRgba, pack_term_frame};
use crate::mirror::TermStateMirror;
use crate::predictive_echo::PredictiveEcho;
use crate::reflow::{ReflowedLine, reflow_terminal};

struct SimpleSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for SimpleSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Summary of what changed after applying a [`DaemonMsg`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalEventSummary {
    pub grid_changed: bool,
    pub meta_changed: bool,
    pub exited: Option<Option<i32>>,
}

/// Complete framework-free terminal state for one observed/attached pane.
pub struct HeadlessTerminal {
    term: Term<VoidListener>,
    processor: Processor,
    mirror: TermStateMirror,
    palette: PaletteRgba,
    echo: PredictiveEcho,
    consumed_offset: u64,
    cwd: Option<PathBuf>,
    agent: Option<CLIAgent>,
    agent_session: Option<AgentSessionState>,
    leased_by: Option<String>,
    at_prompt: bool,
    shell_active: bool,
    last_exit: Option<i32>,
    exited: bool,
    exit_code: Option<i32>,
    needs_full_redraw: bool,
}

impl HeadlessTerminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let size = SimpleSize {
            cols: cols.max(2),
            rows: rows.max(1),
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let term = Term::new(config, &size, VoidListener);
        let mirror = TermStateMirror::from_term(&term);
        Self {
            term,
            processor: Processor::new(),
            mirror,
            palette: PaletteRgba::default(),
            echo: PredictiveEcho::new(),
            consumed_offset: 0,
            cwd: None,
            agent: None,
            agent_session: None,
            leased_by: None,
            at_prompt: false,
            shell_active: false,
            last_exit: None,
            exited: false,
            exit_code: None,
            needs_full_redraw: true,
        }
    }

    pub fn set_palette(&mut self, palette: PaletteRgba) {
        self.palette = palette;
        self.needs_full_redraw = true;
    }

    pub fn consumed_offset(&self) -> u64 {
        self.consumed_offset
    }

    pub fn mirror(&self) -> &TermStateMirror {
        &self.mirror
    }

    pub fn cwd(&self) -> Option<&PathBuf> {
        self.cwd.as_ref()
    }

    pub fn agent(&self) -> Option<CLIAgent> {
        self.agent
    }

    pub fn agent_session(&self) -> Option<&AgentSessionState> {
        self.agent_session.as_ref()
    }

    pub fn leased_by(&self) -> Option<&str> {
        self.leased_by.as_deref()
    }

    pub fn at_prompt(&self) -> bool {
        self.at_prompt
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn is_agent_waiting(&self) -> bool {
        self.agent_session
            .as_ref()
            .is_some_and(|s| s.status == AgentStatus::Waiting)
    }

    /// Notify predictive echo of locally typed bytes before they hit the network.
    pub fn observe_local_input(&mut self, bytes: &[u8]) {
        let eligible = self.shell_active && self.at_prompt && !self.alt_screen();
        self.echo.observe_input(bytes, eligible);
    }

    /// Apply one incoming [`DaemonMsg`] to the local VT emulator and metadata.
    pub fn apply_daemon_msg(&mut self, msg: &DaemonMsg) -> TerminalEventSummary {
        let mut summary = TerminalEventSummary::default();
        match msg {
            DaemonMsg::ReplayCursor { offset, mode } => {
                if *mode == ReplayMode::Full {
                    self.reset_grid();
                    summary.grid_changed = true;
                }
                self.consumed_offset = *offset;
                self.needs_full_redraw = true;
            }
            DaemonMsg::Size(ws) => {
                self.resize_local(*ws);
                self.needs_full_redraw = true;
                summary.grid_changed = true;
            }
            DaemonMsg::Snapshot(bytes) => {
                if !bytes.is_empty() {
                    self.processor.advance(&mut self.term, bytes);
                    self.mirror.publish(&self.term);
                    self.needs_full_redraw = true;
                    summary.grid_changed = true;
                }
            }
            DaemonMsg::Output(bytes) => {
                if !bytes.is_empty() {
                    self.consumed_offset = self.consumed_offset.saturating_add(bytes.len() as u64);
                    self.echo.reconcile_output(bytes);
                    self.processor.advance(&mut self.term, bytes);
                    self.mirror.publish(&self.term);
                    summary.grid_changed = true;
                }
            }
            DaemonMsg::Cwd(path) => {
                self.cwd = Some(path.clone());
                summary.meta_changed = true;
            }
            DaemonMsg::Prompt {
                active,
                at_prompt,
                last_exit,
            } => {
                self.shell_active = *active;
                self.at_prompt = *at_prompt;
                self.last_exit = *last_exit;
                if !*at_prompt {
                    self.echo.clear();
                }
                summary.meta_changed = true;
            }
            DaemonMsg::Agent(agent) => {
                self.agent = *agent;
                summary.meta_changed = true;
            }
            DaemonMsg::AgentStatus(state) => {
                self.agent_session = state.clone();
                summary.meta_changed = true;
            }
            DaemonMsg::Lease(by) => {
                self.leased_by = by.clone();
                summary.meta_changed = true;
            }
            DaemonMsg::Exited { code } => {
                self.exited = true;
                self.exit_code = *code;
                self.echo.clear();
                summary.meta_changed = true;
                summary.exited = Some(*code);
            }
            _ => {}
        }
        summary
    }

    /// Pack the visible terminal grid into a compact 16-byte/cell binary buffer
    /// ready for direct Metal / Vulkan upload.
    pub fn pack_frame(&mut self) -> Vec<u8> {
        let full = std::mem::take(&mut self.needs_full_redraw);
        pack_term_frame(
            &self.term,
            &self.palette,
            self.mirror.grid_revision(),
            full,
            self.echo.overlay(),
        )
    }

    /// Produce zero-PC-impact locally reflowed lines for mobile portrait reading.
    pub fn reflow_lines(&self, target_cols: usize, max_logical_lines: usize) -> Vec<ReflowedLine> {
        reflow_terminal(&self.term, target_cols, max_logical_lines)
    }

    /// Scroll local viewport by `delta` lines (positive = up into history).
    pub fn scroll_display(&mut self, delta: i32) {
        self.term
            .scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        self.mirror.publish(&self.term);
        self.needs_full_redraw = true;
    }

    /// Return the visible screen lines as plain strings (for previews/cards).
    pub fn visible_lines(&self) -> Vec<String> {
        let cols = self.term.columns();
        let rows = self.term.screen_lines();
        let grid = self.term.grid();
        let display_offset = grid.display_offset() as i32;
        let mut out = Vec::with_capacity(rows);
        for screen_y in 0..rows {
            let line = Line(screen_y as i32 - display_offset);
            let mut row = String::with_capacity(cols);
            for col in 0..cols {
                let cell = &grid[Point::new(line, Column(col))];
                if !cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) {
                    row.push(cell.c);
                }
            }
            out.push(row.trim_end().to_string());
        }
        out
    }

    fn resize_local(&mut self, ws: WinSize) {
        let size = SimpleSize {
            cols: usize::from(ws.cols).max(2),
            rows: usize::from(ws.rows).max(1),
        };
        if self.term.columns() != size.cols || self.term.screen_lines() != size.rows {
            self.term.resize(size);
            self.mirror.publish(&self.term);
        }
    }

    fn reset_grid(&mut self) {
        let size = SimpleSize {
            cols: self.term.columns(),
            rows: self.term.screen_lines(),
        };
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        self.term = Term::new(config, &size, VoidListener);
        self.processor = Processor::new();
        self.echo.clear();
        self.mirror.publish(&self.term);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_terminal_tracks_delta_cursor_and_packs_frames() {
        let mut term = HeadlessTerminal::new(80, 24);
        term.apply_daemon_msg(&DaemonMsg::ReplayCursor {
            offset: 12,
            mode: ReplayMode::Full,
        });
        term.apply_daemon_msg(&DaemonMsg::Snapshot(b"hello world!".to_vec()));
        assert_eq!(term.consumed_offset(), 12);

        term.apply_daemon_msg(&DaemonMsg::Output(b"\r\nnext".to_vec()));
        assert_eq!(term.consumed_offset(), 18);

        let lines = term.visible_lines();
        assert_eq!(lines[0], "hello world!");
        assert_eq!(lines[1], "next");

        // Reconnect with Delta keeps previous lines and advances cursor.
        term.apply_daemon_msg(&DaemonMsg::ReplayCursor {
            offset: 22,
            mode: ReplayMode::Delta,
        });
        term.apply_daemon_msg(&DaemonMsg::Snapshot(b"1234".to_vec()));
        assert_eq!(term.consumed_offset(), 22);
        assert_eq!(term.visible_lines()[1], "next1234");

        let frame = term.pack_frame();
        assert!(frame.len() > crate::frame::FRAME_HEADER_BYTES);
    }
}
