//! Cohesive internal state sub-structs owned by [`super::TerminalView`].
//!
//! Grouping `TerminalView`'s fields by lifecycle and subsystem keeps the top-level
//! view struct compact (`≤ 20` fields) with zero extra heap indirection or
//! runtime overhead on hot PTY paint paths.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use gpui::{Bounds, Font, MouseButton, Pixels, SharedString, Task};

use anterm_core::cli_agent::{AgentStatus, CLIAgent};
use anterm_core::daemon::protocol::{NativeSshSpec, ShellSpec};
use crate::backend::hold::GapHold;
use crate::backend::typeahead::Typeahead;
use crate::composer::Composer;
use crate::element::{CellGeom, GridSnapshot, RenderCell, RowCache};
use crate::links::link_probe::LinkProbeCache;
use crate::prompt::cmd_editor::CmdEditor;
use crate::prompt::completion::CompletionSession;
use crate::prompt::history;
use crate::prompt::reverse_search::ReverseSearch;
use crate::render::scrollbar::TerminalScrollHandle;
use crate::search::{LinkTarget, SearchState};
use anterm_ui_kit::host_ops::HostId;

use super::input::{DragScroll, ScrollAnim};
use super::links::HoveredLink;
use super::prompt::{GhostCache, HistoryNav, LastWordWalk, PendingHistory};

/// Pane identity, remote/SSH metadata, title settlement, and remote clipboard staging.
pub struct TerminalSessionState {
    pub(super) host_id: HostId,
    pub(super) shell_spec: Option<ShellSpec>,
    pub(super) restored: bool,
    pub(super) ssh_spec: Option<Box<NativeSshSpec>>,
    pub(super) remote_clipboard_dir: Option<String>,
    pub(super) remote_clipboard_write_in_flight: Option<u64>,
    pub(super) remote_clipboard_write_generation: u64,
    pub(super) displayed: Arc<AtomicBool>,
    pub title: String,
    pub(super) pending_title: Option<String>,
    pub(crate) default_title: String,
    pub marked_text: String,
    pub(super) created_at: Instant,
}

/// Fonts, cell geometry, damage-tracked grid caches, cursor blink, bell flash, and dimming.
pub struct TerminalRenderState {
    pub font: Font,
    pub font_bold: Option<Font>,
    pub font_italic: Option<Font>,
    pub(super) font_features: Option<gpui::FontFeatures>,
    pub font_size: Pixels,
    pub line_height_mul: f32,
    pub cell_width: Pixels,
    pub(crate) line_height: Pixels,
    pub(crate) cell_width_cache: Option<(SharedString, Pixels, Pixels)>,
    pub(crate) grid_slack: Pixels,
    pub(crate) grid_buf: Vec<RenderCell>,
    pub(crate) grid_snap: Option<GridSnapshot>,
    pub(crate) row_cache: RowCache,
    pub(super) frame_alt_screen: bool,
    pub(super) frame_has_selection: bool,
    pub cursor_visible: bool,
    pub bell_flash: bool,
    pub(super) bell_epoch: u64,
    pub(crate) cell_geom: Option<CellGeom>,
    pub(super) ime_anchor: Option<Bounds<Pixels>>,
    pub(crate) dim: f32,
}

/// Pointer tracking, selection drags, wheel/trackpad scroll damping, and mouse reporting.
pub struct PointerAndScrollState {
    pub(super) selecting: bool,
    pub(super) drag_scroll: Option<DragScroll>,
    pub(super) drag_scroll_epoch: u64,
    pub(super) scroll_anim: Option<ScrollAnim>,
    pub(super) scroll_anim_epoch: u64,
    pub(super) gesture_until: Option<Instant>,
    pub(super) gesture_zoom: Option<bool>,
    pub(super) last_mouse_cell: Option<(usize, usize)>,
    pub(super) mouse_grab: Option<MouseButton>,
    pub(crate) context_menu_allowed: bool,
    pub(super) scroll_debt: f32,
    pub(super) zoom_debt: f32,
    pub(crate) scroll_frac: f32,
    pub(crate) scroll_handle: TerminalScrollHandle,
    pub report_mouse: bool,
}

/// Hover link detection, remote path probing, and context-menu link latching.
pub(crate) struct LinkProbeState {
    pub(super) last_hover_cell: Option<(usize, usize)>,
    pub(super) link_modifier_down: bool,
    pub(super) link_probes: LinkProbeCache,
    pub(super) link_repo_root: Option<(PathBuf, Option<PathBuf>)>,
    pub(super) link_repo_root_pending: bool,
    pub(super) menu_link: Option<LinkTarget>,
    pub(crate) hovered_link: Option<HoveredLink>,
}

/// In-pane scrollback search session and live rescan debounce state.
#[derive(Default)]
pub(crate) struct TerminalSearchState {
    pub active: Option<SearchState>,
    pub focused: bool,
    pub case_sensitive: bool,
    pub regex: bool,
    pub regex_error: bool,
    pub last_query: String,
    pub scan_epoch: u64,
    pub scan_armed: bool,
}

/// Docked/overlay coding-agent composer state hosted by [`super::TerminalView`].
#[derive(Default)]
pub(crate) struct ComposerHostState {
    pub active: Option<Composer>,
    pub focused: bool,
}

/// Foreground command, coding-agent status detection, turn notifications, and git cwd tracking.
pub(crate) struct AgentSessionTracker {
    pub(super) last_at_prompt: bool,
    pub(super) last_typeahead_blocked: bool,
    pub(super) running_since: Option<Instant>,
    pub(super) running_title: String,
    pub(super) running_agent: Option<CLIAgent>,
    pub(super) seen_agent: Option<CLIAgent>,
    pub(super) agent_detection_armed: bool,
    pub(super) last_agent_status: Option<AgentStatus>,
    pub(super) last_agent_session: (Option<String>, Option<Vec<String>>),
    pub(super) agent_turn_started: Option<Instant>,
    pub(super) pending_finish_notice: Option<Task<()>>,
    pub(super) agent_was_rich: bool,
    pub(super) agent_result_unread: bool,
    pub(super) keep_unread_on_focus: bool,
    pub(super) agent_status_seen: bool,
    pub(super) git_status_cwd: Option<PathBuf>,
    pub(super) last_agent_activity: u64,
}

/// Local prompt editor, typeahead/gap-hold, history navigation, ghost text, completion, and reverse search.
pub(crate) struct PromptSubsystem {
    pub(super) cmd: CmdEditor,
    pub prompt_editor: bool,
    pub helix_mode: bool,
    pub(super) typeahead: Typeahead,
    pub(super) hold: GapHold,
    pub(super) history_scope: history::Scope,
    pub(super) history_session_id: String,
    pub(super) history_frecency: Vec<f64>,
    pub(super) history_frecency_key: (u64, Option<PathBuf>, bool),
    pub(super) history_seed_pending: bool,
    pub(super) history_load_gen: u64,
    pub(super) ranked_cwd: Option<PathBuf>,
    pub(super) history_nav: Option<HistoryNav>,
    pub(super) history_stash: String,
    pub(super) history_prefix: String,
    pub(super) ghost_cache: RefCell<Option<GhostCache>>,
    pub(super) last_word_nav: Option<LastWordWalk>,
    pub(super) pending_history: Option<PendingHistory>,
    pub(super) completion: Option<CompletionSession>,
    pub(super) remote_completion_inflight: bool,
    pub(super) remote_completion_notice: Option<String>,
    pub(super) completion_generation: u64,
    pub(super) editor_handoff: Option<u64>,
    pub(super) editor_handoff_interrupt_seq: Option<u64>,
    pub(super) reverse_search: Option<ReverseSearch>,
    pub(super) integration_notice: Option<String>,
    pub(super) integration_notice_shown: bool,
    pub(super) editor_selecting: bool,
    pub(super) editor_select_gesture: bool,
    pub(super) editor_drag_word: Option<(usize, usize)>,
    pub(super) editor_goal_col: Option<usize>,
}
