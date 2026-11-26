use alacritty_terminal::event::Event as AlacEvent;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::Direction;
use alacritty_terminal::term::TermMode;
use gpui::{
    App, ClipboardItem, Context, EntityId, ExternalPaths, FocusHandle, Focusable, Font,
    MouseButton, MouseDownEvent, Pixels, Window, div, prelude::*, px,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::scroll::Scrollbar;

use super::TermSize;
use crate::prompt::cmd_editor::CmdEditor;
use crate::prompt::completion::CompletionSession;
#[cfg(test)]
use super::element::RenderCell;
use super::element::TerminalElement;
use crate::backend::hold::GapHold;
use super::remote::RemoteTerminal;
use crate::prompt::reverse_search::ReverseSearch;
use crate::prompt::store::HistoryStore;
use crate::render::scrollbar::{GridScroll, TerminalScrollHandle};
use crate::backend::typeahead::Typeahead;
use anterm_ui_kit::actions::{
    CloseActiveTab, CopyLinkPathUnderPointer, ForkAgentSessionDown, ForkAgentSessionLeft,
    ForkAgentSessionRight, ForkAgentSessionUp, NewTab, OpenLinkUnderPointer,
    OpenLinkWithDefaultApp, RevealLinkUnderPointer, SaveAgentLaunchArgs, SendBackTab, SendTab,
    SplitDown, SplitLeft, SplitRight, SplitUp, ToggleMaximizePane,
};
use anterm_ui_kit::config::{BellMode, Config};
use anterm_core::daemon::protocol::{RemoteContext, ShellSpec};
use anterm_i18n::{L10nKey, t, t_fmt};

mod clipboard;
mod copy_unwrap;
mod input;
mod links;
mod prompt;
mod state;
mod sub_views;

#[cfg(test)]
mod gpui_tests;
#[cfg(any(test, feature = "test-support"))]
mod test_support;
#[cfg(test)]
mod tests;

use state::{
    AgentSessionTracker, ComposerHostState, LinkProbeState, PointerAndScrollState, PromptSubsystem,
    TerminalRenderState, TerminalSearchState, TerminalSessionState,
};

pub(super) use clipboard::clipboard_has_paths;
#[cfg(test)]
pub(super) use clipboard::pasted_paths_text;
pub(super) use input::should_show_context_menu;
#[cfg(test)]
pub use links::PortRoute;
pub(super) use links::HoveredLink;
#[cfg(test)]
pub(super) use links::{LoopbackPlan, loopback_plan};

#[cfg(any(test, feature = "test-support"))]
pub use test_support::{
    quiet_reattached_test_pane, quiet_test_pane, quiet_test_ssh_pane, quiet_test_ssh_pane_of,
    quiet_test_ssh_pane_with, test_stream_pair,
};

use clipboard::{
    allows_remote_clipboard_write, clipboard_paths, files_cwd, paste_bytes, ring_system_bell,
    submit_bytes, trim_trailing_spaces, validate_remote_clipboard_image, wsl_share_distro,
    wsl_share_path,
};
use input::focus_report_bytes;
use links::offers_default_app_open;
use prompt::typeahead_boundary;
use sub_views::{
    before_input, input_char_positions, input_overflow_shift, input_overlay_rows, input_start,
    link_menu_label, mac_only, menu_row_with_hint, wrapped_click_index,
};

#[cfg(test)]
use crate::prompt::completion;
#[cfg(test)]
use crate::backend::typeahead::RawInput;
#[cfg(test)]
use clipboard::{
    REMOTE_CLIPBOARD_MODE, REMOTE_CLIPBOARD_PATH, REMOTE_CLIPBOARD_TTL_SECS, local_path_for_pane,
    paste_dir_name, remote_paste_spec, staged_entry_expired, stages_clipboard_image, staging_cache,
    staging_dir_is_safe, write_clipboard_image, wsl_path,
};
#[cfg(test)]
use anterm_ui_kit::config::LinkFileOpen;
#[cfg(test)]
use input::{
    CmdKey, SCROLL_ANIM_FRAME, SelectEndCopy, WheelRoute, drag_scroll_step, encode_mouse,
    scroll_anim_step, select_end_copy, smooth_scroll_step, wheel_route, zoom_scroll_steps,
    zoom_wheel,
};
#[cfg(test)]
use links::{LinkAt, expand_file_command_template, link_path_style, port_route_of, run_file_command};
#[cfg(test)]
use prompt::{HOLD_WINDOW, HistorySeedSource, history_seed_source, observe_typeahead_for_owner};
#[cfg(test)]
use sub_views::{
    COMPLETION_MENU_MAX_W, InputCaretPaint, description_budget, display_width, elide,
    fig_icon_emoji, fig_icon_glyph, highlight_runs, input_caret_paint, input_cells, menu_layout,
};

pub(super) const GRID_PAD_X: f32 = 8.;

pub(super) const GRID_PAD_Y: f32 = 4.;

/// Which panes are on screen right now, readable without touching the entity
/// map. The chrome (tab strip, sidebar, switcher) reads every pane entity
/// while the window draws, so gpui tracks them all and `notify()` from a
/// hidden pane still dirties the window — this flag is the out-of-band answer
/// the output pump consults instead. `AntermApp::render` declares it each frame
/// for its own tabs; a pane nobody has declared yet counts as displayed, so a
/// missed path can only cost extra repaints, never a frozen grid.
///
/// A gpui `Global` rather than a `static`: entity ids are only unique within
/// one `App`, and parallel `#[gpui::test]` apps mint colliding ids — a
/// process-wide map would let one test's declarations flip another's flags.
/// In the shipped binary there is exactly one `App`, so the two are the same.
#[derive(Default)]
struct DisplayedRegistry(
    std::sync::Mutex<
        std::collections::HashMap<EntityId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    >,
);

impl gpui::Global for DisplayedRegistry {}

pub fn declare_displayed(cx: &App, panes: impl IntoIterator<Item = (EntityId, bool)>) {
    // No registry means no pane has ever been built in this app.
    let Some(registry) = cx.try_global::<DisplayedRegistry>() else {
        return;
    };
    let map = registry.0.lock().unwrap_or_else(|e| e.into_inner());
    for (id, on) in panes {
        if let Some(flag) = map.get(&id) {
            flag.store(on, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// What the registry holds for `id`: `None` when the pane never registered
/// (or already released), otherwise the flag the output gate would consult.
#[cfg(any(test, feature = "test-support"))]
pub fn displayed_for_test(cx: &App, id: EntityId) -> Option<bool> {
    cx.try_global::<DisplayedRegistry>()?
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .map(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
}

pub use anterm_ui_kit::actions::terminal_actions::*;

pub struct ChildExited;

impl gpui::EventEmitter<ChildExited> for TerminalView {}

pub struct AuthPromptReady;

impl gpui::EventEmitter<AuthPromptReady> for TerminalView {}

pub struct AgentSessionChanged;

impl gpui::EventEmitter<AgentSessionChanged> for TerminalView {}

/// A coding agent started running in this pane: its foreground went from no
/// agent to this one. What quick launch counts as the agent being used.
pub struct AgentDetected(pub anterm_core::cli_agent::CLIAgent);

impl gpui::EventEmitter<AgentDetected> for TerminalView {}

/// A file link the user clicked, on its way to whoever can show it. The
/// terminal resolves the path — it is the only thing that knows the pane's
/// directory and host — and the app opens it, because the editor and the file
/// tree are its to drive.
pub struct OpenFileRequested {
    pub path: std::path::PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// A directory link asks "where is this?", not "what does it say?" — the
    /// app answers it in the file tree instead of the editor.
    pub is_dir: bool,
}

impl gpui::EventEmitter<OpenFileRequested> for TerminalView {}

/// Global revision counter bumped whenever any pane's workspace-chrome metadata
/// changes (title, cwd, foreground agent, agent status, unread badge, exit).
///
/// Cached chrome views (`TabSidebarHost`, `TabStripHost`) observe this global
/// rather than waking on high-frequency [`TerminalCanvas`] output repaints.
#[derive(Default)]
pub struct ChromeMeta(pub u64);

impl gpui::Global for ChromeMeta {}

pub fn notify_chrome_meta(cx: &mut gpui::App) {
    if !cx.has_global::<ChromeMeta>() {
        cx.set_global(ChromeMeta(0));
    }
    cx.update_global::<ChromeMeta, _>(|meta, _| {
        meta.0 = meta.0.wrapping_add(1);
    });
}

/// High-frequency rendering surface owned by [`TerminalView`].
///
/// Output wakeups, cursor blink ticks, smooth scroll frames, and link-hover
/// transitions notify *only* this inner view so outer chrome subtrees cached
/// with GPUI `.cached(...)` reuse their prepaint/paint output without running
/// `render()` during agent streaming.
pub(crate) struct TerminalCanvas {
    view: gpui::WeakEntity<TerminalView>,
}

impl Render for TerminalCanvas {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.view.upgrade() else {
            return div().size_full().flex_1().into_any_element();
        };
        let scrollbar = view.update(cx, |v, _| {
            v.sync_scrollbar();
            v.render_scrollbar_in_canvas()
        });
        div()
            .size_full()
            .flex_1()
            .relative()
            .child(TerminalElement::new(view))
            .child(scrollbar)
            .into_any_element()
    }
}

#[cfg(test)]
pub mod overlay_render_probe {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Counts {
        search_bar: AtomicUsize,
        composer: AtomicUsize,
    }

    impl gpui::Global for Counts {}

    pub fn reset(cx: &mut gpui::App) {
        if let Some(counts) = cx.try_global::<Counts>() {
            counts.search_bar.store(0, Ordering::SeqCst);
            counts.composer.store(0, Ordering::SeqCst);
        } else {
            cx.set_global(Counts::default());
        }
    }

    pub(super) fn record_search_bar(cx: &gpui::App) {
        if let Some(counts) = cx.try_global::<Counts>() {
            counts.search_bar.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(super) fn record_composer(cx: &gpui::App) {
        if let Some(counts) = cx.try_global::<Counts>() {
            counts.composer.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn search_bar_renders(cx: &gpui::App) -> usize {
        cx.try_global::<Counts>()
            .map_or(0, |c| c.search_bar.load(Ordering::SeqCst))
    }

    pub fn composer_renders(cx: &gpui::App) -> usize {
        cx.try_global::<Counts>()
            .map_or(0, |c| c.composer.load(Ordering::SeqCst))
    }
}

/// Independent overlay entity hosting the in-pane find bar so high-frequency
/// PTY canvas repaints never re-render the search UI.
pub(crate) struct TerminalSearchBar {
    view: gpui::WeakEntity<TerminalView>,
}

impl Render for TerminalSearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        overlay_render_probe::record_search_bar(cx);
        let Some(view) = self.view.upgrade() else {
            return div().into_any_element();
        };
        view.update(cx, |v, v_cx| {
            v.search_state
                .active
                .as_ref()
                .map(|s| v.render_search_bar(s, window, v_cx).into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        })
    }
}

/// Independent overlay entity hosting the coding-agent composer so high-frequency
/// PTY canvas repaints never re-render the composer UI.
pub(crate) struct AgentComposerView {
    view: gpui::WeakEntity<TerminalView>,
    /// Last naturally laid-out `(width, height)` of the composer. Cleared whenever
    /// `TerminalView` notifies (typing, attachments, picker/menu, resize), so
    /// steady PTY streaming frames can mount this entity via `.cached(style)`.
    measured_size: std::rc::Rc<std::cell::Cell<Option<(Pixels, Pixels)>>>,
}

impl Render for AgentComposerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        overlay_render_probe::record_composer(cx);
        let Some(view) = self.view.upgrade() else {
            return div().into_any_element();
        };
        let Some((el, _)) = view.update(cx, |v, v_cx| v.render_composer(window, v_cx)) else {
            return div().into_any_element();
        };
        let measured = self.measured_size.clone();
        div()
            .w_full()
            .on_children_prepainted(move |bounds, _window, _| {
                let Some(b) = bounds.first() else {
                    return;
                };
                let next = (b.size.width, b.size.height);
                if measured.get() != Some(next) {
                    measured.set(Some(next));
                }
            })
            .child(el)
            .into_any_element()
    }
}

pub struct NativeSshParts {
    terminal: RemoteTerminal,
    pane_id: u64,
    persist: Box<anterm_core::daemon::protocol::NativeSshSpec>,
}

/// What a pane is called when nothing running in it has said otherwise.
pub const DEFAULT_TITLE: &str = "anterm";

/// What a pane is *saying* about itself, if anything — the reading behind
/// [`TerminalView::stated_title`], split out so it can be pinned without a
/// live pane.
///
/// Anything but the placeholder counts. That is wider than "arrived over OSC
/// 0/2" on purpose: an SSH pane answers to the host it dialled and a workspace
/// pane to its workspace's name, and those are names anterm gave the pane
/// deliberately (#438) rather than the absence of one. The literal string
/// `anterm` is the only title that says nothing, because it is the app's own
/// name standing in for a pane that has never introduced itself.
pub fn stated_title(title: &str) -> Option<&str> {
    match title.trim() {
        "" => None,
        t if t == DEFAULT_TITLE => None,
        t => Some(t),
    }
}

pub struct ShellParts {
    terminal: RemoteTerminal,
    pub pane_id: u64,
    shell_spec: Option<ShellSpec>,
    pub restored: bool,
    restored_from: Option<u64>,
}

/// What the reader was last shown of each pane's finished agent turn, kept for
/// the life of the app rather than of a view.
///
/// A view is thrown away and built again over the same daemon pane whenever a
/// workspace is switched out and back or a window is reopened from the tray,
/// and a fresh view sees a `Done` agent arrive from nothing — exactly what a
/// turn finishing live looks like. This is how the new view tells the two apart
/// (#870).
#[derive(Default)]
struct AgentReadMarks(std::collections::HashMap<(anterm_ui_kit::host_ops::HostId, u64), AgentReadMark>);

impl gpui::Global for AgentReadMarks {}

#[derive(Clone)]
struct AgentReadMark {
    session: (Option<String>, Option<Vec<String>>),
    /// The status the pane's last view saw. Kept for every status, not only
    /// `Done`, so that a pane with no mark at all is one this app never watched
    /// an agent in — the only case where a reattach's replayed status may be
    /// taken as a baseline (see `poll_agent_status`).
    status: Option<anterm_core::cli_agent::AgentStatus>,
    turns: u64,
    unread: bool,
}

fn cwd_is_on_host(pane_runs_remotely: bool, host_is_local: bool) -> bool {
    match pane_runs_remotely {
        false => host_is_local,
        true => !host_is_local,
    }
}

pub struct TerminalView {
    pub terminal: RemoteTerminal,
    pub pane_id: u64,
    pub focus_handle: FocusHandle,
    /// High-frequency inner canvas entity hosting `TerminalElement` and the
    /// scrollback bar so output bursts do not invalidate outer chrome or overlays.
    canvas: gpui::Entity<TerminalCanvas>,
    /// Independent find-bar overlay entity isolated from high-frequency PTY ticks.
    search_bar: gpui::Entity<TerminalSearchBar>,
    /// Independent agent-composer entity isolated from high-frequency PTY ticks.
    composer_view: gpui::Entity<AgentComposerView>,
    pub session: TerminalSessionState,
    pub render: TerminalRenderState,
    pub pointer: PointerAndScrollState,
    pub(crate) links: LinkProbeState,
    pub(crate) search_state: TerminalSearchState,
    pub(crate) composer_state: ComposerHostState,
    pub(crate) agent: AgentSessionTracker,
    pub(crate) prompt: PromptSubsystem,
    _focus_subs: Vec<gpui::Subscription>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const INTEGRATION_GRACE: std::time::Duration = std::time::Duration::from_secs(8);

const INTEGRATION_NOTICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

const OPPORTUNISTIC_GIT_GAP: std::time::Duration = std::time::Duration::from_millis(1500);

/// How long a pane with an unknown or negative repository answer waits
/// before asking again. Discovery can miss temporarily unavailable metadata;
/// retry without issuing a request on every poll tick.
const GIT_RETRY_GAP: std::time::Duration = std::time::Duration::from_secs(10);

/// How long a title the program set has to stand before the tab adopts it.
///
/// Long enough that a command which is over almost as soon as it started never
/// reaches the label, short enough that one still running is named while the
/// wait for it is still what the reader is doing.
const TITLE_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// How long a turn has to stay finished before it is announced. Long enough
/// for a queued message or a `Stop` hook to start the next turn, short enough
/// that the notification still reads as the moment the agent stopped.
const AGENT_DONE_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// What a pane does with a title the program just set — see
/// `TerminalView::set_title_when_settled`.
#[derive(Debug, PartialEq, Eq)]
enum TitleSettle {
    /// The tab already reads this, so whatever was waiting to replace it never
    /// has to happen. This is the case a short command lands in, and the one an
    /// agent's spinner lands in: a frame that only changes the status mark in
    /// front of the title reads the same on the tab, so it asks for no repaint.
    Revert,
    /// Hold it: a wait is already running and adopts the newest title when it
    /// elapses. Restarting the wait instead would let a program that rewrites
    /// its title faster than the wait — a download reporting progress — put
    /// off the tab's next update forever.
    Queue,
    /// Hold it and start the wait.
    QueueAndWait,
}

fn settle_title(showing: &str, waiting: bool, incoming: &str) -> TitleSettle {
    if anterm_core::tab_view::same_title_ignoring_status_mark(incoming, showing) {
        return TitleSettle::Revert;
    }
    match waiting {
        true => TitleSettle::Queue,
        false => TitleSettle::QueueAndWait,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GitRefresh {
    Edge,
    Opportunistic,
    /// Nothing happened; the cache just has no answer for this pane's cwd
    /// yet, or the last probe found no repository. Throttled by [`GIT_RETRY_GAP`].
    Retry,
}

fn known_pty_shim(fg: &str) -> Option<&'static str> {
    ["kiro-cli-term", "figterm", "qterm", "cwterm"]
        .into_iter()
        .find(|shim| fg.contains(shim))
}

fn integration_notice_message(wrapper: Option<&str>) -> String {
    match wrapper {
        Some(w) => t_fmt(L10nKey::IntegrationNoticeBlocked, &[("wrapper", w)]),
        None => t(L10nKey::IntegrationNoticeNotEngaged).to_string(),
    }
}

/// Join whatever context a pane has into a notification title, most specific
/// part first: an agent name if one is running, otherwise the machine the pane
/// lives on, then the workspace it belongs to. Kept to two segments — Windows
/// toast titles are a single line and ellipsize anything longer.
fn compose_notification_title(
    lead: Option<String>,
    host: Option<String>,
    workspace: Option<String>,
) -> String {
    match (lead.or(host), workspace) {
        (Some(lead), Some(workspace)) => format!("{lead} · {workspace}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => "anterm".to_string(),
    }
}

/// The longest command line to put in a confirmation. The shell sends up to
/// 512 bytes; a dialog asking whether to end your work should still read as a
/// sentence.
const BUSY_COMMAND_MAX: usize = 60;

/// Undo the escaping the shell integration applies to an OSC 133;C payload so
/// it cannot break OSC framing: `%` and the four control bytes. Anything else
/// is left exactly as the user typed it.
fn unescape_mark_text(raw: &str) -> String {
    let decoded = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    let mut out = String::with_capacity(decoded.len());
    for ch in decoded.chars() {
        match ch {
            '\x1b' | '\x07' | '\r' => {}
            '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn clamp_command(cmd: &str) -> String {
    let cmd = cmd.trim();
    match cmd.chars().count() > BUSY_COMMAND_MAX {
        false => cmd.to_string(),
        true => format!(
            "{}…",
            cmd.chars()
                .take(BUSY_COMMAND_MAX)
                .collect::<String>()
                .trim_end()
        ),
    }
}

fn fallback_chain(family: &str, configured: &[String]) -> Vec<String> {
    let mut chain = configured.to_vec();
    let mut pin = |name: &str| {
        if family != name && !chain.iter().any(|f| f == name) {
            chain.push(name.to_string());
        }
    };
    for name in anterm_ui_kit::config::platform_last_resort_fallbacks() {
        pin(name);
    }
    pin("Hack");
    chain
}

/// Put `chain` on the regular face and on the bold and italic ones.
///
/// Bold and italic never carry a chain of their own — `alt_font` copies theirs
/// off the regular face when they are built — so a rebuild that skipped them
/// would leave two of the three faces resolving against the old chain.
fn apply_fallback_chain(
    chain: Vec<String>,
    font: &mut Font,
    bold: &mut Option<Font>,
    italic: &mut Option<Font>,
) {
    let fallbacks = Some(gpui::FontFallbacks::from_fonts(chain));
    font.fallbacks = fallbacks.clone();
    if let Some(font) = bold {
        font.fallbacks = fallbacks.clone();
    }
    if let Some(font) = italic {
        font.fallbacks = fallbacks;
    }
}

/// Initial PTY grid dimensions and device-pixel cell metrics passed to
/// `Spawn` / `SpawnNativeSsh` so shells start at the pane's true size instead
/// of `80×24` (preventing a first-frame resize + `SIGWINCH` race that leaves
/// zsh's `PROMPT_SP` `%` end-of-line mark visible on startup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialGrid {
    pub size: TermSize,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl Default for InitialGrid {
    fn default() -> Self {
        Self {
            size: TermSize::new(80, 24),
            cell_w: 8,
            cell_h: 17,
        }
    }
}

impl InitialGrid {
    /// Compute grid cols/rows and device-pixel cell metrics for `pane_bounds`
    /// using the exact same `GRID_PAD_X`/`GRID_PAD_Y` insets, floor truncation,
    /// and scale rounding as `TerminalElement::prepaint` + `set_grid_size`.
    /// Falls back to `80×24` when `pane_bounds` are unset or degenerate.
    pub fn from_cell_metrics(
        pane_bounds: gpui::Size<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        scale: f32,
    ) -> Self {
        let cell_width = cell_width.max(px(1.));
        let line_height = line_height.max(px(1.));
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        let cell_w = (cell_width.as_f32() * scale).round().max(1.) as u16;
        let cell_h = (line_height.as_f32() * scale).round().max(1.) as u16;

        let inner_w = pane_bounds.width.as_f32() - 2. * GRID_PAD_X;
        let inner_h = pane_bounds.height.as_f32() - 2. * GRID_PAD_Y;
        if inner_w <= 0. || inner_h <= 0. {
            return Self {
                size: TermSize::new(80, 24),
                cell_w,
                cell_h,
            };
        }
        let cols = (inner_w / cell_width.as_f32()).floor() as usize;
        let rows = (inner_h / line_height.as_f32()).floor() as usize;
        if cols < 2 || rows < 2 {
            return Self {
                size: TermSize::new(80, 24),
                cell_w,
                cell_h,
            };
        }
        Self {
            size: TermSize::new(cols, rows),
            cell_w,
            cell_h,
        }
    }
}

impl TerminalView {
    /// Estimate the initial terminal grid for a pane occupying `pane_bounds`
    /// using the exact same font shaping, `GRID_PAD_X`/`GRID_PAD_Y` insets,
    /// line-height rounding, and display scale as `TerminalElement::prepaint`.
    /// Falls back to `80×24` when bounds are unset or degenerate.
    pub fn estimate_initial_grid(
        pane_bounds: gpui::Size<Pixels>,
        font_size: f32,
        window: &Window,
        cx: &App,
    ) -> InitialGrid {
        let config = cx.global::<Config>();
        let font_family = config.font_family.clone();
        let fallbacks = fallback_chain(&font_family, &config.font_fallbacks);
        let font_size_px = px(font_size);
        let mut font = gpui::font(font_family);
        font.fallbacks = Some(gpui::FontFallbacks::from_fonts(fallbacks));
        if let Some(features) = config
            .font_features
            .as_ref()
            .map(anterm_ui_kit::config::gpui_font_features)
        {
            font.features = features;
        }
        let sample = window.text_system().shape_line(
            gpui::SharedString::new_static("M"),
            font_size_px,
            &[gpui::TextRun {
                len: 1,
                font,
                color: gpui::Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let cell_width = sample.width.max(px(1.));
        let line_height = px((font_size * config.line_height).round()).max(px(1.));
        InitialGrid::from_cell_metrics(pane_bounds, cell_width, line_height, window.scale_factor())
    }
}

impl TerminalView {
    fn notify_command_finished(
        &self,
        label: &str,
        elapsed: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        let secs = elapsed.as_secs().to_string();
        let command = label.trim();
        let body = if command.is_empty() {
            t_fmt(L10nKey::NotifyCommandFinished, &[("secs", &secs)])
        } else {
            t_fmt(
                L10nKey::NotifyCommandFinishedWithCommand,
                &[("command", command), ("secs", &secs)],
            )
        };
        self.notify_pane(None, &body, cx);
    }

    fn notify_agent_finished(
        &self,
        agent: anterm_core::cli_agent::CLIAgent,
        elapsed: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        let secs = elapsed.as_secs().to_string();
        let body = t_fmt(L10nKey::NotifyAgentFinished, &[("secs", &secs)]);
        self.notify_pane(Some(agent.display_name()), &body, cx);
    }

    /// Notify about this pane, with a title describing where it lives and a
    /// click that reveals it. The id has to be the pane's *gpui entity id*:
    /// that is what `TrayAction::RevealPane` matches leaves on, and it is a
    /// different number from `pane_id`, which the daemon assigns.
    fn notify_pane(&self, lead: Option<&str>, body: &str, cx: &mut Context<Self>) {
        let title = self.notification_title(lead, cx);
        anterm_ui_kit::platform::notify::notify_desktop_for_pane(Some(&title), body, Some(cx.entity_id()));
    }

    /// Desktop notifications the program wrote (OSC 9, 99, 777), such as
    /// Claude Code's with its Notifications setting on `ghostty`, `kitty` or
    /// `iterm2`. The agent's Waiting mark on the tab is the daemon's doing.
    ///
    /// `Unfocused` holds a note back only while the reader is looking at this
    /// very pane. A pane whose agent reports through anterm's hooks already gets
    /// its Waiting and Done notices from `poll_agent_status` whenever
    /// `hooks_notify`, so the program's own copy would be a duplicate there.
    ///
    /// At most a few per pane every few seconds reach the desktop (pane output
    /// is untrusted), with one note saying the rest were not shown.
    fn show_program_notes(&self, hooks_notify: bool, window: &Window, cx: &mut Context<Self>) {
        let notes = self.terminal.take_osc_notes();
        let watched = window.is_window_active() && self.focus_handle.is_focused(window);
        let hooked = self.terminal.agent_session().is_some_and(|s| s.rich);
        let show = cx
            .global::<Config>()
            .notify_on_command_finish
            .allows(watched)
            && !(hooked && hooks_notify);
        if !notes.is_empty() {
            log::debug!(
                "{} program notification(s) {}",
                notes.len(),
                if show { "shown" } else { "held back" }
            );
        }
        // Asked on every poll, so the rest are said once a flood stops too,
        // and while notes are held back, so a stale count never surfaces
        // minutes later.
        let (notes, dropped) = self.terminal.pace_osc_notes(
            if show { notes } else { Vec::new() },
            std::time::Instant::now(),
        );
        if !show {
            return;
        }
        let agent = self.terminal.foreground_agent().map(|a| a.display_name());
        if dropped {
            self.notify_pane(agent, t(L10nKey::ProgramNotesDropped), cx);
        }
        for (title, body) in notes {
            match title {
                Some(title) => anterm_ui_kit::platform::notify::notify_desktop_for_pane(
                    Some(&title),
                    &body,
                    Some(cx.entity_id()),
                ),
                None => self.notify_pane(agent, &body, cx),
            }
        }
    }

    fn notification_title(&self, lead: Option<&str>, _cx: &App) -> String {
        compose_notification_title(lead.map(str::to_string), None, None)
    }

    pub fn spawn_shell_terminal_in(
        working_directory: Option<std::path::PathBuf>,
        restore_pane: Option<u64>,
        shell: Option<ShellSpec>,
        initial: InitialGrid,
    ) -> anyhow::Result<ShellParts> {
        let route = crate::PaneRoute::Local;
        let attached = match restore_pane {
            Some(id) => match RemoteTerminal::attach_on(
                &route,
                initial.size,
                initial.cell_w,
                initial.cell_h,
                id,
            ) {
                Ok(terminal) => Some((terminal, id, None)),
                Err(e) if crate::attach_unanswered(&e) => {
                    log::warn!("attach to pane {id} went unanswered ({e:#}); spawning fresh");
                    None
                }
                Err(e) if crate::attach_refused(&e) => {
                    log::info!("pane {id} is gone on its machine ({e:#}); spawning fresh");
                    None
                }
                Err(e) => {
                    log::info!("pane {id} is gone ({e:#}); spawning fresh");
                    None
                }
            },
            None => None,
        };
        let restored = attached.is_some();
        let (terminal, pane_id, shell_spec) = match attached {
            Some(parts) => parts,
            None => {
                let restore = restore_pane.map(|pane_id| anterm_core::daemon::protocol::RestoreFrom {
                    pane_id,
                    banner: None,
                });
                let (terminal, id) = RemoteTerminal::spawn_on(
                    &route,
                    initial.size,
                    initial.cell_w,
                    initial.cell_h,
                    working_directory,
                    shell.clone(),
                    None,
                    restore,
                )?;
                (terminal, id, shell)
            }
        };
        Ok(ShellParts {
            terminal,
            pane_id,
            shell_spec,
            restored,
            restored_from: if restored { None } else { restore_pane },
        })
    }

    pub fn from_shell_parts(
        parts: ShellParts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::with_terminal(parts.terminal, parts.pane_id, window, cx);
        view.session.shell_spec = parts.shell_spec;
        view.session.restored = parts.restored;
        if let Some(old_pane_id) = parts.restored_from {
            let host_id = view.session.host_id;
            let new_pane_id = view.pane_id;
            let (optimistic, rx) = cx
                .default_global::<HistoryStore>()
                .carry_session(host_id, old_pane_id, new_pane_id);
            view.prompt.history_session_id = optimistic;
            cx.spawn(async move |this, cx| {
                if let Ok(resolved) = rx.recv().await {
                    let _ = this.update(cx, |view, cx| {
                        view.prompt.history_session_id = resolved.clone();
                        cx.default_global::<HistoryStore>()
                            .note_resolved_session(host_id, new_pane_id, resolved);
                        view.invalidate_ghost_cache();
                    });
                }
            })
            .detach();
        }
        view
    }

    pub fn restored(&self) -> bool {
        self.session.restored
    }

    /// Dials `spec` through the local daemon. `remote_start_dir` is a
    /// directory on the far host — see [`ClientMsg::SpawnNativeSsh`]'s `cwd`
    /// — and never a local one.
    ///
    /// [`ClientMsg::SpawnNativeSsh`]: anterm_core::daemon::protocol::ClientMsg::SpawnNativeSsh
    pub fn spawn_native_ssh_terminal(
        spec: Box<anterm_core::daemon::protocol::NativeSshSpec>,
        remote_start_dir: Option<std::path::PathBuf>,
        initial: InitialGrid,
    ) -> anyhow::Result<NativeSshParts> {
        let persist = Box::new(spec.without_secrets());
        let (terminal, pane_id) = RemoteTerminal::spawn_native_ssh(
            initial.size,
            initial.cell_w,
            initial.cell_h,
            remote_start_dir,
            spec,
        )?;
        Ok(NativeSshParts {
            terminal,
            pane_id,
            persist,
        })
    }

    pub fn from_native_ssh_parts(
        parts: NativeSshParts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::with_terminal(parts.terminal, parts.pane_id, window, cx);
        if let Some(name) = parts
            .persist
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            view.session.default_title = name.to_string();
            view.session.title = name.to_string();
        }
        view.session.ssh_spec = Some(parts.persist);
        view
    }

    fn with_terminal(
        terminal: RemoteTerminal,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = cx.global::<Config>();
        let font_family = config.font_family.clone();
        let fallbacks = fallback_chain(&font_family, &config.font_fallbacks);
        let font_size = px(config.font_size);
        let line_height_mul = config.line_height;
        let font_features = config
            .font_features
            .as_ref()
            .map(anterm_ui_kit::config::gpui_font_features);
        let report_mouse = config.mouse_reporting;
        let prompt_editor = config.prompt_editor;
        let helix_mode = config.helix_mode;
        let agent_detection_armed = !terminal.reattached();
        let mut font = gpui::font(font_family);
        font.fallbacks = Some(gpui::FontFallbacks::from_fonts(fallbacks.clone()));
        if let Some(features) = &font_features {
            font.features = features.clone();
        }
        let alt_font = |family: &Option<String>| {
            family.as_ref().map(|f| {
                let mut af = gpui::font(f.clone());
                af.fallbacks = Some(gpui::FontFallbacks::from_fonts(fallbacks.clone()));
                if let Some(features) = &font_features {
                    af.features = features.clone();
                }
                af
            })
        };
        let font_bold = alt_font(&config.font_family_bold);
        let font_italic = alt_font(&config.font_family_italic);

        let focus_handle = cx.focus_handle();
        let parent_entity = cx.entity();
        let weak_self = cx.weak_entity();
        let canvas = cx.new(|_| TerminalCanvas {
            view: weak_self.clone(),
        });
        let search_bar = cx.new(|bar_cx| {
            bar_cx.observe(&parent_entity, |_, _, cx| cx.notify()).detach();
            TerminalSearchBar {
                view: weak_self.clone(),
            }
        });
        let composer_view = cx.new(|comp_cx| {
            comp_cx
                .observe(&parent_entity, |this: &mut AgentComposerView, _, cx| {
                    this.measured_size.set(None);
                    cx.notify();
                })
                .detach();
            AgentComposerView {
                view: weak_self,
                measured_size: std::rc::Rc::new(std::cell::Cell::new(None)),
            }
        });

        let events = terminal.events.clone();
        cx.spawn(async move |this, cx| {
            let mut batch = Vec::new();
            while let Ok(ev) = events.recv().await {
                batch.push(ev);
                while let Ok(ev) = events.try_recv() {
                    batch.push(ev);
                }
                // Output reaches the screen through `handle_event`'s
                // `notify()`, which gpui scopes to windows currently
                // rendering this view. A pane in a background tab must stay
                // out of the frame loop entirely: refreshing the window
                // directly from here pinned the visible tab at full frame
                // rate whenever any hidden pane was producing output.
                let res = this.update(cx, |view, cx| {
                    let mut woke = false;
                    for ev in batch.drain(..) {
                        if matches!(ev, AlacEvent::Wakeup) && std::mem::replace(&mut woke, true) {
                            continue;
                        }
                        view.handle_event(ev, cx);
                    }
                });
                if res.is_err() {
                    break;
                }
            }
        })
        .detach();

        let focus_subs = vec![
            cx.on_focus_in(&focus_handle, window, |view, _window, cx| {
                view.render.cursor_visible = true;
                if view.agent.keep_unread_on_focus {
                    view.agent.keep_unread_on_focus = false;
                } else if view.agent.agent_result_unread {
                    view.agent.agent_result_unread = false;
                    view.note_agent_result_unread(cx);
                    notify_chrome_meta(cx);
                } else {
                    view.note_agent_result_unread(cx);
                }
                view.report_focus_change(true);
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |view, _window, cx| {
                view.report_focus_change(false);
                cx.notify();
            }),
        ];

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(530))
                    .await;
                if this
                    .update_in(cx, |view, window, cx| {
                        // A pane can be focused once while it is being built, before
                        // its leaf is attached to the window. Moving focus away in
                        // that interval does not deliver the leaf a blur callback,
                        // so no flag a callback maintains can be trusted to say
                        // which pane the reader is on. The window's handle is
                        // authoritative here, just as it is in the paint path.
                        if view.focus_handle.is_focused(window) {
                            if cx.global::<Config>().cursor_blink {
                                view.render.cursor_visible = !view.render.cursor_visible;
                                view.notify_canvas(cx);
                            } else if !view.render.cursor_visible {
                                view.render.cursor_visible = true;
                                view.notify_canvas(cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let displayed_for_poll = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let displayed = displayed_for_poll.clone();
        cx.spawn(async move |this, cx| {
            loop {
                // Hidden background tabs poll gently (1s) instead of waking
                // the UI thread every 300ms for each inactive pane.
                let interval = if displayed_for_poll.load(std::sync::atomic::Ordering::Relaxed) {
                    std::time::Duration::from_millis(300)
                } else {
                    std::time::Duration::from_millis(1000)
                };
                cx.background_executor().timer(interval).await;
                if this
                    .update_in(cx, |view, window, cx| view.poll_foreground(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        crate::render::color_scheme::watch(cx);

        let entity_id = cx.entity().entity_id();
        cx.default_global::<DisplayedRegistry>()
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(entity_id, displayed.clone());

        cx.on_release_in(window, move |view, window, cx| {
            if let Some(registry) = cx.try_global::<DisplayedRegistry>() {
                registry
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&entity_id);
            }
            view.terminal.detach_link();
            for image in view.terminal.images().take_for_release() {
                cx.drop_image(image, Some(window));
            }
        })
        .detach();

        window.focus(&focus_handle, cx);

        let host_id = anterm_ui_kit::host_ops::HostId::LOCAL;
        let history_scope = crate::prompt::history::Scope::Local;
        let history_session_id = {
            let store = cx.default_global::<HistoryStore>();
            store.resolve_session(host_id, pane_id)
        };
        let initial_worker = cx
            .default_global::<HistoryStore>()
            .begin_scope_load(&history_scope, false);
        if let Some(worker) = initial_worker {
            let scope = history_scope.clone();
            let rx = worker.request_scope_load(scope.clone(), Vec::new());
            cx.spawn(async move |this, cx| {
                if let Ok(loaded) = rx.recv().await {
                    let _ = this.update(cx, |view, cx| {
                        cx.default_global::<HistoryStore>()
                            .install_loaded_scope(&scope, loaded);
                        view.invalidate_ghost_cache();
                        if view.prompt.history_scope == scope {
                            view.refresh_reverse_search(cx);
                        }
                        cx.notify();
                    });
                }
            })
            .detach();
        }

        Self {
            terminal,
            pane_id,
            focus_handle,
            canvas,
            search_bar,
            composer_view,
            session: TerminalSessionState {
                host_id,
                shell_spec: None,
                restored: false,
                ssh_spec: None,
                remote_clipboard_dir: None,
                remote_clipboard_write_in_flight: None,
                remote_clipboard_write_generation: 0,
                displayed,
                title: DEFAULT_TITLE.to_string(),
                pending_title: None,
                default_title: DEFAULT_TITLE.to_string(),
                marked_text: String::new(),
                created_at: std::time::Instant::now(),
            },
            render: TerminalRenderState {
                font,
                font_bold,
                font_italic,
                font_features,
                font_size,
                line_height_mul,
                cell_width: px(8.),
                line_height: px(17.),
                cell_width_cache: None,
                grid_slack: px(0.),
                grid_buf: Vec::new(),
                grid_snap: None,
                row_cache: super::element::RowCache::default(),
                frame_alt_screen: false,
                frame_has_selection: false,
                cursor_visible: true,
                bell_flash: false,
                bell_epoch: 0,
                cell_geom: None,
                ime_anchor: None,
                dim: 1.,
            },
            pointer: PointerAndScrollState {
                selecting: false,
                drag_scroll: None,
                drag_scroll_epoch: 0,
                scroll_anim: None,
                scroll_anim_epoch: 0,
                gesture_until: None,
                gesture_zoom: None,
                last_mouse_cell: None,
                mouse_grab: None,
                context_menu_allowed: true,
                scroll_debt: 0.,
                zoom_debt: 0.,
                scroll_frac: 0.,
                scroll_handle: TerminalScrollHandle::default(),
                report_mouse,
            },
            links: LinkProbeState {
                last_hover_cell: None,
                link_modifier_down: false,
                link_probes: Default::default(),
                link_repo_root: None,
                link_repo_root_pending: false,
                menu_link: None,
                hovered_link: None,
            },
            search_state: TerminalSearchState::default(),
            composer_state: ComposerHostState::default(),
            agent: AgentSessionTracker {
                last_at_prompt: false,
                last_typeahead_blocked: false,
                running_since: None,
                running_title: String::new(),
                running_agent: None,
                seen_agent: None,
                agent_detection_armed,
                last_agent_status: None,
                last_agent_session: (None, None),
                agent_turn_started: None,
                pending_finish_notice: None,
                agent_was_rich: false,
                agent_result_unread: false,
                keep_unread_on_focus: false,
                agent_status_seen: false,
                git_status_cwd: None,
                last_agent_activity: 0,
            },
            prompt: PromptSubsystem {
                cmd: CmdEditor::new(),
                prompt_editor,
                helix_mode,
                typeahead: Typeahead::new(),
                hold: GapHold::new(),
                history_scope,
                history_session_id,
                history_frecency: Vec::new(),
                history_frecency_key: (u64::MAX, None, false),
                history_seed_pending: false,
                history_load_gen: 0,
                ranked_cwd: None,
                history_nav: None,
                history_stash: String::new(),
                history_prefix: String::new(),
                ghost_cache: std::cell::RefCell::new(None),
                last_word_nav: None,
                pending_history: None,
                completion: None,
                remote_completion_inflight: false,
                remote_completion_notice: None,
                completion_generation: 0,
                editor_handoff: None,
                editor_handoff_interrupt_seq: None,
                reverse_search: None,
                integration_notice: None,
                integration_notice_shown: false,
                editor_selecting: false,
                editor_select_gesture: false,
                editor_drag_word: None,
                editor_goal_col: None,
            },
            _focus_subs: focus_subs,
        }
    }

    pub fn set_font_size(&mut self, font_size: Pixels, cx: &mut Context<Self>) {
        self.render.font_size = font_size;
        cx.notify();
    }

    pub fn set_line_height_mul(&mut self, line_height_mul: f32, cx: &mut Context<Self>) {
        self.render.line_height_mul = line_height_mul;
        cx.notify();
    }

    pub fn set_report_mouse(&mut self, report_mouse: bool, cx: &mut Context<Self>) {
        if self.pointer.report_mouse != report_mouse {
            self.pointer.report_mouse = report_mouse;
            cx.notify();
        }
    }

    pub fn ensure_cursor_visible(&mut self, cx: &mut Context<Self>) {
        if !self.render.cursor_visible {
            self.render.cursor_visible = true;
            cx.notify();
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn cursor_visible(&self) -> bool {
        self.render.cursor_visible
    }

    pub fn set_grid_size(
        &mut self,
        cols: usize,
        rows: usize,
        cell_width: Pixels,
        line_height: Pixels,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        // A column change reflows every wrapped line, and a match point is an
        // absolute (line, column) against the width it was scanned at — so
        // the highlights keep washing the *old* positions until something
        // rescans. Output rescans them (`Wakeup` → refresh), but a quiet
        // local pane has no output coming: without this the drift outlasts
        // the resize indefinitely (#586). Rescan with the output path's
        // discipline — it keeps the selection and never scrolls. Rows alone
        // reflow nothing, so a height-only drag stays cheap.
        let old_size = self.terminal.size();
        let cols_changed = cols != old_size.cols;
        let grid_changed = (cols, rows) != (old_size.cols, old_size.rows);
        let cell_changed = self.render.cell_width != cell_width || self.render.line_height != line_height;
        let old_cursor = self.cursor_cell();
        if grid_changed {
            self.links.last_hover_cell = None;
            self.links.hovered_link = None;
        }
        self.render.cell_width = cell_width;
        self.render.line_height = line_height;
        // Report the cell size to the child in *device* pixels (logical × display
        // scale), so `ws_xpixel`/`ws_ypixel` describe the real framebuffer. A
        // pixel-aware program like terminal-browser renders its frame at that
        // native resolution; painted back into logical-pixel bounds, gpui blits
        // it ~1:1 on the framebuffer instead of upscaling a half-resolution
        // bitmap (which looked soft and magnified on Retina). This is what
        // kitty/ghostty report. `self.cell_width` stays logical — glyph layout
        // and mouse mapping work in logical pixels.
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        self.terminal.resize(
            TermSize::new(cols, rows),
            (cell_width.as_f32() * scale).round().max(1.) as u16,
            (line_height.as_f32() * scale).round().max(1.) as u16,
        );
        if cols_changed && self.search_state.active.is_some() {
            self.refresh_matches_after_output(cx);
        }
        grid_changed || cell_changed || self.cursor_cell() != old_cursor
    }

    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        self.terminal.foreground_cwd()
    }

    /// The title this pane is showing, or `None` while it is still answering
    /// to the app's own name — see [`stated_title`]. The label ladder reads
    /// this where the machine tree reads
    /// [`PaneRecord::osc_title`](anterm_core::machine::PaneRecord::osc_title),
    /// which is what lets the tab strip and the switcher name a tab the same
    /// way.
    pub fn stated_title(&self) -> Option<&str> {
        stated_title(&self.session.title)
    }

    /// `name` with the suffix that says how this pane ended.
    fn ended_title(&self, name: &str) -> String {
        t_fmt(L10nKey::PaneTitleProcessExited, &[("title", name)])
    }

    /// Sets how opaque the pane wants this terminal painted; the pane leaf
    /// calls this every frame while rendering, and the terminal element
    /// blends its colours toward the window background during paint (see
    /// [`Self::dim`] for why that beats an element-opacity style). The
    /// element resets the value to 1.0 at the end of every paint, so a
    /// render site that forgets to set it gets full brightness next frame —
    /// never a stale dim.
    pub fn set_dim(&mut self, dim: f32) {
        self.render.dim = dim;
    }

    pub fn remote_context(&self) -> Option<RemoteContext> {
        self.terminal.remote_context()
    }

    pub fn local_cwd(&self) -> Option<std::path::PathBuf> {
        self.paths_are_local().then(|| self.cwd())?
    }

    fn paths_are_local(&self) -> bool {
        self.remote_context().is_none() && self.session.host_id.is_local()
    }

    /// The directory a `~` in this pane's paths stands for, for anything that
    /// draws one of them shortened.
    ///
    /// The pane's own host answers, never this machine on its behalf (#580).
    /// A shell that has ssh'd somewhere gets no answer at all: the paths it
    /// reports are on a third machine anterm has no link to, and neither this
    /// laptop's home nor the pane host's describes it — the same reason #568
    /// stopped resolving file links in those panes.
    pub fn display_home(&self, cx: &gpui::App) -> Option<std::path::PathBuf> {
        match self.remote_context().is_some() {
            true => None,
            false => anterm_ui_kit::path_display::home_for_host(cx, self.session.host_id),
        }
    }

    pub fn spawnable_cwd(&self) -> Option<std::path::PathBuf> {
        self.remote_context().is_none().then(|| self.cwd())?
    }

    pub fn host(&self, cx: &gpui::App) -> Option<anterm_ui_kit::host_ops::SharedHost> {
        anterm_ui_kit::host_registry::HostRegistry::lookup(cx, self.session.host_id)
    }

    pub fn host_id(&self) -> anterm_ui_kit::host_ops::HostId {
        self.session.host_id
    }

    pub fn pane_route(&self) -> crate::PaneRoute {
        crate::PaneRoute::Local
    }

    fn accepts_input(&self, _cx: &gpui::App) -> bool {
        true
    }

    #[cfg(test)]
    pub fn adopt_relink(
        &mut self,
        stream: anterm_core::daemon::transport::Stream,
        buffered: Vec<u8>,
        route: &crate::PaneRoute,
        size: TermSize,
        cell_w: u16,
        cell_h: u16,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.session.remote_clipboard_write_generation =
            self.session.remote_clipboard_write_generation.wrapping_add(1);
        self.session.remote_clipboard_write_in_flight = None;
        self.terminal
            .adopt_relink(stream, buffered, route, size, cell_w, cell_h)?;
        self.session.title = self.session.default_title.clone();
        cx.notify();
        Ok(())
    }

    pub fn host_cwd(&self) -> Option<std::path::PathBuf> {
        self.cwd_is_on_host().then(|| self.cwd())?
    }

    fn cwd_is_on_host(&self) -> bool {
        cwd_is_on_host(!self.paths_are_local(), self.session.host_id.is_local())
    }

    pub fn agent(&self) -> Option<anterm_core::cli_agent::CLIAgent> {
        self.terminal.foreground_agent()
    }

    pub fn agent_session(&self) -> Option<anterm_core::cli_agent::AgentSessionState> {
        self.terminal.agent_session()
    }

    /// What this pane is in the middle of, when it can say so. `None` means
    /// either nothing is running or the shell never told us — and a terminal
    /// that guessed would raise this question on every single close.
    pub fn busy(&self) -> Option<PaneBusy> {
        use anterm_core::cli_agent::AgentStatus;
        // `Done` is the opposite of busy: the turn is over, and the badge
        // saying so is exactly what sends a reader to close the tab. Only a
        // turn still in flight — running, or stopped on a question — is work
        // that closing would cut short.
        if let Some(agent) = self.agent()
            && self
                .agent_session()
                .is_some_and(|s| matches!(s.status, AgentStatus::Working | AgentStatus::Waiting))
        {
            return Some(PaneBusy::Agent(agent.display_name()));
        }
        // Without shell integration `at_prompt` is permanently false, so
        // `running_since` is permanently Some. Only trust it when the shell is
        // actually reporting.
        if !self.terminal.shell_active() {
            return None;
        }
        self.agent.running_since?;
        // Prefer what the shell said it was running. `running_title` is only
        // the window title as it stood when the command began, and a prompt
        // that titles by directory — a very common setup — made this ask
        // "anterm is still running. Closing ends it." about a folder. The
        // OSC 133;C mark carries the submitted line itself.
        let named = Some(clamp_command(&unescape_mark_text(
            &self.terminal.running_command(),
        )))
        .filter(|t| !t.is_empty());
        Some(PaneBusy::Command(match named {
            Some(cmd) => cmd,
            None => match self.agent.running_title.trim().is_empty() {
                true => self.session.title.clone(),
                false => self.agent.running_title.clone(),
            },
        }))
    }

    pub fn agent_result_unread(&self) -> bool {
        self.agent.agent_result_unread
    }

    pub fn mark_agent_result_unread(&mut self, refocus_incoming: bool, cx: &mut App) {
        self.agent.agent_result_unread = true;
        self.agent.keep_unread_on_focus = refocus_incoming;
        self.note_agent_result_unread(cx);
    }

    /// Leave what the reader has seen of this pane's agent where the pane's
    /// next view will look for it — see [`AgentReadMarks`]. A mark left at any
    /// status other than `Done` never vouches for a finished turn, but it still
    /// records that this app was watching: a turn that was running when the
    /// view went and finished before the next one came must badge.
    fn record_agent_read_mark(&self, turns: u64, cx: &mut App) {
        cx.default_global::<AgentReadMarks>().0.insert(
            (self.session.host_id, self.pane_id),
            AgentReadMark {
                session: self.agent.last_agent_session.clone(),
                status: self.agent.last_agent_status,
                turns,
                unread: self.agent.agent_result_unread,
            },
        );
    }

    /// Carry a change to the badge alone into the mark the last status left.
    fn note_agent_result_unread(&self, cx: &mut App) {
        if !cx.has_global::<AgentReadMarks>() {
            return;
        }
        let key = (self.session.host_id, self.pane_id);
        if let Some(mark) = cx.global_mut::<AgentReadMarks>().0.get_mut(&key) {
            mark.unread = self.agent.agent_result_unread;
        }
    }

    pub fn git_status(&self, cx: &App) -> Option<anterm_ui_kit::git::status::GitStatus> {
        let cwd = self.agent.git_status_cwd.as_ref()?;
        cx.try_global::<anterm_ui_kit::git::status::GitStatusCache>()?
            .status_for(self.session.host_id, cwd)
    }

    pub fn git_status_cwd(&self) -> Option<&std::path::Path> {
        self.agent.git_status_cwd.as_deref()
    }

    /// Retry unknown and negative repo answers. A transient discovery failure
    /// must not leave an idle pane in Ungrouped until it moves or restarts.
    ///
    /// Read-only on purpose: this runs on every poll tick, and the cache is a
    /// global whose every mutable touch redraws the window — while a probe is
    /// in flight or throttled, the answer has to be "no" at no cost.
    fn git_retry_due(&self, cx: &App) -> bool {
        let Some(cwd) = self.agent.git_status_cwd.as_deref() else {
            return false;
        };
        let Some(cache) = cx.try_global::<anterm_ui_kit::git::status::GitStatusCache>() else {
            return false;
        };
        cache.retry_due(self.session.host_id, cwd, GIT_RETRY_GAP)
    }

    /// The directory this pane's *work* is happening in — what every panel
    /// that answers "where am I?" should show.
    ///
    /// [`Self::cwd`] is the kernel's idea: the cwd of the foreground process.
    /// That is right for a shell, and wrong for a coding agent, because an
    /// agent moving into a git worktree does not `chdir` — the `claude`
    /// process stays where it was launched while the session works somewhere
    /// else entirely. The hook stream carries the agent's own cwd for exactly
    /// this reason (`AgentSessionState::cwd`), and the git-status poll already
    /// folds the two together into `git_status_cwd`; this reads that result
    /// back out under a name that doesn't imply it's only about git.
    ///
    /// `git_status_cwd` is only ever set for a pane whose paths belong to its
    /// host, so the fallback here is what decides that: this one takes any
    /// cwd, [`Self::effective_host_cwd`] takes only one the host can resolve.
    pub fn effective_cwd(&self) -> Option<std::path::PathBuf> {
        self.agent.git_status_cwd.clone().or_else(|| self.cwd())
    }

    /// [`Self::effective_cwd`], restricted to paths the pane's host can act
    /// on — for callers that will hand the result to a `Host` call.
    pub fn effective_host_cwd(&self) -> Option<std::path::PathBuf> {
        self.agent.git_status_cwd.clone().or_else(|| self.host_cwd())
    }

    /// The directory the Files panel roots its tree at — see [`files_cwd`].
    ///
    /// Follows a coding agent the way [`Self::effective_cwd`] does. A WSL
    /// pane gets no `git_status_cwd` (its paths are not its host's), so the
    /// agent's own cwd is read here directly before the shell's.
    pub fn files_cwd(&self) -> Option<std::path::PathBuf> {
        let distro = wsl_share_distro(
            self.terminal.remote_context().as_ref(),
            self.session.host_id.is_local(),
        );
        let cwd = self
            .terminal
            .agent_session()
            .and_then(|s| s.cwd)
            .or_else(|| self.cwd());
        files_cwd(self.effective_host_cwd(), distro.as_deref(), cwd)
    }

    pub fn refresh_git_status_now(&mut self, cx: &mut Context<Self>) {
        let cwd = self.agent.git_status_cwd.clone();
        if cwd.is_some() {
            self.refresh_git_status(cwd, GitRefresh::Opportunistic, cx);
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.terminal
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.trim().is_empty())
    }

    pub fn send_agent_prompt(&self, prompt: &str) {
        self.terminal
            .write(anterm_core::agent_prompt::submit_bytes(prompt));
    }

    pub fn run_command_line(&self, cmd: &str) {
        self.terminal.write(format!("{cmd}\r").into_bytes());
    }

    pub fn shell_spec(&self) -> Option<ShellSpec> {
        self.session.shell_spec.clone()
    }

    /// The shell binary this pane is running, for the path-quoting rules.
    ///
    /// `None` before the pane has resolved one, which [`quote_for_shell`]
    /// answers from the platform — PowerShell on Windows, POSIX elsewhere.
    /// The one pane that guess is wrong for is a cmd.exe pane that has not
    /// reported in yet.
    pub(super) fn shell_program(&self) -> Option<String> {
        self.session.shell_spec.as_ref().map(|s| s.program.clone())
    }

    pub fn ssh_spec(&self) -> Option<Box<anterm_core::daemon::protocol::NativeSshSpec>> {
        self.session.ssh_spec.clone()
    }

    pub fn ssh_phase(&self) -> Option<anterm_core::daemon::protocol::SshPhase> {
        self.terminal.ssh_phase()
    }

    pub fn ssh_disconnected(&self) -> bool {
        self.session.ssh_spec.is_some() && self.terminal.exited
    }

    /// Takes a title the program set, and gives it to the tab only once it has
    /// stood for `TITLE_SETTLE`.
    ///
    /// Nearly every prompt framework titles the tab with the command it is
    /// about to run and puts the old title back at the next prompt — that is
    /// where a tab's process name comes from. For a command that finishes in a
    /// blink both edges land within a few frames of each other, so the label
    /// flashed the command and snapped back, which reads as a rendering glitch
    /// rather than as information. Holding the change means a command has to
    /// actually still be running to name the tab.
    ///
    /// A title that reverts before it lands never happens at all: the revert
    /// matches what the tab already shows and just drops the pending one. A
    /// program that keeps rewriting its title — a download writing progress
    /// there — still updates, because the wait already in flight adopts
    /// whatever the newest title is when it elapses rather than starting over.
    fn set_title_when_settled(&mut self, title: String, cx: &mut Context<Self>) {
        match settle_title(&self.session.title, self.session.pending_title.is_some(), &title) {
            TitleSettle::Revert => self.session.pending_title = None,
            TitleSettle::Queue => self.session.pending_title = Some(title),
            TitleSettle::QueueAndWait => {
                self.session.pending_title = Some(title);
                cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(TITLE_SETTLE).await;
                    view.update(cx, |view, cx| {
                        if let Some(title) = view.session.pending_title.take() {
                            view.session.title = title;
                            notify_chrome_meta(cx);
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn poll_remote_clipboard_write(&mut self, cx: &mut Context<Self>) {
        if self.session.remote_clipboard_write_in_flight.is_some() {
            return;
        }
        let write = loop {
            let Some(write) = self.terminal.pop_clipboard_write() else {
                return;
            };
            if allows_remote_clipboard_write(self.session.ssh_spec.as_deref()) {
                break write;
            }
            self.terminal.finish_clipboard_write();
            self.terminal.write(anterm_core::clipboard::response(
                write.id.as_deref(),
                "EPERM",
            ));
        };

        let generation = self.session.remote_clipboard_write_generation;
        self.session.remote_clipboard_write_in_flight = Some(generation);
        let request_id = write.id.clone();
        cx.spawn(async move |view, cx| {
            let validated = cx
                .background_spawn(async move { validate_remote_clipboard_image(write) })
                .await;
            view.update(cx, |view, cx| {
                if view.session.remote_clipboard_write_in_flight != Some(generation) {
                    return;
                }
                view.session.remote_clipboard_write_in_flight = None;
                view.terminal.finish_clipboard_write();
                match validated {
                    Ok((image, id)) => {
                        cx.write_to_clipboard(ClipboardItem::new_image(&image));
                        view.terminal
                            .write(anterm_core::clipboard::response(id.as_deref(), "DONE"));
                    }
                    Err(reason) => {
                        log::warn!("refusing remote clipboard image: {reason}");
                        view.terminal.write(anterm_core::clipboard::response(
                            request_id.as_deref(),
                            "EINVAL",
                        ));
                    }
                }
                view.poll_remote_clipboard_write(cx);
            })
            .ok();
        })
        .detach();
    }

    fn handle_event(&mut self, ev: AlacEvent, cx: &mut Context<Self>) {
        self.terminal.poll_exited();
        self.sync_typeahead_owner();
        self.poll_remote_clipboard_write(cx);
        if self.terminal.has_pending_auth() {
            cx.emit(AuthPromptReady);
        }
        match ev {
            AlacEvent::Wakeup => {
                self.sync_shared_history(cx);
                // The grid moved under whatever the search bar last measured.
                self.note_output_under_search(cx);
                // Only a pane that is on screen repaints on output. Notify the
                // inner `TerminalCanvas` alone on pure output frames so cached
                // workspace chrome (tab sidebar, tab strip) skips re-rendering.
                if self.session.displayed.load(std::sync::atomic::Ordering::Relaxed) {
                    let needs_outer = self.input_active() || self.composer_is_open();
                    if needs_outer {
                        cx.notify();
                    } else {
                        self.notify_canvas(cx);
                    }
                }
            }
            AlacEvent::Title(title) => self.set_title_when_settled(title, cx),
            AlacEvent::ResetTitle => self.set_title_when_settled(self.session.default_title.clone(), cx),
            AlacEvent::PtyWrite(text) => self.terminal.write(text.into_bytes()),
            AlacEvent::ChildExit(_) | AlacEvent::Exit => {
                self.terminal.exited = true;
                // Nothing is left to settle, and a title still waiting its turn
                // would land on top of the state below a moment from now.
                self.session.pending_title = None;
                // The pane keeps answering to its own name (an SSH pane's
                // host, #438) — only the state suffix is localized (#602).
                self.session.title = self.ended_title(&self.session.default_title);
                if self.terminal.child_exited() {
                    cx.emit(ChildExited);
                }
                notify_chrome_meta(cx);
                cx.notify();
            }
            AlacEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            AlacEvent::ClipboardLoad(_, fmt) => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.terminal.write(fmt(&text).into_bytes());
                }
            }
            AlacEvent::ColorRequest(idx, fmt) => {
                let theme = cx.theme();
                let rgb = match idx {
                    256 => crate::render::palette::hsla_to_rgb(theme.foreground),
                    257 => crate::render::palette::hsla_to_rgb(theme.background),
                    258 => crate::render::palette::hsla_to_rgb(theme.caret),
                    i => self.terminal.palette[i.min(255)],
                };
                self.terminal.write(fmt(rgb).into_bytes());
            }
            AlacEvent::Bell => match cx.global::<Config>().bell {
                BellMode::None => {}
                BellMode::Visual => self.flash_bell(cx),
                BellMode::Audible => {
                    if !ring_system_bell() {
                        self.flash_bell(cx);
                    }
                }
                BellMode::Both => {
                    ring_system_bell();
                    self.flash_bell(cx);
                }
            },
            AlacEvent::TextAreaSizeRequest(fmt) => {
                let size = self.terminal.size();
                let reply = fmt(alacritty_terminal::event::WindowSize {
                    num_lines: size.rows as u16,
                    num_cols: size.cols as u16,
                    cell_width: self.render.cell_width.as_f32().round() as u16,
                    cell_height: self.render.line_height.as_f32().round() as u16,
                });
                self.terminal.write(reply.into_bytes());
            }
            _ => {}
        }
    }

    fn report_focus_change(&self, focused: bool) {
        let mode = self.terminal.mode_nonblocking();
        if let Some(bytes) = focus_report_bytes(mode, focused) {
            self.terminal.write(bytes);
        }
    }

    pub fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        use alacritty_terminal::vte::ansi::{ClearMode, Handler as _};

        self.cancel_scroll_anim();
        // Go through `clear_screen` rather than `grid_mut().clear_history()`:
        // it drops a selection anchored in the rows we are about to discard and
        // clamps the vi cursor back into the grid, which purging the history
        // behind the term's back would leave pointing at rows that no longer
        // exist.
        {
            let mut term = self.terminal.term.lock();
            term.clear_screen(ClearMode::Saved);
            self.terminal.publish_term_state(&term);
        }
        // Image placements are anchored in absolute scrollback rows, so the
        // rows we just discarded moved every anchor. Drop them; the daemon does
        // not replay out-of-band image frames, so a browser redraws on its next
        // transmit (same reasoning as the reattach path in `adopt_relink`).
        self.terminal.images().clear();
        self.terminal.clear_scrollback();
        self.pointer.scroll_frac = 0.;
        self.terminal.write(vec![0x0c_u8]);
        cx.notify();
    }

    pub fn set_font_family(&mut self, family: String, cx: &mut Context<Self>) {
        let mut font = gpui::font(family);
        if let Some(features) = &self.render.font_features {
            font.features = features.clone();
        }
        self.render.font = font;
        // Rebuild rather than carry the chain over: `fallback_chain` skips
        // pinning a last-resort face that the family already is, so the chain
        // that went with the old family can be missing a pin the new one needs.
        self.reread_fallback_chain(cx);
        cx.notify();
    }

    /// Rebuild the fallback chain from the config and put it on all three faces.
    ///
    /// The chain is built once in `with_terminal` and then only ever cloned
    /// around, so a `font_fallbacks` edit reaches new panes and no one else.
    pub fn reread_fallback_chain(&mut self, cx: &mut Context<Self>) {
        let chain = fallback_chain(&self.render.font.family, &cx.global::<Config>().font_fallbacks);
        apply_fallback_chain(
            chain,
            &mut self.render.font,
            &mut self.render.font_bold,
            &mut self.render.font_italic,
        );
    }

    pub fn set_font_family_bold(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        self.render.font_bold = self.alt_font(family);
        cx.notify();
    }

    pub fn set_font_family_italic(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        self.render.font_italic = self.alt_font(family);
        cx.notify();
    }

    pub fn set_font_features(
        &mut self,
        features: Option<gpui::FontFeatures>,
        cx: &mut Context<Self>,
    ) {
        self.render.font_features = features.clone();
        let apply = |font: &mut Font| {
            font.features = features.clone().unwrap_or_default();
        };
        apply(&mut self.render.font);
        if let Some(font) = &mut self.render.font_bold {
            apply(font);
        }
        if let Some(font) = &mut self.render.font_italic {
            apply(font);
        }
        cx.notify();
    }

    fn alt_font(&self, family: Option<String>) -> Option<Font> {
        family.map(|f| {
            let mut af = gpui::font(f);
            af.fallbacks = self.render.font.fallbacks.clone();
            if let Some(features) = &self.render.font_features {
                af.features = features.clone();
            }
            af
        })
    }

    fn poll_foreground(&mut self, window: &Window, cx: &mut Context<Self>) {
        let notify_allowed = cx
            .global::<Config>()
            .notify_on_command_finish
            .allows(window.is_window_active());
        // Ahead of the exit check: what a program said just before its shell
        // exited is still shown.
        self.show_program_notes(notify_allowed, window, cx);
        if self.terminal.exited {
            return;
        }
        let at_prompt = self.terminal.at_prompt();

        self.sync_shared_history(cx);

        if self
            .prompt
            .pending_history
            .as_ref()
            .is_some_and(|p| at_prompt && self.terminal.prompt_seq() > p.seq)
        {
            self.flush_pending_history(cx);
            cx.notify();
        }

        if let Some(cwd) = self.cwd()
            && self.prompt.ranked_cwd.as_ref() != Some(&cwd)
        {
            self.prompt.ranked_cwd = Some(cwd);
            self.invalidate_ghost_cache();
            if self.prompt.reverse_search.is_some() {
                self.refresh_reverse_search(cx);
            }
            notify_chrome_meta(cx);
        }

        if self.prompt.integration_notice.is_some() && self.terminal.shell_active() {
            self.prompt.integration_notice = None;
            cx.notify();
        }

        if at_prompt != self.agent.last_at_prompt {
            self.agent.last_at_prompt = at_prompt;
            cx.notify();
        }

        let running = !at_prompt;
        if running && self.agent.running_agent.is_none() {
            self.agent.running_agent = self.terminal.foreground_agent();
        }
        let cmd_finished = self.agent.running_since.is_some() && !running;
        match (self.agent.running_since, running) {
            (None, true) => {
                self.agent.running_since = Some(std::time::Instant::now());
                self.agent.running_title = self.session.title.clone();
                self.agent.running_agent = self.terminal.foreground_agent();
            }
            (Some(start), false) => {
                let elapsed = start.elapsed();
                let title = std::mem::take(&mut self.agent.running_title);
                let agent = self.agent.running_agent.take();
                self.agent.running_since = None;
                if notify_allowed {
                    match agent {
                        Some(_) if self.agent.agent_was_rich => {}
                        Some(agent) => self.notify_agent_finished(agent, elapsed, cx),
                        None => {
                            let threshold = std::time::Duration::from_secs(
                                cx.global::<Config>().notify_threshold_secs,
                            );
                            if elapsed >= threshold {
                                self.notify_command_finished(&title, elapsed, cx);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        self.poll_agent_detection(at_prompt, cx);

        let turn_finished = self.poll_agent_status(notify_allowed, window, cx);

        let session = self.terminal.agent_session();
        let tool_activity = match session.as_ref().map(|s| s.activity) {
            Some(n) => std::mem::replace(&mut self.agent.last_agent_activity, n) != n,
            None => {
                self.agent.last_agent_activity = 0;
                false
            }
        };
        let cwd_now = self
            .cwd_is_on_host()
            .then(|| {
                session
                    .as_ref()
                    .and_then(|s| s.cwd.clone())
                    .or_else(|| self.cwd())
            })
            .flatten();
        if cwd_now.as_ref() != self.agent.git_status_cwd.as_ref() || cmd_finished || turn_finished {
            if cmd_finished || turn_finished {
                self.mark_repo_changed(cwd_now.as_deref(), cx);
            }
            self.refresh_git_status(cwd_now, GitRefresh::Edge, cx);
        } else if tool_activity {
            self.refresh_git_status(cwd_now, GitRefresh::Opportunistic, cx);
        } else if self.git_retry_due(cx) {
            self.refresh_git_status(cwd_now, GitRefresh::Retry, cx);
        }

        self.follow_history_scope(cx);
    }

    /// Tell the source control cache that a command just ran here.
    ///
    /// The `.git` watch catches anything that writes the repository, and the
    /// file tree catches edits in the directories it is showing. What is left
    /// is the common case neither sees: a command that edits a file somewhere
    /// the tree is not looking. A command boundary is the cheapest honest
    /// signal that that may have happened.
    ///
    /// Only the epoch moves. Scheduling the debounced re-read needs the app
    /// entity, which a pane does not hold — but `refresh_git_status` below
    /// writes `GitStatusCache`, the app observes that global, and the panel's
    /// next render finds the repository stale and asks. One notify, not two.
    fn mark_repo_changed(&self, cwd: Option<&std::path::Path>, cx: &mut Context<Self>) {
        use anterm_ui_kit::git::data::ScmData;
        use anterm_ui_kit::git::status::GitStatusCache;

        let Some(cwd) = cwd else { return };
        let Some(root) = cx
            .try_global::<GitStatusCache>()
            .and_then(|cache| cache.repo_root_for(self.session.host_id, cwd))
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        cx.default_global::<ScmData>().bump(self.session.host_id, &root);
    }

    fn ssh_connected(&self) -> bool {
        matches!(
            self.terminal.ssh_phase(),
            Some(anterm_core::daemon::protocol::SshPhase::Connected)
        )
    }

    fn refresh_git_status(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        trigger: GitRefresh,
        cx: &mut Context<Self>,
    ) {
        use anterm_ui_kit::git::status::GitStatusCache;

        let changed = self.agent.git_status_cwd != cwd;
        self.agent.git_status_cwd = cwd.clone();
        let Some(cwd) = cwd else {
            if changed {
                cx.notify();
            }
            return;
        };
        let id = self.session.host_id;
        let Some(host) = self.host(cx) else {
            if changed {
                cx.notify();
            }
            return;
        };
        if !host.is_connected() {
            if changed {
                cx.notify();
            }
            return;
        }
        cx.default_global::<GitStatusCache>();
        let claimed = cx.update_global::<GitStatusCache, _>(|cache, _| match trigger {
            GitRefresh::Edge => cache.begin_probe(id, &cwd),
            GitRefresh::Opportunistic => {
                cache.begin_probe_throttled(id, &cwd, OPPORTUNISTIC_GIT_GAP)
            }
            GitRefresh::Retry => cache.begin_probe_throttled(id, &cwd, GIT_RETRY_GAP),
        });
        if !claimed {
            return;
        }
        let probe_cwd = cwd.clone();
        let pane = cx.weak_entity();
        anterm_ui_kit::host_ops::HostOps::run_detached(
            host,
            cx,
            move |h| anterm_ui_kit::git::status::probe_repo(h, &probe_cwd),
            move |cx, result| {
                use anterm_ui_kit::git::status::RepoProbe;
                let rerun = cx.update_global::<GitStatusCache, _>(|cache, _| match result {
                    RepoProbe::Repo(snap) => cache.finish_probe(id, &cwd, Some(snap)),
                    RepoProbe::NotARepo => cache.finish_probe(id, &cwd, None),
                    RepoProbe::Failed => cache.fail_probe(id, &cwd),
                });
                if rerun {
                    let _ = pane.update(cx, |view, cx| {
                        if view.agent.git_status_cwd.as_deref() == Some(&cwd) {
                            view.refresh_git_status(Some(cwd), GitRefresh::Edge, cx);
                        }
                    });
                }
            },
        );
    }

    /// Report an agent that has just started in this pane ([`AgentDetected`]),
    /// once per start, and never one that was already running when this view
    /// reattached to the pane.
    fn poll_agent_detection(&mut self, at_prompt: bool, cx: &mut Context<Self>) {
        let agent = self.terminal.foreground_agent();
        if agent != self.agent.seen_agent {
            self.agent.seen_agent = agent;
            notify_chrome_meta(cx);
            if let Some(agent) = agent
                && self.agent.agent_detection_armed
            {
                cx.emit(AgentDetected(agent));
            }
        }
        if agent.is_none() && at_prompt {
            self.agent.agent_detection_armed = true;
        }
    }

    fn poll_agent_status(
        &mut self,
        notify_allowed: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use anterm_core::cli_agent::AgentStatus;

        // Attaching to a pane the daemon kept alive — the app restarting onto
        // last session's tabs above all — has the daemon replay the pane's
        // stored agent status as an ordinary report. When this app has never
        // watched the pane, that is a baseline, not an edge: the turn it
        // describes ended before this view existed, often before this process
        // did, and reading it as "a result just landed" is what used to bring
        // every restored agent tab up wearing an unread badge for output its
        // reader had long since read.
        //
        // When an earlier view of this app did watch the pane (a workspace
        // switched out and back, a window reopened from the tray), its read
        // mark knows more than the replay does, so the replay stays an edge and
        // the mark decides below: the same finished turn takes its badge back
        // as the reader left it, anything else is news (#870).
        let adopted_baseline = match self.terminal.take_replayed_agent_status() {
            Some(restored)
                if !cx
                    .try_global::<AgentReadMarks>()
                    .is_some_and(|marks| marks.0.contains_key(&(self.session.host_id, self.pane_id))) =>
            {
                self.agent.last_agent_status = restored;
                self.agent.agent_status_seen = true;
                true
            }
            _ => false,
        };

        let session = self.terminal.agent_session();
        if session.as_ref().is_some_and(|s| s.rich) {
            self.agent.agent_was_rich = true;
        }
        if self.terminal.foreground_agent().is_none() && session.is_none() {
            self.agent.agent_was_rich = false;
        }

        let identity = (
            session.as_ref().and_then(|s| s.session_id.clone()),
            session.as_ref().and_then(|s| s.launch_argv.clone()),
        );
        if identity != self.agent.last_agent_session {
            self.agent.last_agent_session = identity;
            cx.emit(AgentSessionChanged);
        }

        let status = session.as_ref().map(|s| s.status);
        let turns = session.as_ref().map_or(0, |s| s.turns);
        if adopted_baseline {
            // From here on this app is watching the pane, so a later rebuild
            // must find a mark rather than adopt its own replay.
            self.record_agent_read_mark(turns, cx);
        }
        if status == self.agent.last_agent_status {
            return false;
        }
        let prev = std::mem::replace(&mut self.agent.last_agent_status, status);
        let first_sight = !std::mem::replace(&mut self.agent.agent_status_seen, true);
        self.agent.pending_finish_notice = None;
        // anterm's own conclusion, not the agent's: the user interrupted the
        // turn, or it went silent. Neither is a result to announce.
        let inferred = session.as_ref().is_some_and(|s| s.inferred);

        // A view built over a pane that already holds a finished turn sees
        // `Done` arrive from nothing, the same as a turn finishing now. If the
        // pane's previous view left a mark for this session at this turn count,
        // it is the turn the reader was already shown: take their badge back as
        // they left it instead of raising a new one (#870). A turn that
        // finished after the old view went has no such mark, or a lower count.
        if first_sight
            && status == Some(AgentStatus::Done)
            && let Some(mark) = cx
                .try_global::<AgentReadMarks>()
                .and_then(|marks| marks.0.get(&(self.session.host_id, self.pane_id)))
                .filter(|mark| {
                    mark.status == Some(AgentStatus::Done)
                        && mark.session == self.agent.last_agent_session
                        && mark.turns == turns
                })
                .cloned()
        {
            self.agent.agent_result_unread = mark.unread && !self.focus_handle.is_focused(window);
            self.agent.keep_unread_on_focus = false;
            self.record_agent_read_mark(turns, cx);
            notify_chrome_meta(cx);
            cx.notify();
            return false;
        }

        let turn_finished = status == Some(AgentStatus::Done) && prev != Some(AgentStatus::Done);

        match status {
            Some(AgentStatus::Done) if prev != Some(AgentStatus::Done) => {
                // Whether the reader saw the result is a question about the
                // window's live focus, not about the last focus callback this
                // pane happened to receive. A pane focused while it was being
                // built, before its leaf was attached, never gets the blur that
                // would clear `self.focused` — trusting the cached flag there
                // drops the badge on the one pane the reader is not looking at.
                self.agent.agent_result_unread = !inferred && !self.focus_handle.is_focused(window);
                self.agent.keep_unread_on_focus = false;
            }
            Some(AgentStatus::Done) => {}
            _ => {
                self.agent.agent_result_unread = false;
                self.agent.keep_unread_on_focus = false;
            }
        }
        self.record_agent_read_mark(turns, cx);

        let rich = session.as_ref().is_some_and(|s| s.rich);
        let agent_name = self
            .terminal
            .foreground_agent()
            .map(|a| a.display_name())
            .unwrap_or("Agent");
        match status {
            Some(AgentStatus::Working) => {
                self.agent.agent_turn_started = Some(std::time::Instant::now());
            }
            Some(AgentStatus::Waiting) if rich && notify_allowed => {
                let body = session
                    .as_ref()
                    .and_then(|s| s.message.clone())
                    .unwrap_or_else(|| t(L10nKey::NotifyAgentWaiting).to_string());
                self.notify_pane(Some(agent_name), &body, cx);
            }
            Some(AgentStatus::Done)
                if rich
                    && notify_allowed
                    && !inferred
                    && matches!(
                        prev,
                        Some(AgentStatus::Working) | Some(AgentStatus::Waiting)
                    ) =>
            {
                let body = match self.agent.agent_turn_started.take() {
                    Some(start) => {
                        let secs = start.elapsed().as_secs().to_string();
                        t_fmt(L10nKey::NotifyAgentFinished, &[("secs", &secs)])
                    }
                    None => t(L10nKey::NotifyTurnFinished).to_string(),
                };
                // Held back briefly: a turn that ends only for the next one to
                // start at once — a message queued while the agent worked, a
                // `Stop` hook sending it back to work — is not finished, and
                // announcing it would be a notification about nothing.
                let agent_name = agent_name.to_string();
                self.agent.pending_finish_notice = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(AGENT_DONE_SETTLE).await;
                    this.update(cx, |view, cx| {
                        view.agent.pending_finish_notice = None;
                        view.notify_pane(Some(&agent_name), &body, cx);
                    })
                    .ok();
                }));
            }
            _ => {}
        }
        notify_chrome_meta(cx);
        cx.notify();
        turn_finished
    }

    /// Invalidates the high-frequency terminal surface without unnecessarily
    /// notifying outer chrome observers.
    pub fn notify_canvas(&self, cx: &mut Context<Self>) {
        self.canvas.update(cx, |_, cx| cx.notify());
    }

    /// Settle up with the scrollback bar for this frame: move the viewport
    /// where a drag asked for, then tell the bar where the grid ended up.
    ///
    /// Both halves belong here rather than in the handle, so the bar — which
    /// runs from a mouse handler, with no pane to call into — never reaches
    /// into the terminal behind the pane's back.
    fn sync_scrollbar(&mut self) {
        if let Some(target) = self.pointer.scroll_handle.take_pending() {
            // Whatever the wheel had in flight was heading somewhere else.
            self.cancel_scroll_anim();
            let mut term = self.terminal.term.lock();
            let delta = target as i32 - term.grid().display_offset() as i32;
            if delta != 0 {
                term.scroll_display(Scroll::Delta(delta));
                self.terminal.publish_term_state(&term);
            }
            drop(term);
            // A sub-line remainder left over from a smooth wheel scroll would
            // paint the grid shifted off the row the thumb just picked.
            self.pointer.scroll_frac = 0.;
        }
        // Not worth a wait: read from live term if uncontended, otherwise fall
        // back to the lock-free atomic mirror so the scrollbar never stalls
        // the window behind the reader thread.
        let (history, display_offset, screen_lines) = match self.terminal.term.try_lock_unfair() {
            Some(term) => {
                self.terminal.sync_term_state(&term);
                (
                    term.grid().history_size(),
                    term.grid().display_offset(),
                    term.screen_lines(),
                )
            }
            None => {
                let m = self.terminal.mirror();
                (m.history_size(), m.display_offset(), m.rows())
            }
        };
        let grid = GridScroll {
            history,
            display_offset,
            screen_lines,
            line_height: self.render.line_height.as_f32(),
        };
        self.pointer.scroll_handle.sync(grid);
    }

    /// Re-read the two things the frame itself declares — the terminal mode its
    /// keymap context is built from, and whether there is a selection to draw.
    ///
    /// Reads the live term when uncontended and falls back to the atomic
    /// [`crate::remote::TermStateMirror`] when the reader holds the
    /// lock, so drawing never queues behind output bursts.
    fn sync_frame_facts(&mut self) {
        if let Some(term) = self.terminal.term.try_lock_unfair() {
            self.terminal.sync_term_state(&term);
            self.render.frame_alt_screen = term.mode().contains(TermMode::ALT_SCREEN);
            self.render.frame_has_selection = term.selection.is_some();
        } else {
            let mirror = self.terminal.mirror();
            self.render.frame_alt_screen = mirror.alt_screen();
            self.render.frame_has_selection = mirror.has_selection();
        }
    }

    /// The scrollback bar, laid down the right edge of the canvas.
    ///
    /// Mounted inside [`TerminalCanvas`], which already sits inside
    /// `px(GRID_PAD_X)` / `py(GRID_PAD_Y)`, so the vertical inset is `0` and
    /// horizontal edges extend out by `GRID_PAD_X` to match the outer surface.
    fn render_scrollbar_in_canvas(&self) -> impl IntoElement + use<> {
        div()
            .absolute()
            .top_0()
            .left(px(-GRID_PAD_X))
            .right(px(-GRID_PAD_X))
            .h(self.render.line_height * self.terminal.size().rows as f32)
            .child(Scrollbar::vertical(&self.pointer.scroll_handle).id("terminal-scrollbar"))
    }
}

/// Why closing a pane would end work that is still going on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneBusy {
    Command(String),
    Agent(&'static str),
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        self.flush_pending_history_on_drop();
    }
}

impl TerminalView {
    /// Reconcile frame facts, typeahead/hold state, and composer presence before
    /// constructing this frame's element tree.
    fn prepare_render_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_frame_facts();
        self.sync_typeahead_owner();
        self.sync_composer(window, cx);
        if self.shell_owns_prompt() {
            if let Some((_net, bytes)) = self.prompt.hold.release() {
                self.terminal.write(bytes);
            }
            self.prompt.typeahead.drain();
        } else if self.input_active() {
            self.engage_hold_into_editor();
            if self.terminal.zle_reading() {
                self.flush_typeahead();
            }
        }
    }

    /// Attach input, drag-and-drop, and keybinding action handlers to the root
    /// terminal surface element.
    fn attach_surface_actions(
        &self,
        el: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        el.on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseDownEvent, window, cx| {
                    if window.default_prevented() {
                        return;
                    }
                    window.focus(&this.focus_handle, cx);
                }),
            )
            // Latch the context-menu verdict for this click. gpui-component's
            // `ContextMenu` element owns the right mouse-down that opens the
            // popup and hands the builder no event to inspect, so the modifiers
            // have to be recorded here. That element wraps this one, so its
            // listener actually fires *before* this one — harmless, because it
            // only builds the menu from a `window.defer` callback that runs
            // once the whole mouse dispatch has unwound.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _window, _cx| {
                    this.pointer.context_menu_allowed =
                        should_show_context_menu(this.mouse_mode(), ev.modifiers.shift);
                }),
            )
            .drag_over::<ExternalPaths>(|s, _, _, cx| s.bg(cx.theme().drag_border.opacity(0.12)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.drop_files(paths, cx);
            }))
            .drag_over::<anterm_ui_kit::panel_tokens::RemotePathDrag>(|s, _, _, cx| {
                s.bg(cx.theme().drag_border.opacity(0.12))
            })
            .on_drop(cx.listener(
                |this, drag: &anterm_ui_kit::panel_tokens::RemotePathDrag, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.drop_remote_path(drag, cx);
                },
            ))
            .on_action(cx.listener(|this, _: &CopyText, _w, cx| {
                this.copy_contextual(false, false, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyRaw, _w, cx| {
                // Nothing selected: let the chord reach the key handler.
                if !this.copy_contextual(false, true, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &CutText, _w, cx| {
                this.cut_contextual(cx);
            }))
            .on_action(cx.listener(|this, _: &PasteText, _w, cx| this.paste_from_clipboard(cx)))
            .on_action(cx.listener(|this, _: &AlternatePaste, _w, cx| this.alternate_paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _w, cx| this.select_all_contextual(cx)))
            .on_action(cx.listener(|this, _: &UndoEdit, _w, cx| this.undo_edit(false, cx)))
            .on_action(cx.listener(|this, _: &RedoEdit, _w, cx| this.undo_edit(true, cx)))
            .on_action(
                cx.listener(|this, _: &FindInTerminal, window, cx| this.open_search(window, cx)),
            )
            // Off macOS these two live on F3 and Shift+F3, which is also where
            // PSReadLine keeps CharacterSearch and readline users put their
            // own widgets. With no find bar open there is no next match to
            // step to, so the keystroke is given back the way `EditorSave`
            // gives back Ctrl+S — otherwise the action swallows the key and
            // the shell never sees it (#834).
            .on_action(cx.listener(|this, _: &FindNext, _w, cx| {
                if this.search_state.active.is_none() {
                    cx.propagate();
                    return;
                }
                this.step_match(Direction::Right, cx);
            }))
            .on_action(cx.listener(|this, _: &FindPrevious, _w, cx| {
                if this.search_state.active.is_none() {
                    cx.propagate();
                    return;
                }
                this.step_match(Direction::Left, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearScrollback, _w, cx| this.clear_scrollback(cx)))
            .on_action(cx.listener(|this, _: &OpenLinkUnderPointer, window, cx| {
                this.open_menu_link(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenLinkWithDefaultApp, window, cx| {
                this.open_menu_link_with_default_app(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RevealLinkUnderPointer, window, cx| {
                this.reveal_menu_link(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyLinkPathUnderPointer, _w, cx| {
                this.copy_menu_link_path(cx);
            }))
            .on_action(cx.listener(|this, _: &InsertNewline, _w, cx| {
                this.insert_newline_action(cx);
            }))
            .on_action(cx.listener(|this, _: &InsertNewlineFallback, _w, cx| {
                this.insert_newline_fallback_action(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleComposer, window, cx| {
                this.toggle_composer(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RunReverseSearchSelection, _w, cx| {
                this.run_reverse_search_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &SendTab, _w, cx| {
                this.tab_pressed(true, cx);
            }))
            .on_action(cx.listener(|this, _: &SendBackTab, _w, cx| {
                this.tab_pressed(false, cx);
            }))
    }

    /// Build the right-click context menu for this terminal pane.
    fn build_context_menu(
        menu: gpui_kit::component::menu::PopupMenu,
        menu_view: &gpui::Entity<Self>,
        menu_focus: &FocusHandle,
        has_selection: bool,
        window: &mut Window,
        cx: &mut Context<gpui_kit::component::menu::PopupMenu>,
    ) -> gpui_kit::component::menu::PopupMenu {
        // Suppressing the popup means handing back an item-less menu:
        // gpui-component's `ContextMenu` element skips rendering the
        // anchored overlay entirely when the built menu `is_empty()`,
        // and its right mouse-down listener is registered after ours
        // (it wraps this element), so we cannot out-order it or veto it
        // any earlier. See `should_show_context_menu` for the rule.
        if !menu_view.read(cx).pointer.context_menu_allowed {
            return menu;
        }
        let view = menu_view.read(cx);
        // A path is the most specific thing the pointer can be on, so
        // what it can do goes above what the pane can do.
        let menu = match view.menu_link_path() {
            Some(path) => {
                let local = view.session.host_id.is_local();
                let reveal = match cfg!(target_os = "macos") {
                    true => L10nKey::AppMenuRevealInFinder,
                    false => L10nKey::AppMenuRevealInFolder,
                };
                let label = link_menu_label(path);
                let default_app =
                    offers_default_app_open(local, cx.global::<Config>().file_open_mode());
                menu.min_w(px(220.))
                    .action_context(menu_focus.clone())
                    .label(label)
                    .menu(t(L10nKey::AppMenuOpenLink), Box::new(OpenLinkUnderPointer))
                    .when(default_app, |menu| {
                        menu.menu(
                            t(L10nKey::AppMenuOpenLinkWithDefaultApp),
                            Box::new(OpenLinkWithDefaultApp),
                        )
                    })
                    // A file on another machine has no folder here to
                    // show it in, and naming this one's would show
                    // whatever it happens to keep at that path.
                    .menu_element_with_disabled(
                        Box::new(RevealLinkUnderPointer),
                        !local,
                        menu_row_with_hint(t(reveal), None),
                    )
                    .menu(
                        t(L10nKey::AppMenuCopyLinkPath),
                        Box::new(CopyLinkPathUnderPointer),
                    )
                    .separator()
            }
            None => menu,
        };
        let menu = menu
            .min_w(px(240.))
            .action_context(menu_focus.clone())
            .menu_element_with_disabled(
                Box::new(CopyText),
                !has_selection,
                menu_row_with_hint(t(L10nKey::AppMenuCopy), mac_only("secondary-c")),
            )
            .menu_element_with_disabled(
                Box::new(CutText),
                !has_selection,
                menu_row_with_hint(t(L10nKey::AppMenuCut), mac_only("secondary-x")),
            )
            .menu_element(
                Box::new(PasteText),
                menu_row_with_hint(t(L10nKey::AppMenuPaste), mac_only("secondary-v")),
            )
            .menu_element(
                Box::new(SelectAll),
                menu_row_with_hint(t(L10nKey::AppMenuSelectAll), mac_only("secondary-a")),
            )
            .separator()
            .menu(t(L10nKey::AppMenuFind), Box::new(FindInTerminal))
            // Inside the terminal, what gets cleared goes without
            // saying; the menu bar keeps the full "Clear Scrollback".
            .menu(t(L10nKey::TerminalContextClear), Box::new(ClearScrollback));

        // `fork_label` is anterm-core's capability probe, and core has no
        // locale table — take the answer, not its English wording.
        let can_fork = view.agent().and_then(|a| a.fork_label()).is_some();
        let fork_ready = can_fork
            && view.remote_context().is_none()
            && view.agent_session().is_some_and(|s| s.session_id.is_some());
        // `None` over a pane with no agent; otherwise whether it knows
        // the command line it was started with.
        let launch_argv_known = view.agent().map(|_| {
            view.agent_session()
                .is_some_and(|s| s.launch_argv.is_some())
        });

        let menu = match (can_fork, fork_ready) {
            (true, true) => {
                let focus = menu_focus.clone();
                menu.separator().submenu(
                    t(L10nKey::AppMenuForkSession),
                    window,
                    cx,
                    move |submenu, _window, _cx| {
                        submenu
                            .action_context(focus.clone())
                            .menu(
                                t(L10nKey::AppMenuSplitRight),
                                Box::new(ForkAgentSessionRight),
                            )
                            .menu(
                                t(L10nKey::AppMenuSplitLeft),
                                Box::new(ForkAgentSessionLeft),
                            )
                            .menu(
                                t(L10nKey::AppMenuSplitDown),
                                Box::new(ForkAgentSessionDown),
                            )
                            .menu(t(L10nKey::AppMenuSplitUp), Box::new(ForkAgentSessionUp))
                    },
                )
            }
            (true, false) => menu
                .separator()
                .item(PopupMenuItem::new(t(L10nKey::AppMenuForkSession)).disabled(true)),
            (false, _) => menu,
        };

        // Only over a running agent, and only live when the pane knows
        // the command line it was started with — there is nothing to
        // save otherwise. Beside Fork when both are there.
        let menu = match launch_argv_known {
            Some(known) => {
                let menu = if can_fork { menu } else { menu.separator() };
                menu.menu_with_disabled(
                    t(L10nKey::AppMenuSaveAgentLaunchArgs),
                    Box::new(SaveAgentLaunchArgs),
                    !known,
                )
            }
            None => menu,
        };

        menu.separator()
            .menu(t(L10nKey::AppMenuSplitRight), Box::new(SplitRight))
            .menu(t(L10nKey::AppMenuSplitLeft), Box::new(SplitLeft))
            .menu(t(L10nKey::AppMenuSplitDown), Box::new(SplitDown))
            .menu(t(L10nKey::AppMenuSplitUp), Box::new(SplitUp))
            .menu(t(L10nKey::AppMenuZoomPane), Box::new(ToggleMaximizePane))
            .separator()
            .menu(t(L10nKey::AppMenuNewTab), Box::new(NewTab))
            .menu(t(L10nKey::AppMenuClosePaneTab), Box::new(CloseActiveTab))
    }
    /// Mount the in-pane search bar as a cached fixed-size overlay entity so
    /// high-frequency canvas repaints reuse its retained subtree.
    fn render_search_overlay(&self) -> Option<gpui::AnyElement> {
        self.search_state.active.as_ref()?;
        let mut style = gpui::StyleRefinement::default();
        style.size.width = Some(px(super::search::BAR_WIDTH).into());
        style.size.height = Some(px(super::search::BAR_HEIGHT).into());
        Some(
            div()
                .absolute()
                .top(px(super::search::BAR_TOP))
                .right_4()
                .child(self.search_bar.clone().cached(style))
                .into_any_element(),
        )
    }

    /// Mount the agent composer entity either docked below the canvas or covering
    /// the bottom rows, caching it once its natural height is measured and no
    /// popup/picker is open.
    fn render_composer_overlays(
        &self,
        cx: &Context<Self>,
    ) -> (Option<gpui::AnyElement>, Option<gpui::AnyElement>) {
        let presence = self.composer_presence();
        if matches!(
            presence,
            super::composer::Presence::Hidden | super::composer::Presence::SteppedAside
        ) {
            return (None, None);
        }
        let measured = self.composer_view.read(cx).measured_size.get();
        let composer_el = if let Some((_, h)) = measured.filter(|_| !self.composer_has_popup(cx)) {
            let mut style = gpui::StyleRefinement::default();
            style.size.width = Some(gpui::relative(1.).into());
            style.size.height = Some(h.into());
            self.composer_view.clone().cached(style).into_any_element()
        } else {
            self.composer_view.clone().into_any_element()
        };
        match presence {
            super::composer::Presence::Docked => (
                Some(div().flex_none().w_full().child(composer_el).into_any_element()),
                None,
            ),
            super::composer::Presence::Covering(_) => (
                None,
                Some(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(composer_el)
                        .into_any_element(),
                ),
            ),
            super::composer::Presence::Hidden | super::composer::Presence::SteppedAside => {
                (None, None)
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prepare_render_frame(window, cx);

        let focused = self.focus_handle.is_focused(window);
        let input_active = self.input_active();
        let search_bar = self.render_search_overlay();
        let (composer_docked, composer_over) = self.render_composer_overlays(cx);

        let surface = div()
            .id("terminal-surface")
            // The surface, not the grid inside it, is what carries the role:
            // a11y focus is only ever reported for a `div` that tracks a focus
            // handle *and* has a node of its own, so a terminal with no role
            // here is a window whose focused element is the window.
            .role(gpui::Role::MultilineTextInput)
            .track_focus(&self.focus_handle)
            .key_context(self.key_context())
            .size_full()
            .relative()
            // A column so the composer docks under the grid and takes its rows
            // from it: the grid's laid-out height is what sizes the pty.
            .flex()
            .flex_col()
            .overflow_hidden()
            .px(px(GRID_PAD_X))
            .py(px(GRID_PAD_Y))
            .text_color(cx.theme().foreground);

        let menu_focus = self.focus_handle.clone();
        let has_selection = self.any_selection();
        let menu_view = cx.entity();

        self.attach_surface_actions(surface, cx)
            .child(self.canvas.clone())
            .children(composer_docked)
            .children(search_bar)
            .children(input_active.then(|| self.render_input_bar(focused, cx)).flatten())
            .children(input_active.then(|| self.render_completion_menu(cx)).flatten())
            .children(
                input_active
                    .then(|| self.render_reverse_search_menu(cx))
                    .flatten(),
            )
            .children(self.render_integration_notice(cx))
            .children(self.render_remote_completion_notice(cx))
            .children(self.render_helix_prompt_badge(cx))
            .children(composer_over)
            .context_menu(move |menu, window, cx| {
                Self::build_context_menu(menu, &menu_view, &menu_focus, has_selection, window, cx)
            })
    }
}
