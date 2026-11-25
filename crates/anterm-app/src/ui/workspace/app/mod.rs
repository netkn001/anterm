use gpui::{
    App, Axis, Bounds, Context, Edges, Entity, Pixels, Size, Subscription, Window, div, img, point,
    prelude::*, px, size,
};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{ActiveTheme as _, TitleBar};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::core::config::{
    Config, CursorStyle as ConfigCursorStyle, RightPanelTab, TabBarPosition,
};
use crate::core::session::{SessionPane, SessionTab};
use crate::core::shells::ShellInventory;
use crate::daemon::protocol::ShellSpec;
use crate::terminal::view::TerminalView;
use anterm_i18n::{L10nKey, t, t_fmt};
use crate::ui::workspace::pane::{Dir, Pane};
use crate::ui::chrome::search::SearchView;

mod actions;
mod lifecycle;
mod search_and_agents;
mod session_spawn;
mod settings_bridge;
mod tabs;
mod theme_bridge;
#[cfg(test)]
mod tests;

pub(crate) use session_spawn::{
    join_shell_args, new_terminal,
};
#[cfg(test)]
pub(crate) use session_spawn::parse_ssh_option_words;
#[cfg(test)]
pub(crate) use tests::test_window;

/// What to start in a pane that is about to be opened.
pub(crate) enum SpawnAs {
    /// A local shell; `None` is whatever the default one is.
    Shell(Option<ShellSpec>),
    Ssh(Box<crate::daemon::protocol::NativeSshSpec>),
}

/// How a pane being opened relates to the pane it was opened from — which is
/// what decides whether it may start in that pane's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenedFrom {
    /// A local shell.
    LocalShell,
    /// The source pane's own SSH host, dialled again (⌘T, ⌘D on an SSH pane).
    SameSshHost,
    /// A saved host or quick connect: a connection of its own, whatever pane
    /// happened to be in front when it was asked for.
    FreshSshHost,
}

/// The directory a pane opened from another one starts in.
///
/// `source_cwd` is the source pane's last reported directory, and on a remote
/// pane that is a path on the far host. It carries over only to a pane on the
/// same side of the link: a local shell takes a local pane's directory, and a
/// redial of the same host takes the far shell's. Anything else starts where a
/// fresh pane would — a local path handed to a remote `cd` names nothing there,
/// and a remote one handed to a local spawn names nothing here.
fn inherited_start_dir(
    opened: OpenedFrom,
    source_cwd: Option<std::path::PathBuf>,
    source_is_remote: bool,
) -> Option<std::path::PathBuf> {
    match opened {
        OpenedFrom::LocalShell => source_cwd.filter(|_| !source_is_remote),
        OpenedFrom::SameSshHost => source_cwd,
        OpenedFrom::FreshSshHost => None,
    }
}

/// Where a row taken out of the new-tab menu lands.
///
/// Windows Terminal's rule, and the reason this is a parameter rather than two
/// menus: the same list of shells and hosts serves both, and ⌥ at click time
/// picks between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnWhere {
    NewTab,
    /// Beside the pane in front of the user, on [`Axis::Horizontal`] — the
    /// side `SplitRight` already puts a pane on. Deliberately not Windows
    /// Terminal's aspect-ratio guess: a split that lands somewhere different
    /// depending on the shape of the pane cannot be aimed.
    Split,
}

impl SpawnWhere {
    /// Read off the live keyboard state at click time. The menu hands its
    /// handler a synthetic click with no modifiers on it, so there is nowhere
    /// else the ⌥ the user is holding can be found.
    pub(crate) fn from_modifiers(mods: gpui::Modifiers) -> Self {
        if mods.alt {
            SpawnWhere::Split
        } else {
            SpawnWhere::NewTab
        }
    }
}


// The steppers clamp to the same range `sanitize` allows, defined in
// anterm-core next to the validation: a local, narrower pair used to push a
// config-legal value the wrong way — `font_size: 50` shrank to 48 on "+",
// and the result was written back to the file (#550).
pub(crate) use crate::core::config::{
    FONT_SIZE_MAX, FONT_SIZE_MIN,
};

pub(crate) use anterm_ui_kit::intent::FONT_SIZE_STEP;

const MAX_CLOSED_TABS: usize = 20;

/// How long ⌘⇧T waits for this window's queued edits to reach the machine
/// before asking it for the newest closed tab: polls of [`REOPEN_SETTLE_POLL`].
/// A close is one round trip; two seconds covers a slow link without making a
/// wedged one hang the keypress.
const REOPEN_SETTLE_POLLS: u32 = 40;
const REOPEN_SETTLE_POLL: std::time::Duration = std::time::Duration::from_millis(50);

const RESIZE_STEP: f32 = 0.05;


pub(crate) use crate::ui::chrome::window_chrome::{
    CONTENT_INSET, TILE_GLYPH, TILE_SIZE, TITLE_BAR_BORDER, TITLE_BAR_HEIGHT, TITLE_BAR_LEAD,
    TitleStripRow, tile_trailing_inset, title_bar_drag, title_bar_hug_offset,
    window_background_image_layer,
};
#[cfg(test)]
pub(crate) use crate::ui::chrome::window_chrome::{TERMINAL_MIN_W, WINDOW_CONTROLS_W, title_strip_centre, title_strip_rule_and_pad};
#[cfg(test)]
pub(crate) use anterm_ui_kit::intent::{split_shell_args, wd_path_saveable};
#[cfg(test)]
pub(crate) use anterm_ui_kit::ssh_helpers::parse_ssh_connect_input;
#[cfg(test)]
pub(crate) use crate::ui::editor::document_column::{DOCUMENT_MIN_W, document_column_px};

/// Half the period of the home page's cursor, near enough the terminal's own
/// 530ms that the two do not read as different clocks.
pub(crate) const HOME_CURSOR_BLINK_MS: u64 = 600;

pub(crate) use anterm_ui_kit::window_chrome::side_panel_max;

/// The centre of the box a row built by [`TitleStripRow`] gives its contents:
/// the bar's height, less whatever its bottom edge spends on rule and padding.
#[cfg(test)]
fn title_strip_content_centre(rule: gpui::Pixels) -> gpui::Pixels {
    let (border, pad) = title_strip_rule_and_pad(rule);
    gpui::px((TITLE_BAR_HEIGHT - border.as_f32() - pad.as_f32()) / 2.)
}

const DOCS_URL: &str = "https://github.com/netkn001/anterm#readme";
const DISCORD_URL: &str = "https://discord.gg/s3dethqz2V";
const ISSUES_URL: &str = "https://github.com/netkn001/anterm/issues/new";

/// The bounds to remember for reopening this window at the same place.
///
/// Under client-side decorations the window's *outer* rectangle is the whole
/// surface, shadow included, and both Linux backends put the shadow back on
/// their own once the window is up: Wayland's first sized configure adds the
/// inset to whatever geometry was asked for (`compute_outer_size`), and on X11
/// the inset travels as `_GTK_FRAME_EXTENTS`, which a window manager only
/// advertises — and gpui only turns CSD on for — when it honours those extents
/// by keeping the *visible* frame put. Save the outer rectangle either way and
/// it comes back twice the shadow larger every launch; that is the bug Zed
/// fixed in ca9cee85e1 ("linux: Fix non-maximized Zed windows growing larger
/// across sessions", #22301), whose own measurements were taken on X11.
///
/// So save the inner rectangle, exactly as Zed does, on every platform. It is
/// the visible frame, which is what the backends re-inflate back to, and it
/// costs nothing elsewhere: `inner_window_bounds` defaults to `window_bounds`
/// on the `PlatformWindow` trait, and neither the macOS nor the Windows backend
/// nor gpui's `TestWindow` overrides it. On X11 without a compositor the same
/// holds for a different reason — `window_decorations()` answers `Server`, the
/// window border never calls `set_client_inset`, and the insets stay zero.
fn window_bounds_to_remember(window: &Window) -> Bounds<Pixels> {
    window.inner_window_bounds().get_bounds()
}

/// The band the tab strip claims for a drop, in the window's outer coordinates.
///
/// The strip is the top of the *content*, and under client-side decorations the
/// content sits inside the frame padding while `viewport` still measures the
/// whole surface, shadow included — so the band starts at the padding and stops
/// at the far edge of the frame, one padding in from each side. Anything else
/// leaves the outermost chips outside the zone that is supposed to contain
/// them. `window_paddings` answers `Edges::all(0)` under server-side
/// decorations, which is every platform but Linux CSD, so off Linux this stays
/// the full-width rectangle from the window's corner it has always been.
fn strip_band(viewport: Size<Pixels>, pad: Edges<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(pad.left, pad.top),
        size: size(
            (viewport.width - pad.left - pad.right).max(px(0.)),
            px(TITLE_BAR_HEIGHT),
        ),
    }
}

pub(crate) const WINDOW_MARK_SIZE: f32 = 20.;

/// A transparent sheet that answers one question: is the pointer inside the
/// box it covers. Lay it over a region as that region's *last* child and read
/// the flag to reveal chrome only while the pointer is there.
///
/// The obvious way to write this is `group_hover` on the region itself, and it
/// does not work. Group hover asks whether the group's *hitbox* is the one
/// under the pointer, and gpui's hit test stops at the first `occlude()`d
/// element it meets on the way down. Tab chips and the chrome tiles are all
/// occluding, so the region stopped counting as hovered the instant the
/// pointer reached the very button it was revealing, and the button vanished
/// from under the cursor. Painted last, this sheet's own hitbox sits in front
/// of all of them, and it blocks nothing — it is not opaque, so the rows,
/// chips and tiles underneath keep their clicks, cursors and tooltips.
pub(crate) fn hover_sheet(id: &'static str, flag: &Rc<Cell<bool>>) -> gpui::Stateful<gpui::Div> {
    use gpui::{InteractiveElement as _, StatefulInteractiveElement as _};
    let flag = flag.clone();
    gpui::div()
        .id(id)
        .absolute()
        .inset_0()
        .on_hover(move |over, window, _cx| {
            if flag.get() != *over {
                flag.set(*over);
                window.refresh();
            }
        })
}

/// Whether a group of title-bar tiles paints this frame: always, unless
/// `auto_hide_titlebar_buttons` is on, and then only while the pointer is over
/// the bar the group sits in.
pub(crate) fn titlebar_chrome_shown(auto_hide: bool, pointer_over_bar: bool) -> bool {
    !auto_hide || pointer_over_bar
}

/// Take the paint off a chrome tile that [`titlebar_chrome_shown`] says is
/// resting, and nothing else.
///
/// Opacity, not `invisible()`. Both keep the tile's slot, so revealing it never
/// shifts what is beside it. But gpui skips a hidden element's whole paint
/// pass, and that is where its click handler, its accessibility actions and
/// its tooltip are registered: a hidden tile still shows up to a screen reader,
/// by label, from prepaint, and then does nothing when pressed from there. At
/// zero opacity the tile is fully there and merely not drawn.
pub(crate) fn resting_chrome<E: gpui::Styled>(tile: E, shown: bool) -> E {
    match shown {
        true => tile,
        false => tile.opacity(0.),
    }
}



pub(crate) fn window_mark() -> Option<impl IntoElement> {
    if cfg!(target_os = "macos") {
        return None;
    }
    static LOGO: std::sync::OnceLock<Arc<gpui::Image>> = std::sync::OnceLock::new();
    let logo = LOGO
        .get_or_init(|| {
            Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/logo@256.png")).to_vec(),
            ))
        })
        .clone();
    Some(img(logo).size(px(WINDOW_MARK_SIZE)).flex_shrink_0())
}

pub struct Tab {
    pub pane: Pane,
    pub name: Option<String>,
    last_focused: Option<gpui::EntityId>,
    /// The pane zoomed in this tab, stashed here by `activate` while another
    /// tab is on screen — zoom is a tab's view state, not the window's, so
    /// looking at another tab and coming back must not lose it (#599). `None`
    /// while the tab is active: then the zoom lives in `AntermApp::maximized`.
    pub(crate) zoomed: Option<Entity<TerminalView>>,
    /// User-assigned tags attached to this tab.
    pub(crate) tags: std::cell::RefCell<Vec<crate::core::tag::TagId>>,
    pub(crate) tree_id: std::cell::Cell<anterm_core::machine::TabId>,
    /// Monotonic stamp of when this tab was last activated, used to order the
    /// switcher's tab column most-recently-used first. Zero means never.
    pub(crate) last_used: std::cell::Cell<u64>,
    /// Where a directional focus move started, keyed by the pane it landed on
    /// and the direction that undoes it, so reversing a move comes back here
    /// instead of wherever geometry ranks first (#738). Per tab because the
    /// panes are.
    ///
    /// Keyed by pane and not by direction alone: one slot per direction is
    /// overwritten by the next move the same way, so a walk of two steps left
    /// and two back right ends somewhere other than it started — the very drift
    /// this is here to stop. A pane remembering its own way in retraces the
    /// whole walk.
    ///
    /// A recorded pane only ever breaks a tie between the panes already next to
    /// the one focus is leaving, so an entry left over from an older layout — or
    /// from before a click moved focus somewhere else entirely — can at worst
    /// pick a different neighbour, never a distant one. Entries naming a pane
    /// the tab no longer holds are dropped as the next one is written, so a
    /// closed pane leaves nothing behind either.
    focus_origin: std::collections::HashMap<(gpui::EntityId, Dir), gpui::EntityId>,
    /// Set while the tab is asleep (#762): its panes were stopped and `pane`
    /// is `Pane::Empty`. What is kept instead is what it takes to bring them
    /// back — see [`Asleep`].
    pub(crate) asleep: Option<Asleep>,
}

/// A sleeping tab: nothing of it runs, and everything a wake needs is here.
///
/// `layout` is the tab as the restore path reads a tab — pane ids, cwds,
/// shells, agent sessions — so waking is that restore, run on one tab: each
/// pane's id asks the daemon for the screen it stored when the pane was
/// stopped, and an agent with a known session is resumed the way a reboot
/// resumes it. `view` is what the tab is called in the meantime, read off
/// the panes as they went to sleep (or off the tree's records, after a
/// restart), since there is no terminal left to ask.
pub(crate) struct Asleep {
    pub(crate) layout: SessionPane,
    pub(crate) view: anterm_core::tab_view::TabView,
    pub(crate) home: Option<std::path::PathBuf>,
}

impl Tab {
    pub(crate) fn new(pane: Pane) -> Self {
        Self {
            pane,
            name: None,
            last_focused: None,
            zoomed: None,
            tags: std::cell::RefCell::new(Vec::new()),
            tree_id: std::cell::Cell::new(anterm_core::machine::TabId::new()),
            last_used: std::cell::Cell::new(0),
            focus_origin: Default::default(),
            asleep: None,
        }
    }

    pub(crate) fn from_tree(tree: &anterm_core::machine::Tab, pane: Pane) -> Self {
        Self {
            pane,
            name: tree.name.clone(),
            last_focused: None,
            zoomed: None,
            tags: std::cell::RefCell::new(tree.tags.clone()),
            tree_id: std::cell::Cell::new(tree.id),
            last_used: std::cell::Cell::new(0),
            focus_origin: Default::default(),
            asleep: None,
        }
    }

    pub(crate) fn is_asleep(&self) -> bool {
        self.asleep.is_some()
    }

    /// The layout a sleeping tab will wake into; `None` for a tab that is awake.
    pub(crate) fn asleep_layout(&self) -> Option<&SessionPane> {
        self.asleep.as_ref().map(|a| &a.layout)
    }

    /// The agent a sleeping tab was running when it went to sleep, so its row
    /// still says what it is while nothing in it can.
    pub(crate) fn asleep_agent(&self) -> Option<crate::core::cli_agent::CLIAgent> {
        self.asleep.as_ref().and_then(|a| a.view.agent)
    }

    fn focus_target(&self) -> Option<crate::ui::workspace::pane::PaneSlot> {
        match self.last_focused {
            Some(id) => self.pane.leaf_matching_or_first(|l| l.entity_id() == id),
            None => self.pane.first_leaf(),
        }
    }

    /// The pane a move in `dir` out of `at` should return to, if it reverses
    /// the move that brought focus to `at`.
    fn focus_origin(&self, at: gpui::EntityId, dir: Dir) -> Option<gpui::EntityId> {
        self.focus_origin.get(&(at, dir)).copied()
    }

    /// Remember that a move in `dir` carried focus from `from` to `to`, so the
    /// move back out of `to` returns. `live` names the panes the tab holds now:
    /// anything the layout has moved on from is forgotten here rather than
    /// accumulating for the life of the window.
    fn remember_focus_origin(
        &mut self,
        from: gpui::EntityId,
        to: gpui::EntityId,
        dir: Dir,
        live: &[gpui::EntityId],
    ) {
        self.focus_origin
            .retain(|(at, _), origin| live.contains(at) && live.contains(origin));
        self.focus_origin.insert((to, dir.opposite()), from);
    }

    pub(crate) fn detail_pane(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> Option<Entity<TerminalView>> {
        self.pane
            .focused_leaf(window, cx)
            .or_else(|| self.focus_target())
            .and_then(|slot| slot.terminal().cloned())
    }

    /// The leaf a tab names itself after — its title and, with it, the home
    /// that title's path is measured against.
    fn title_leaf(&self, window: Option<&Window>, cx: &App) -> Option<Entity<TerminalView>> {
        let leaf = match window {
            Some(window) => self
                .pane
                .focused_leaf(window, cx)
                .or_else(|| self.focus_target()),
            None => self.focus_target(),
        };
        leaf.and_then(|l| l.terminal().cloned())
    }

    /// This tab as the shared label ladder reads it, together with what a `~`
    /// in whatever it ends up named would mean.
    ///
    /// [`TabView`](anterm_core::tab_view::TabView) is how a tab looks to
    /// someone who is *not* the window showing it — the switcher listing
    /// another window's workspace, `anterm tab ls` on the far side of a socket.
    /// Building one here from the live pane is what stops this window having a
    /// second opinion: both sides then rank a given name, a title, an agent and
    /// a directory through
    /// [`TabView::label`](anterm_core::tab_view::TabView::label), so the
    /// strip's answer to "which repo is this?" is the switcher's answer too.
    ///
    /// Everything comes off the one leaf the tab names itself after, so the
    /// title and the directory standing in for it can never describe different
    /// panes (#580).
    pub(crate) fn label_view(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> (
        anterm_core::tab_view::TabView,
        Option<std::path::PathBuf>,
    ) {
        let name = self.name.clone();
        if let Some(asleep) = &self.asleep {
            let view = anterm_core::tab_view::TabView {
                id: self.tree_id.get(),
                name,
                ..asleep.view.clone()
            };
            return (view, asleep.home.clone());
        }
        let Some(leaf) = self.title_leaf(window, cx) else {
            return (
                anterm_core::tab_view::TabView {
                    id: self.tree_id.get(),
                    name,
                    title: String::new(),
                    osc_title: None,
                    cwd: None,
                    agent: None,
                    status: None,
                    live: false,
                    panes: 0,
                },
                None,
            );
        };
        let leaf = leaf.read(cx);
        let view = anterm_core::tab_view::TabView {
            id: self.tree_id.get(),
            name,
            // The tree's `title` is the foreground process name — what it falls
            // back on once a pane has said nothing about itself. A live pane's
            // equivalent is the placeholder it answers to unprompted: any
            // *other* default it was given (an SSH host, a workspace name) is a
            // name anterm chose for it deliberately, and `stated_title` hands
            // those up as the title the pane is showing.
            title: crate::terminal::view::DEFAULT_TITLE.to_string(),
            osc_title: leaf.stated_title().map(str::to_string),
            cwd: leaf.cwd().map(|p| p.display().to_string()),
            agent: leaf.agent(),
            status: leaf.agent_session().map(|s| s.status),
            live: !leaf.terminal.exited,
            panes: self.pane.terminals().len(),
        };
        (view, leaf.display_home(cx))
    }

    pub(crate) fn git_status(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<crate::git::status::GitStatus> {
        let leaf = match window {
            Some(window) => self.pane.focused_or_first(window, cx),
            None => self.pane.first_leaf().and_then(|s| s.terminal().cloned()),
        }?;
        leaf.read(cx).git_status(cx)
    }

    pub(crate) fn agent(&self, cx: &App) -> Option<crate::core::cli_agent::CLIAgent> {
        self.pane
            .terminals()
            .into_iter()
            .find_map(|l| l.read(cx).agent())
    }

    /// The tab's most urgent agent leaf, named and reported by that one leaf.
    ///
    /// `agent` and `agent_status` answer independently — the *first* leaf
    /// carrying an agent, and the highest urgency found *anywhere* in the tab
    /// — so reading them as a pair can put one pane's name beside another
    /// pane's state, a row no leaf ever had. Anywhere both halves are shown
    /// at once reads them from here instead (#543).
    pub(crate) fn agent_row(
        &self,
        cx: &App,
    ) -> Option<(
        crate::core::cli_agent::CLIAgent,
        crate::core::cli_agent::AgentStatus,
    )> {
        use crate::core::cli_agent::AgentStatus;
        let urgency = |s: AgentStatus| match s {
            AgentStatus::Waiting => 3,
            AgentStatus::Working => 2,
            AgentStatus::Done => 1,
            AgentStatus::Idle => 0,
        };
        self.pane
            .terminals()
            .into_iter()
            .filter_map(|l| {
                let view = l.read(cx);
                let agent = view.agent()?;
                // A pane whose agent is running but has never reported a
                // session reads as idle, the same reading the badge has always
                // given it.
                let status = view
                    .agent_session()
                    .map(|s| s.status)
                    .unwrap_or(AgentStatus::Idle);
                Some((agent, status))
            })
            .max_by_key(|(_, status)| urgency(*status))
    }

    pub(crate) fn agent_status(&self, cx: &App) -> Option<crate::core::cli_agent::AgentStatus> {
        self.agent_row(cx).map(|(_, status)| status)
    }

    pub(crate) fn agent_unread_count(&self, cx: &App) -> usize {
        use crate::core::cli_agent::AgentStatus;
        if self.agent_status(cx) != Some(AgentStatus::Done) {
            return 0;
        }
        self.pane
            .terminals()
            .into_iter()
            .filter(|l| {
                let v = l.read(cx);
                v.agent_session().map(|s| s.status) == Some(AgentStatus::Done)
                    && v.agent_result_unread()
            })
            .count()
    }
}

/// What a rename box's contents mean for the tab's name.
#[derive(Debug, PartialEq, Eq)]
enum Rename {
    /// The box still holds what it was seeded with, so nothing was asked for.
    Unchanged,
    /// Emptied on purpose: the tab goes back to following its pane.
    Cleared,
    /// A name the user typed.
    Named(String),
}

/// Read a rename box against the label it was seeded with.
///
/// The box is prefilled with the label as rendered, so it is never empty when
/// it opens and a commit cannot tell "left alone" from "typed the same thing"
/// by looking at the value alone. `Blur` commits as readily as Enter does, so
/// without the comparison, opening the box and clicking away stored the label
/// as a name — and a name is a different thing from the title it was copied
/// from: it stops following the pane, freezing the tab on whatever it happened
/// to say at that moment, with no way to undo it.
fn rename_outcome(value: &str, prefill: &str) -> Rename {
    let value = value.trim();
    match value {
        v if v == prefill.trim() => Rename::Unchanged,
        "" => Rename::Cleared,
        v => Rename::Named(v.to_string()),
    }
}

pub(crate) struct Renaming {
    /// The tab being renamed, by tree id rather than index: an index drifts
    /// the moment any other tab closes or the strip reorders, which used to
    /// force every unrelated tab event to throw the half-typed name away —
    /// and left a window where the commit landed on the wrong tab (#598).
    pub(crate) tab: anterm_core::machine::TabId,
    pub(crate) input: Entity<InputState>,
    /// What the box was prefilled with, so a commit can tell an untouched box
    /// from a typed one. The box is seeded with the label already on screen,
    /// which means it is never empty and every commit would otherwise store a
    /// name — including the ones the user never typed.
    prefill: String,
    _subs: Vec<Subscription>,
}

pub(crate) struct TagRename {
    /// The tag being renamed. By id, so a rename that lands after another
    /// window edited the catalog still names this tag.
    pub(crate) tag: crate::core::tag::TagId,
    pub(crate) input: Entity<InputState>,
    pub(crate) _subs: Vec<Subscription>,
}

#[cfg(test)]
pub(crate) mod chrome_render_probe {
    use std::cell::Cell;

    thread_local! {
        static SIDEBAR_RENDERS: Cell<u64> = const { Cell::new(0) };
        static STRIP_RENDERS: Cell<u64> = const { Cell::new(0) };
    }

    pub(crate) fn reset() {
        SIDEBAR_RENDERS.set(0);
        STRIP_RENDERS.set(0);
    }

    pub(crate) fn record_sidebar() {
        SIDEBAR_RENDERS.set(SIDEBAR_RENDERS.get() + 1);
    }

    pub(crate) fn record_strip() {
        STRIP_RENDERS.set(STRIP_RENDERS.get() + 1);
    }

    pub(crate) fn sidebar_renders() -> u64 {
        SIDEBAR_RENDERS.get()
    }

    pub(crate) fn strip_renders() -> u64 {
        STRIP_RENDERS.get()
    }
}

/// Cached GPUI view hosting the left tab sidebar.
///
/// Rendered via `.cached(style)` in [`AntermApp::render`] so high-frequency
/// [`crate::terminal::view::TerminalCanvas`] output repaints reuse the
/// sidebar's previous prepaint and paint output instead of re-measuring every
/// tab title/path/branch on every frame.
pub(crate) struct TabSidebarHost {
    pub(super) app: gpui::WeakEntity<AntermApp>,
}

impl Render for TabSidebarHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        chrome_render_probe::record_sidebar();
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, app_cx| {
            app.tab_sidebar(window, app_cx).into_any_element()
        })
    }
}

/// Cached GPUI view hosting the top title-bar tab strip.
pub(crate) struct TabStripHost {
    pub(super) app: gpui::WeakEntity<AntermApp>,
    pub(super) show_chips: Rc<Cell<bool>>,
}

impl Render for TabStripHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        chrome_render_probe::record_strip();
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let show_chips = self.show_chips.get();
        div()
            .size_full()
            .min_w_0()
            .flex()
            .items_center()
            .child(app.update(cx, |app, app_cx| {
                app.tab_strip(show_chips, window, app_cx).into_any_element()
            }))
            .into_any_element()
    }
}

/// Last-applied terminal typography & behavior settings, held for diffing
/// inside `reload_from_config` so unchanged panes are not updated.
pub(crate) struct TerminalConfigSnapshot {
    pub(crate) line_height: f32,
    pub(crate) font_family: String,
    pub(crate) font_family_bold: Option<String>,
    pub(crate) font_family_italic: Option<String>,
    pub(crate) font_fallbacks: Vec<String>,
    pub(crate) font_features: Option<gpui::FontFeatures>,
    pub(crate) cursor_style: ConfigCursorStyle,
    pub(crate) scrollback_limit: usize,
}

impl TerminalConfigSnapshot {
    pub(crate) fn from_config(cfg: &Config) -> Self {
        Self {
            line_height: cfg.line_height,
            font_family: cfg.font_family.clone(),
            font_family_bold: cfg.font_family_bold.clone(),
            font_family_italic: cfg.font_family_italic.clone(),
            font_fallbacks: cfg.font_fallbacks.clone(),
            font_features: cfg
                .font_features
                .as_ref()
                .map(crate::core::config::gpui_font_features),
            cursor_style: cfg.cursor_style,
            scrollback_limit: cfg.scrollback_limit,
        }
    }
}

pub struct AntermApp {
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    tab_use_seq: std::cell::Cell<u64>,
    pending_tab: Option<anterm_core::machine::TabId>,
    pub(crate) font_size: f32,
    pub(crate) applied_terminal_config: TerminalConfigSnapshot,
    _subscriptions: Vec<Subscription>,
    pub(crate) search: Option<Entity<SearchView>>,
    theme_preview_restore: Option<String>,
    pub(crate) closed: Vec<SessionTab>,
    pub(crate) renaming: Option<Renaming>,
    pub(crate) maximized: Option<Entity<TerminalView>>,
    pub(crate) mod_hints: crate::ui::workspace::hints::ModHintState,
    pub(crate) home_focus: gpui::FocusHandle,
    pub(crate) home_cursor_on: bool,
    pub(crate) working_dot_on: bool,
    pub(crate) shells: ShellInventory,
    pub(crate) right_panel: Entity<crate::ui::files::right_panel::RightPanelView>,
    pub(crate) editor: Entity<crate::ui::editor::CodeEditorWorkspace>,
    pub(crate) sidebar_width: Rc<Cell<f32>>,
    pub(crate) sidebar_dragging: Rc<Cell<bool>>,
    pub(crate) sidebar_chrome_hover: Rc<Cell<bool>>,
    pub(crate) strip_chrome_hover: Rc<Cell<bool>>,
    pub(crate) sidebar_collapsed: bool,
    pub(crate) sidebar_scroll: gpui_kit::component::VirtualListScrollHandle,
    pub(crate) sidebar_reveal: Rc<Cell<Option<anterm_core::machine::TabId>>>,
    pub(crate) reorder: Rc<RefCell<Option<crate::ui::workspace::reorder::Reorder>>>,
    pub(crate) pane_hover: Rc<Cell<Option<gpui::EntityId>>>,
    pub(crate) pane_drag: crate::ui::workspace::pane_drag::PaneDragState,
    pub(crate) tab_merge: Cell<
        Option<(
            anterm_core::machine::TabId,
            crate::ui::workspace::pane_drag::DropZone<gpui::EntityId>,
        )>,
    >,
    pub(crate) pane_detach: Cell<Option<(gpui::EntityId, usize)>>,
    pub(crate) strip_slots: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    pub(crate) sidebar_slots: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    pub(crate) sidebar_tags: crate::core::tag::SidebarTags,
    pub(crate) active_tag_filter: Option<crate::core::tag::TagId>,
    pub(crate) pane_area: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Handle to the autonomous Settings window when open.
    pub(crate) settings_window: Option<crate::ui::chrome::settings::window::SettingsWindowHandle>,
    pub(crate) ssh_prompt: Entity<crate::ui::remote::ssh_prompt::SshPromptModal>,
    close_prompt_open: bool,
    quit_prompt_open: bool,
    window_bounds: Bounds<Pixels>,
    pub(crate) tag_rename: Option<TagRename>,
    window_title: std::cell::RefCell<String>,
    pub(crate) startup_error: Option<gpui::SharedString>,
    pub(super) sidebar_host: Entity<TabSidebarHost>,
    pub(super) strip_host: Entity<TabStripHost>,
}

/// What a raised close question is about. Tabs are named by their id, not their
/// index: a tab that exits on its own while the question is on screen shifts
/// every index after it, and answering "Close" must not then end a bystander.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseTarget {
    Tab(anterm_core::machine::TabId),
    Pane,
    /// Close Other Tabs, keeping this one.
    OtherTabs(anterm_core::machine::TabId),
    /// Close Tabs to the Right of this one.
    TabsRightOf(anterm_core::machine::TabId),
}

/// Why closing needs a question first. Closing a tab is the highest-frequency
/// destructive key in any terminal, and the product's headline claim is that
/// shells outlive the app — so the one action that permanently ends one has to
/// name what it is about to end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CloseReason {
    LiveSsh,
    Busy(crate::terminal::view::PaneBusy),
    /// Nothing is running; the user asked to be asked anyway
    /// ([`ConfirmClose::Always`](crate::core::config::ConfirmClose::Always)).
    Idle,
    /// A bulk close of this many tabs, asked about once for all of them.
    Tabs(usize),
}

/// Whether closing one pane needs a question, and which.
///
/// The SSH warning comes first and is not governed by `mode`: it is an opt-in
/// of its own, and `Never` must not quietly switch it off.
fn close_reason(
    mode: crate::core::config::ConfirmClose,
    warn_ssh: bool,
    busy: Option<crate::terminal::view::PaneBusy>,
) -> Option<CloseReason> {
    use crate::core::config::ConfirmClose;
    if warn_ssh {
        return Some(CloseReason::LiveSsh);
    }
    match mode {
        ConfirmClose::Never => None,
        ConfirmClose::WhenBusy => busy.map(CloseReason::Busy),
        ConfirmClose::Always => Some(busy.map_or(CloseReason::Idle, CloseReason::Busy)),
    }
}

/// The question to put to the user before ending work that is still going on.
fn close_prompt(ends_the_tab: bool, reason: &CloseReason) -> (String, String) {
    use crate::terminal::view::PaneBusy;
    use anterm_i18n::L10nKey;
    match reason {
        CloseReason::LiveSsh => (
            t(L10nKey::CloseSshConnectionTitle).to_string(),
            t(L10nKey::CloseSshConnectionBody).to_string(),
        ),
        CloseReason::Busy(busy) => {
            let title = match ends_the_tab {
                true => t(L10nKey::CloseTabBusyTitle),
                false => t(L10nKey::ClosePaneBusyTitle),
            };
            let body = match busy {
                PaneBusy::Command(what) => t_fmt(L10nKey::CloseBusyCommandBody, &[("what", what)]),
                PaneBusy::Agent(name) => t_fmt(L10nKey::CloseBusyAgentBody, &[("agent", name)]),
            };
            (title.to_string(), body)
        }
        CloseReason::Idle => {
            let title = match ends_the_tab {
                true => t(L10nKey::CloseTabBusyTitle),
                false => t(L10nKey::ClosePaneBusyTitle),
            };
            (title.to_string(), t(L10nKey::CloseIdleBody).to_string())
        }
        CloseReason::Tabs(count) => (
            anterm_i18n::t_plural(L10nKey::CloseTabsTitle, *count, &[]),
            t(L10nKey::CloseTabsBody).to_string(),
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ForkPlacement {
    NewTab,
    Split { axis: Axis, before: bool },
}

pub(crate) struct TabAgentSession {
    pub(crate) fork_label: Option<&'static str>,
    pub(crate) session_id: Option<String>,
    pub(crate) remote: bool,
}

impl TabAgentSession {
    pub(crate) fn forkable(&self) -> bool {
        self.fork_label.is_some() && self.session_id.is_some() && !self.remote
    }
}

/// What a full-window overlay (settings, the opened file, the diff view)
/// paints between its own fill and its content.
///
/// Those overlays fill opaquely on purpose, so the OS backdrop cannot show
/// through their text — but that fill also sits on top of the background
/// image the workspace root paints, and would erase it for as long as an
/// overlay is open. So each one repaints the image, then the workspace's own
/// translucent fill over it. That second layer is what keeps the overlay
/// readable: it dims the image to exactly the strength it had when these
/// overlays were themselves translucent, before they were made opaque.
///
/// Empty when the theme has no image — then the opaque fill alone is already
/// what the overlay wants, and a second pass of the same paint buys nothing.


/// Clears the window overrides that are effective on the current platform.
/// `backdrop_is_local` models whether the Windows-only backdrop participates
/// in this platform's rendering and therefore belongs to its reset operation.
#[cfg(test)]
pub(crate) use anterm_ui_kit::intent::clear_window_override_values;

/// The id the fullscreen hint is pushed under, so that entering again replaces
/// it and leaving takes it away.
struct FullscreenHint;

/// Whether the title bar carries minimize, maximize and close right now.
///
/// Not in fullscreen. A fullscreen window has no caption: Windows clears
/// `WS_CAPTION` and answers `HTCLIENT` along the whole top edge, so the three
/// buttons would draw, light up under the pointer and do nothing when clicked.
/// The row they sit at the end of stays, because it is also the tab strip.
///
/// Never on macOS, which draws no buttons of its own: those are the system's
/// traffic lights, and the system hides them itself.
pub(crate) use anterm_ui_kit::window_chrome::{window_controls_drawn, window_controls_w};

impl AntermApp {
    /// Sync the narrow workspace context snapshot into [`CodeEditorWorkspace`].
    pub(crate) fn sync_editor_context(&self, window: Option<&Window>, cx: &mut App) {
        let active_tab = self.tabs.get(self.active).map(|t| t.tree_id.get());
        let live_tabs = self.tabs.iter().map(|t| t.tree_id.get()).collect();
        let active_tab_label = self
            .tabs
            .get(self.active)
            .map(|tab| self.tab_label(tab, self.active, window, cx))
            .unwrap_or_default();
        let spawn_host = self.spawn_host(cx);
        let active_host = self.active_host(cx);
        let active_tree_roots = self.active_tree_roots(cx);
        let left_panel_open = self.left_panel_open(cx);
        let right_panel_open = self.right_panel_open(cx);
        let file_tree = Some(self.right_panel.read(cx).file_tree.downgrade());
        let open_search_tab = self.search.as_ref().map(|s| s.read(cx).tab());
        // Push tab/host metadata first so `document_floor` reflects the active tab
        // before computing laid-out panel widths.
        self.editor.update(cx, |ed, _| {
            ed.set_workspace_context(crate::ui::editor::EditorWorkspaceContext {
                active_tab,
                live_tabs,
                active_tab_label,
                spawn_host,
                active_host,
                active_tree_roots,
                left_panel_open,
                right_panel_open,
                sidebar_px: ed.ctx.sidebar_px,
                right_panel_px: ed.ctx.right_panel_px,
                file_tree,
                open_search_tab,
            });
        });
        if let Some(window) = window {
            let sidebar_px = if self.sidebar_open(cx) {
                self.sidebar_px(window, cx)
            } else {
                0.
            };
            let right_panel_px = if self.right_panel_open(cx) {
                self.right_panel_px(window, cx)
            } else {
                0.
            };
            self.editor.update(cx, |ed, _| {
                ed.ctx.sidebar_px = sidebar_px;
                ed.ctx.right_panel_px = right_panel_px;
            });
        }
    }

    pub(crate) fn handle_editor_event(
        &mut self,
        ev: &crate::ui::editor::CodeEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::editor::CodeEditorEvent;
        use crate::ui::editor::code_editor::AfterUnsaved;
        match ev {
            CodeEditorEvent::FocusActiveTerminal => self.focus_active(window, cx),
            CodeEditorEvent::RememberActivePane => self.remember_active_pane(window, cx),
            CodeEditorEvent::AttachPathToAgent { path, suffix } => {
                self.attach_path_to_agent(path, suffix, cx);
            }
            CodeEditorEvent::OpenSearch { tab, query } => {
                self.open_search(*tab, query, window, cx);
            }
            CodeEditorEvent::OpenSearchLocations { heading, items } => {
                self.open_search(crate::ui::chrome::search::SearchTab::Locations, "", window, cx);
                if let Some(view) = self.search.clone() {
                    let heading = *heading;
                    let items = items.clone();
                    view.update(cx, |view, cx| view.set_locations(heading, items, window, cx));
                }
            }
            CodeEditorEvent::OpenSearchSymbols {
                symbols,
                selected,
                live_query,
            } => {
                self.open_search(crate::ui::chrome::search::SearchTab::Symbols, "", window, cx);
                if let Some(view) = self.search.clone() {
                    let symbols = symbols.clone();
                    let selected = selected.clone();
                    let live_query = live_query.clone();
                    view.update(cx, |view, cx| {
                        view.set_symbols(symbols, selected, window, cx);
                        if let Some(live) = live_query {
                            view.set_live_query(live, window, cx);
                        }
                    });
                }
            }
            CodeEditorEvent::SetProjectSymbols(items) => {
                if let Some(view) = self.search.clone() {
                    if items.is_empty() || view.read(cx).tab() == crate::ui::chrome::search::SearchTab::Symbols {
                        let items = items.clone();
                        view.update(cx, |view, cx| view.set_project_symbols(items, window, cx));
                    }
                }
            }
            CodeEditorEvent::CloseSearch => self.close_search(window, cx),
            CodeEditorEvent::FocusGlobalSearch => {
                self.set_right_panel_tab(RightPanelTab::Files, cx);
                let (panel_search, file_search) = {
                    let rp = self.right_panel.read(cx);
                    (rp.panel_search.clone(), rp.file_search.clone())
                };
                panel_search.update(cx, |s, _| s.request_focus());
                file_search.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }
            CodeEditorEvent::ShowFileTree(path) => self.file_tree_show(path, window, cx),
            CodeEditorEvent::RevealInFileTree(path) => {
                self.file_tree_reveal_path(path, cx);
            }
            CodeEditorEvent::CyclePane => self.cycle_pane(true, window, cx),
            CodeEditorEvent::CodePanelShown { empty } => {
                self.file_tree_refresh_roots(window, cx);
                if *empty {
                    if !self.file_tree_on_screen(cx) {
                        self.set_right_panel_tab(RightPanelTab::Files, cx);
                    }
                    let handle = self
                        .right_panel
                        .read(cx)
                        .file_tree
                        .read(cx)
                        .focus_handle
                        .clone();
                    handle.focus(window, cx);
                }
            }
            CodeEditorEvent::ContinueAfterUnsaved(then) => match then.clone() {
                AfterUnsaved::CloseTab(tab) => {
                    if let Some(ix) = self.tabs.iter().position(|t| t.tree_id.get() == tab) {
                        self.close_tab(ix, window, cx);
                    }
                }
                AfterUnsaved::ClosePane => self.close_pane_after_unsaved(window, cx),
                AfterUnsaved::CloseWindow => self.close_window_after_unsaved(window, cx),
                AfterUnsaved::Quit => self.quit_after_unsaved(window, cx),
                AfterUnsaved::CloseFiles(..) => {}
            },
        }
    }

    pub(crate) fn document_floor(&self, cx: &App) -> f32 {
        self.editor.read(cx).document_floor(cx)
    }

    pub(crate) fn document_dock_px(&self, window: &Window, cx: &App) -> Option<f32> {
        self.editor.read(cx).document_dock_px(window, cx)
    }

    pub(crate) fn code_panel_visible(&self, cx: &App) -> bool {
        self.editor.read(cx).code_panel_visible()
    }

    pub(crate) fn editor_unsaved(&self, cx: &App) -> Vec<crate::ui::editor::code_editor::BufferId> {
        self.editor.read(cx).editor_unsaved()
    }

    pub(crate) fn editor_unsaved_in_tab(
        &self,
        tab_ix: usize,
        cx: &App,
    ) -> Vec<crate::ui::editor::code_editor::BufferId> {
        let Some(tab) = self.tabs.get(tab_ix).map(|t| t.tree_id.get()) else {
            return Vec::new();
        };
        self.editor.read(cx).editor_unsaved_in_tab(tab)
    }

    pub(crate) fn editor_guard_unsaved(
        &mut self,
        ids: Vec<crate::ui::editor::code_editor::BufferId>,
        then: crate::ui::editor::code_editor::AfterUnsaved,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.sync_editor_context(Some(window), cx);
        self.editor
            .update(cx, |ed, cx| ed.editor_guard_unsaved(ids, then, window, cx))
    }

    pub(crate) fn lsp_window_closed(&self, cx: &mut App) {
        self.editor.update(cx, |ed, cx| ed.lsp_window_closed(cx));
    }

    pub(crate) fn maybe_refresh_diff_overlay(&mut self, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |ed, cx| ed.maybe_refresh_diff_overlay(cx));
    }

    pub(crate) fn toggle_code_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_editor_context(Some(window), cx);
        self.editor
            .update(cx, |ed, cx| ed.toggle_code_panel(window, cx));
    }

    pub(crate) fn open_file_in_editor(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editor_context(Some(window), cx);
        self.editor
            .update(cx, |ed, cx| ed.open_file_in_editor(path, window, cx));
    }

    pub(crate) fn open_file_in_editor_at(
        &mut self,
        path: &std::path::Path,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editor_context(Some(window), cx);
        self.editor.update(cx, |ed, cx| {
            ed.open_file_in_editor_at(path, line, column, window, cx)
        });
    }

    pub(crate) fn open_linked_file(
        &mut self,
        path: &std::path::Path,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editor_context(Some(window), cx);
        self.editor.update(cx, |ed, cx| {
            ed.open_linked_file(path, line, column, is_dir, window, cx)
        });
    }

    pub(crate) fn toggle_diff_overlay(
        &mut self,
        host: crate::ui::remote::host_ops::HostId,
        cwd: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editor_context(Some(window), cx);
        self.editor
            .update(cx, |ed, cx| ed.toggle_diff_overlay(host, cwd, window, cx));
    }

    pub(crate) fn open_diff_overlay(
        &mut self,
        host: crate::ui::remote::host_ops::HostId,
        cwd: std::path::PathBuf,
        source: crate::git::diff::DiffSource,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editor_context(Some(window), cx);
        self.editor.update(cx, |ed, cx| {
            ed.open_diff_overlay(host, cwd, source, focus, window, cx)
        });
    }

    /// Reconcile deferred tab/group/watcher/drag state at the start of a frame
    /// before constructing the window element tree.
    fn reconcile_before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.claim_pending_tab(window, cx);
        self.touch_active_tab();
        self.sync_editor_context(Some(window), cx);
        self.declare_displayed_panes(cx);
        self.sync_file_tree_context(window, cx);
        self.sync_scm_watchers(window, cx);
        self.editor.update(cx, |ed, cx| ed.editor_sync(window, cx));
        // Keeps looking for new listening ports on the pane in front, panel
        // open or not — a port that appears while the panel is shut is exactly
        // the one worth forwarding unasked.
        self.sync_port_watch(window, cx);
        if cx.has_active_drag() {
            crate::ui::workspace::reorder::clear_pending(&self.reorder);
            crate::ui::workspace::pane_drag::clear_landing(&self.pane_drag);
        } else {
            // Taken first either way: this is what ends the drag, and the merge
            // below must not find the tab it just moved still in the air.
            let landed = crate::ui::workspace::reorder::take_landed(&self.reorder);
            if let Some((tab, zone)) = self.tab_merge.take() {
                self.merge_tab(tab, zone, window, cx);
            } else if let Some(order) = landed.order {
                self.apply_tab_order(&order, cx);
            }
            // Also what ends the pane drag, so it is taken whichever of the two
            // readings the last frame left behind.
            let landing = crate::ui::workspace::pane_drag::take_landing(&self.pane_drag);
            if let Some((pane, at)) = self.pane_detach.take() {
                self.detach_pane(pane, at, window, cx);
            } else if let Some((from, zone)) = landing {
                self.drop_pane(from, zone, window, cx);
            }
        }
        // Windows has no closed-hand cursor and gpui answers `ClosedHand` with
        // the plain arrow there, which would drop the grip's pointing hand the
        // instant the drag it advertised began.
        let held = if cfg!(target_os = "windows") {
            gpui::CursorStyle::PointingHand
        } else {
            gpui::CursorStyle::ClosedHand
        };
        if (self.reorder.borrow().is_some() || self.pane_drag.borrow().is_some())
            && cx.active_drag_cursor_style() != Some(held)
        {
            cx.set_active_drag_cursor_style(held, window);
        }
    }

    fn sync_scm_watchers(&mut self, window: &Window, cx: &mut Context<Self>) {
        use crate::git::data::{ScmWatcher, scm_sync_watchers};
        let wanted = [
            (ScmWatcher::Panel, self.scm_panel_target(window, cx)),
            (ScmWatcher::FileTree, self.scm_tree_target(window, cx)),
            (ScmWatcher::Editor, self.scm_editor_target(cx)),
        ];
        let lost = scm_sync_watchers(cx.entity_id().as_u64(), wanted, cx);
        let scm = self.right_panel.read(cx).scm.clone();
        for host in lost {
            scm.update(cx, |scm, _| scm.forget_host(host));
        }
    }

    fn scm_panel_target(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(crate::ui::remote::host_ops::HostId, std::path::PathBuf)> {
        let rp = self.right_panel.read(cx);
        if !rp.visible || rp.tab != RightPanelTab::Scm {
            return None;
        }
        if let Some(repo) = rp.scm.read(cx).active_repo() {
            return Some((repo.host, repo.root.clone()));
        }
        let leaf = self.tabs.get(self.active)?.detail_pane(window, cx)?;
        let view = leaf.read(cx);
        let host = view.host_id();
        let root = cx
            .try_global::<crate::git::status::GitStatusCache>()?
            .repo_root_for(host, view.git_status_cwd()?)?;
        Some((host, root.to_path_buf()))
    }

    fn scm_tree_target(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(crate::ui::remote::host_ops::HostId, std::path::PathBuf)> {
        if !self.file_tree_on_screen(cx) {
            return None;
        }
        let leaf = self.tabs.get(self.active)?.detail_pane(window, cx)?;
        let view = leaf.read(cx);
        let host = view.host_id();
        let root = cx
            .try_global::<crate::git::status::GitStatusCache>()?
            .repo_root_for(host, view.git_status_cwd()?)?;
        Some((host, root.to_path_buf()))
    }

    fn scm_editor_target(
        &self,
        cx: &App,
    ) -> Option<(crate::ui::remote::host_ops::HostId, std::path::PathBuf)> {
        let ed = self.editor.read(cx);
        if !ed.code_panel_visible() {
            return None;
        }
        let (host, path) = ed.editor_active_location()?;
        let root = cx
            .try_global::<crate::git::status::GitStatusCache>()
            .and_then(|cache| cache.repo_root_for(host, path.parent()?))
            .or_else(|| ed.editor_gutter_root())?;
        Some((host, root.to_path_buf()))
    }

    pub(crate) fn active_host(
        &self,
        cx: &App,
    ) -> Option<crate::ui::remote::host_ops::SharedHost> {
        crate::ui::remote::host_registry::HostRegistry::lookup(cx, self.spawn_host(cx))
    }

    pub(crate) fn file_tree_on_screen(&self, cx: &App) -> bool {
        let rp = self.right_panel.read(cx);
        self.right_panel_open(cx)
            && rp.tab == RightPanelTab::Files
            && !rp.sftp_panel.read(cx).is_open()
    }

    pub(crate) fn active_tree_roots(&self, cx: &App) -> Vec<std::path::PathBuf> {
        self.right_panel
            .read(cx)
            .file_tree
            .read(cx)
            .active_roots()
            .to_vec()
    }

    pub(crate) fn sync_file_tree_context(&self, window: &Window, cx: &mut Context<Self>) {
        use crate::daemon::protocol::RemoteKind;
        use crate::ui::files::file_tree::FileTreeContext;

        let tab = self.tabs.get(self.active);
        let tab_id = tab.map(|t| t.tree_id.get());
        let host_id = self.spawn_host(cx);
        let host = self.active_host(cx);
        let leaves = tab.map(|t| t.pane.terminals()).unwrap_or_default();
        let pane_cwds = leaves
            .iter()
            .filter(|leaf| leaf.read(cx).host_id() == host_id)
            .filter_map(|leaf| leaf.read(cx).files_cwd())
            .collect();
        let active_leaf = tab.and_then(|t| t.detail_pane(window, cx));
        let active_is_ssh = active_leaf.as_ref().is_some_and(|leaf| {
            leaf.read(cx)
                .remote_context()
                .is_some_and(|remote| remote.kind == RemoteKind::NativeSsh)
        });
        let display_home = active_leaf.and_then(|leaf| leaf.read(cx).display_home(cx));
        let on_screen = self.file_tree_on_screen(cx);
        let dirty_paths = self.editor.read(cx).editor_dirty_paths(host_id, cx);
        let ctx = FileTreeContext {
            tab_id,
            host_id,
            host,
            pane_cwds,
            active_is_ssh,
            display_home,
            on_screen,
            dirty_paths,
        };
        let file_tree = self.right_panel.read(cx).file_tree.clone();
        file_tree.update(cx, |ft, cx| ft.sync_context(ctx, cx));
    }

    pub(crate) fn file_tree_refresh_roots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_file_tree_context(window, cx);
        let file_tree = self.right_panel.read(cx).file_tree.clone();
        file_tree.update(cx, |ft, cx| ft.refresh_roots(cx));
    }

    pub(crate) fn path_under_tree_root(
        &self,
        path: &std::path::Path,
        cx: &App,
    ) -> Option<std::path::PathBuf> {
        self.right_panel
            .read(cx)
            .file_tree
            .read(cx)
            .path_under_tree_root(path)
    }

    pub(crate) fn file_tree_reveal_path(
        &mut self,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> bool {
        let file_tree = self.right_panel.read(cx).file_tree.clone();
        file_tree.update(cx, |ft, cx| ft.reveal_path(path, cx))
    }

    pub(crate) fn file_tree_show(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_file_tree_context(window, cx);
        let file_tree = self.right_panel.read(cx).file_tree.clone();
        file_tree.update(cx, |ft, cx| ft.show_path(path, window, cx));
    }

    /// Types `@path` into the running agent's prompt, relative to the tree's
    /// root when the file is under one. `suffix` goes straight after the
    /// path — the editor adds the selected lines as `#L3-9`.
    pub(crate) fn attach_path_to_agent(
        &mut self,
        path: &std::path::Path,
        suffix: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.agent_target_leaf(cx) else {
            crate::platform::notify::notify_desktop(
                Some("anterm"),
                t(L10nKey::AppNoRunningCodingAgent),
            );
            return;
        };
        let rel = self
            .path_under_tree_root(path, cx)
            .unwrap_or_else(|| path.to_path_buf());
        target.update(cx, |view, cx| {
            view.paste(format!("@{}{suffix} ", rel.display()), cx);
        });
    }

    fn handle_file_tree_event(
        &mut self,
        ev: &crate::ui::files::file_tree::FileTreeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::files::file_tree::{FileTreeEvent, shell_quote_for};
        match ev {
            FileTreeEvent::OpenFile(path) => self.open_file_in_editor(path, window, cx),
            FileTreeEvent::CdToDir(dir) => {
                let Some(leaf) = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.pane.focused_or_first(window, cx))
                else {
                    return;
                };
                let program = leaf.read(cx).shell_spec().map(|spec| spec.program);
                leaf.read(cx)
                    .run_command_line(&format!("cd {}", shell_quote_for(dir, program.as_deref())));
                self.focus_active(window, cx);
            }
            FileTreeEvent::InsertPath(path) => {
                if let Some(leaf) = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.pane.focused_or_first(window, cx))
                {
                    leaf.update(cx, |view, cx| {
                        let program = view.shell_spec().map(|spec| spec.program);
                        view.paste(shell_quote_for(path, program.as_deref()), cx);
                    });
                }
            }
            FileTreeEvent::AttachToAgent(path) => self.attach_path_to_agent(path, "", cx),
            FileTreeEvent::EditorPathMoved { host, from, to } => {
                self.editor
                    .update(cx, |ed, cx| ed.editor_path_moved(*host, from, to, cx));
            }
            FileTreeEvent::EditorPathRemoved { host, path } => {
                self.editor
                    .update(cx, |ed, cx| ed.editor_path_removed(*host, path, cx));
            }
            FileTreeEvent::ShowPanel => {
                if !self.right_panel_open(cx) {
                    self.toggle_right_panel(cx);
                }
                self.set_right_panel_tab(RightPanelTab::Files, cx);
            }
        }
    }

    pub(crate) fn sync_scm_context(&self, width: f32, window: &Window, cx: &mut Context<Self>) {
        let pane_target = self
            .tabs
            .get(self.active)
            .and_then(|t| t.detail_pane(window, cx))
            .and_then(|leaf| {
                let view = leaf.read(cx);
                let cwd = view.effective_host_cwd()?;
                Some((view.host(cx)?, cwd))
            });
        let diff_focus = self
            .editor
            .read(cx)
            .active_diff_overlay()
            .and_then(|o| o.read(cx).focus_target());
        let ctx = crate::ui::files::scm::ScmContext {
            active_tab: self.active,
            pane_target,
            diff_focus,
            panel_width: width,
        };
        let scm = self.right_panel.read(cx).scm.clone();
        scm.update(cx, |scm, _| scm.sync_context(ctx));
    }

    pub(crate) fn run_scm_action(
        &mut self,
        intent: crate::ui::files::scm::ScmIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let width = self.right_panel_px(window, cx);
        self.sync_scm_context(width, window, cx);
        let scm = self.right_panel.read(cx).scm.clone();
        scm.update(cx, |scm, cx| scm.run_action(intent, window, cx));
    }

    pub(crate) fn scm_toggle_graph(&mut self, cx: &mut Context<Self>) {
        let scm = self.right_panel.read(cx).scm.clone();
        scm.update(cx, |scm, cx| scm.toggle_graph(cx));
    }

    pub(crate) fn toggle_diff_view_mode(&mut self, cx: &mut Context<Self>) {
        let next = match cx.global::<crate::core::config::Config>().diff_view {
            crate::core::config::DiffViewMode::Split => crate::core::config::DiffViewMode::Unified,
            crate::core::config::DiffViewMode::Unified => crate::core::config::DiffViewMode::Split,
        };
        self.update_config(cx, |cfg| cfg.diff_view = next);
    }

    pub(crate) fn right_panel_open(&self, cx: &App) -> bool {
        self.right_panel.read(cx).is_open(!self.tabs.is_empty())
    }

    pub(crate) fn sidebar_floor(&self, cx: &App) -> f32 {
        if self.sidebar_open(cx) {
            crate::ui::workspace::tab_sidebar::MIN_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    pub(crate) fn right_panel_min_px(&self, window: &Window, cx: &App) -> f32 {
        self.right_panel.read(cx).min_px(window, cx)
    }

    pub(crate) fn right_panel_px(&self, window: &Window, cx: &App) -> f32 {
        let others_floor = self.sidebar_floor(cx) + self.document_floor(cx);
        self.right_panel
            .read(cx)
            .width_px(others_floor, window, cx)
    }

    pub(crate) fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        self.right_panel
            .update(cx, |rp, cx| rp.toggle_visible(cx));
        cx.notify();
    }

    pub(crate) fn set_right_panel_tab(&mut self, tab: RightPanelTab, cx: &mut Context<Self>) {
        self.right_panel.update(cx, |rp, cx| rp.set_tab(tab, cx));
        cx.notify();
    }

    pub(crate) fn toggle_sftp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::ui::workspace::intent::WorkspaceIntent;
        let intent = {
            let rp = self.right_panel.read(cx);
            if rp.visible && rp.tab == RightPanelTab::Files {
                WorkspaceIntent::HideRightPanel
            } else {
                WorkspaceIntent::ShowRightPanel(RightPanelTab::Files)
            }
        };
        self.handle_intent(intent, window, cx);
    }

    pub(crate) fn pane_shell_cwd(&self, pane_id: u64, window: &Window, cx: &App) -> Option<String> {
        let leaf = self
            .tabs
            .get(self.active)?
            .pane
            .focused_or_first(window, cx)?;
        let leaf = leaf.read(cx);
        if leaf.pane_id != pane_id {
            return None;
        }
        let path = leaf.cwd()?;
        let s = path.to_string_lossy().to_string();
        s.starts_with('/').then_some(s)
    }

    fn remote_files_pane(&self, window: &Window, cx: &App) -> Option<(u64, String)> {
        use crate::daemon::protocol::{RemoteKind, SshPhase};
        let leaf = self.tabs.get(self.active)?.detail_pane(window, cx)?;
        let view = leaf.read(cx);
        let remote = view.remote_context()?;
        if remote.kind != RemoteKind::NativeSsh
            || !matches!(view.ssh_phase(), Some(SshPhase::Connected))
        {
            return None;
        }
        Some((view.pane_id, remote.target))
    }

    pub(crate) fn pane_forward_ctx(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<crate::ui::files::right_panel::PaneForwardCtx> {
        let leaf = self.tabs.get(self.active)?.detail_pane(window, cx)?;
        let view = leaf.read(cx);
        let connected_ssh = view
            .remote_context()
            .is_some_and(|c| c.kind == crate::daemon::protocol::RemoteKind::NativeSsh)
            && matches!(
                view.ssh_phase(),
                Some(crate::daemon::protocol::SshPhase::Connected)
            );
        Some(crate::ui::files::right_panel::PaneForwardCtx {
            pane_id: view.pane_id,
            route: connected_ssh.then(|| view.forward_route()),
            port_route: view.port_route(cx),
            host: (!view.host_id().is_local())
                .then(|| view.host(cx))
                .flatten(),
        })
    }

    pub(crate) fn sync_port_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ctx = self.pane_forward_ctx(window, cx);
        self.right_panel
            .update(cx, |rp, cx| rp.sync_port_watch(ctx, cx));
    }

    pub(crate) fn show_ssh_forwards(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ctx = self.pane_forward_ctx(window, cx);
        self.right_panel
            .update(cx, |rp, cx| rp.show_ssh_forwards(ctx.as_ref(), window, cx));
    }

    pub(crate) fn remember_forward_enabled(
        &mut self,
        pane_id: u64,
        forward: &crate::daemon::protocol::ManagedForward,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let profile_id = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane.terminals())
            .find_map(|leaf| {
                let view = leaf.read(cx);
                (view.pane_id == pane_id).then(|| view.ssh_spec()?.profile_id.clone())?
            })
            .and_then(|id| uuid::Uuid::parse_str(&id).ok());
        let Some(profile_id) = profile_id else {
            return;
        };
        let saved = cx
            .global::<Config>()
            .ssh_profiles
            .iter()
            .find(|p| p.id == profile_id)
            .and_then(|p| {
                crate::ui::remote::forwards::saved_rule_index(&p.forwards, forward, enabled)
            });
        let Some(idx) = saved else {
            return;
        };
        self.update_config(cx, |cfg| {
            if let Some(rule) = cfg
                .ssh_profiles
                .iter_mut()
                .find(|p| p.id == profile_id)
                .and_then(|p| p.forwards.get_mut(idx))
            {
                rule.enabled = enabled;
            }
        });
    }

    fn build_info_snapshot(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<crate::ui::files::right_panel::InfoPaneSnapshot> {
        let forward_ctx = self.pane_forward_ctx(window, cx)?;
        let leaf = self.tabs.get(self.active)?.detail_pane(window, cx)?;
        let view = leaf.read(cx);
        Some(crate::ui::files::right_panel::InfoPaneSnapshot {
            pane_id: view.pane_id,
            forward_ctx,
            cwd: view.effective_cwd(),
            display_home: view.display_home(cx),
            is_local: view.local_cwd().is_some(),
            shell_program: view.shell_spec().map(|s| s.program.clone()),
            default_shell_label: self.default_shell_label(cx),
            ssh_host: view.ssh_spec().map(|s| s.host.clone()),
            git_status: view.git_status(cx),
            diff_target: view
                .git_status_cwd()
                .map(|cwd| (view.host_id(), cwd.to_path_buf())),
        })
    }

    fn render_right_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let others_floor = self.sidebar_floor(cx) + self.document_floor(cx);
        let remote_pane = self.remote_files_pane(window, cx);
        let remote_shell_cwd = remote_pane
            .as_ref()
            .and_then(|(id, _)| self.pane_shell_cwd(*id, window, cx));
        let info_snapshot = self.build_info_snapshot(window, cx);
        let ctx = crate::ui::files::right_panel::RightPanelContext {
            has_tabs: !self.tabs.is_empty(),
            others_floor,
            remote_pane,
            remote_shell_cwd,
            info_snapshot,
        };
        let open = self
            .right_panel
            .update(cx, |rp, cx| rp.sync_context(ctx, cx));
        if !open {
            return None;
        }
        let tab = self.right_panel.read(cx).tab;
        match tab {
            RightPanelTab::Scm => {
                let width = self.right_panel_px(window, cx);
                self.sync_scm_context(width, window, cx);
            }
            RightPanelTab::Files | RightPanelTab::Search => {
                self.sync_file_tree_context(window, cx);
            }
            RightPanelTab::Info => {}
        }
        Some(self.right_panel.clone().into_any_element())
    }

    fn handle_right_panel_event(
        &mut self,
        ev: &crate::ui::files::right_panel::RightPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::ui::files::right_panel::RightPanelEvent;
        match ev {
            RightPanelEvent::CloseAndFocusTerminal => self.focus_active(window, cx),
            RightPanelEvent::ToggleDiffOverlay { host, cwd } => {
                self.toggle_diff_overlay(*host, cwd.clone(), window, cx);
            }
            RightPanelEvent::OpenDiff {
                host,
                root,
                source,
                focus,
            } => {
                self.open_diff_overlay(
                    *host,
                    root.clone(),
                    source.clone(),
                    focus.clone(),
                    window,
                    cx,
                );
            }
            RightPanelEvent::OpenFile(path) => self.open_file_in_editor(path, window, cx),
            RightPanelEvent::OpenRemoteFile { host, path } => {
                self.sync_editor_context(Some(window), cx);
                self.editor
                    .update(cx, |ed, cx| ed.editor_open_on_host(host.clone(), path, window, cx));
            }
            RightPanelEvent::GotoRemoteShellCwd { pane_id } => {
                if let Some(cwd) = self.pane_shell_cwd(*pane_id, window, cx) {
                    let sftp = self.right_panel.read(cx).sftp_panel.clone();
                    sftp.update(cx, |s, cx| s.navigate(cwd, cx));
                }
            }
            RightPanelEvent::FileTree(ft_ev) => self.handle_file_tree_event(ft_ev, window, cx),
            RightPanelEvent::RememberForwardEnabled {
                pane_id,
                forward,
                enabled,
            } => self.remember_forward_enabled(*pane_id, forward, *enabled, cx),
            RightPanelEvent::Intent(intent) => self.handle_intent(intent.clone(), window, cx),
        }
    }
}

impl Render for AntermApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prof = crate::ui::chrome::perf::enabled().then(std::time::Instant::now);
        self.reconcile_before_render(window, cx);
        let vertical = matches!(cx.global::<Config>().tab_bar_position, TabBarPosition::Left)
            && !self.tabs.is_empty();
        // Asked through the predicate the right panel sizes itself against —
        // two spellings of "is the rail up" is one more than the layout can
        // afford to have disagree.
        let rail = self.sidebar_open(cx);
        // A hover flag is only written by its sheet, and only on a change of
        // state it saw. A sheet that is not in the tree sees none: the rail
        // hidden by its own tile with the pointer on it, or the switch turned
        // off mid-hover, would leave its flag set, and the tiles would come
        // back painted, with nobody pointing at them, the next time the sheet
        // does. Clear the flag of every sheet not built this frame.
        let auto_hide_chrome = cx.global::<Config>().auto_hide_titlebar_buttons;
        if !auto_hide_chrome || !rail {
            self.sidebar_chrome_hover.set(false);
        }
        if !auto_hide_chrome {
            self.strip_chrome_hover.set(false);
        }
        // Both read before the strip and the sidebar are built: a tab held out
        // over the layout suspends the reorder, which is what those two ask
        // what to draw, and a pane held over *them* is measured against where
        // they put their tabs last frame — which is what they are about to
        // blank and write again.
        let tab_landing = self.tab_landing(window, cx);
        let detach_caret = self.detach_caret(window, cx);
        let chrome_dragging = cx.has_active_drag()
            || self.reorder.borrow().is_some()
            || self.pane_drag.borrow().is_some()
            || self.sidebar_dragging.get();
        let show_chips = !vertical;
        let show_chips_changed =
            self.strip_host.read(cx).show_chips.replace(show_chips) != show_chips;
        if show_chips_changed && !chrome_dragging {
            self.strip_host.update(cx, |_, cx| cx.notify());
        }
        let strip: gpui::AnyElement = if chrome_dragging {
            self.tab_strip(show_chips, window, cx).into_any_element()
        } else {
            let mut strip_style = gpui::StyleRefinement::default();
            strip_style.flex_grow = Some(1.);
            strip_style.size.width = Some(gpui::relative(1.).into());
            strip_style.size.height = Some(gpui::relative(1.).into());
            strip_style.min_size.width = Some(px(0.).into());
            self.strip_host
                .clone()
                .cached(strip_style)
                .into_any_element()
        };
        let sidebar: Option<gpui::AnyElement> = rail.then(|| {
            if chrome_dragging {
                self.tab_sidebar(window, cx).into_any_element()
            } else {
                let width = self.sidebar_px(window, cx);
                let mut sidebar_style = gpui::StyleRefinement::default();
                sidebar_style.flex_shrink = Some(0.);
                sidebar_style.size.width = Some(px(width).into());
                sidebar_style.size.height = Some(gpui::relative(1.).into());
                self.sidebar_host
                    .clone()
                    .cached(sidebar_style)
                    .into_any_element()
            }
        });
        let ssh_status = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))
            .and_then(|leaf| {
                let view = leaf.read(cx);
                view.ssh_phase()?;
                if !view.ssh_disconnected() {
                    return None;
                }
                let host = view
                    .terminal
                    .ssh_endpoint()
                    .map(|(h, _)| h)
                    .or_else(|| view.remote_context().map(|c| c.target))
                    .unwrap_or_default();
                let reason = match view.ssh_phase() {
                    Some(crate::daemon::protocol::SshPhase::Failed { reason })
                        if !reason.is_empty() =>
                    {
                        Some(reason)
                    }
                    _ => None,
                };
                let profile_id = view
                    .ssh_spec()
                    .and_then(|s| s.profile_id.clone())
                    .and_then(|id| uuid::Uuid::parse_str(&id).ok());
                let weak_app = cx.weak_entity();
                Some(crate::ui::remote::forwards::render_ssh_status_strip(
                    crate::ui::remote::forwards::SshStatusStripProps {
                        host,
                        reason,
                        profile_id,
                    },
                    move |intent, window, cx| {
                        let _ = weak_app.update(cx, |this, cx| {
                            this.handle_intent(intent, window, cx);
                        });
                    },
                    cx,
                ))
            });
        let body = match self.tabs.get(self.active) {
            None => self.render_home(cx).into_any_element(),
            Some(active_tab) => {
                let maximized = self.maximized.as_ref().filter(|leaf| {
                    active_tab
                        .pane
                        .leaves()
                        .iter()
                        .any(|l| l.entity_id() == leaf.entity_id())
                });
                match maximized {
                    Some(leaf) => div()
                        .size_full()
                        .overflow_hidden()
                        .child(leaf.clone())
                        .into_any_element(),
                    None => {
                        let several = active_tab.pane.leaves().len() > 1;
                        let chrome = crate::ui::workspace::pane::PaneChrome {
                            dim_inactive: several && cx.global::<Config>().dim_inactive_panes,
                            rearrangeable: several,
                            hovered: self.pane_hover.clone(),
                            lifted: crate::ui::workspace::pane_drag::lifted(&self.pane_drag),
                            drag: self.pane_drag.clone(),
                        };
                        active_tab.pane.render(&chrome, window, cx)
                    }
                }
            }
        };

        // No window buttons in fullscreen, where they cannot work: the window
        // has no caption for the platform to hit-test, so they would draw,
        // light up under the pointer and do nothing when clicked. `TitleBar`
        // always draws them, so the strip goes into a plain row of the same
        // geometry instead — the row itself stays, since it holds the tabs,
        // the chrome tiles and the docked document's header.
        let title_bar =
            if window_controls_drawn(window.is_fullscreen()) || cfg!(target_os = "macos") {
                // `TitleBar` pads its own fixed 80pt; the lead is ours, since
                // it follows where the traffic lights are put.
                TitleBar::new()
                    .h(px(TITLE_BAR_HEIGHT))
                    .pl(px(TITLE_BAR_LEAD))
                    .bg(cx.theme().transparent)
                    .border_color(cx.theme().transparent)
                    .child(strip)
                    .into_any_element()
            } else {
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .pl(px(TITLE_BAR_LEAD))
                    .title_strip_row(px(TITLE_BAR_BORDER), cx.theme().transparent)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h_full()
                            .flex_1()
                            .child(strip),
                    )
                    .into_any_element()
            };
        let body_area = div()
            .flex_1()
            .relative()
            .overflow_hidden()
            .child(
                gpui::canvas(
                    {
                        let area = self.pane_area.clone();
                        move |bounds, _window, _cx| area.set(Some(bounds))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(body)
            .when_some(self.pane_landing(window, cx), |this, el| this.child(el))
            .when_some(tab_landing, |this, el| this.child(el))
            .when_some(
                crate::ui::chrome::notice::anchor(
                    ssh_status
                        .into_iter()
                        .chain(self.render_lease_notices(cx))
                        .collect(),
                ),
                |this, el| this.child(el),
            );

        // One decision for the whole document surface. Docked, exactly one of
        // the two surfaces is drawn — a column has one child, and two `flex_1`
        // siblings would split it and fight — so `overlay_top` stops ordering a
        // pair and starts choosing between them. Filling, nothing changes: both
        // are rendered, ordered by `overlay_top`, and the front one wins on
        // paint order as it always has.
        let document_dock_px = self.document_dock_px(window, cx);
        let document_chrome = if cfg!(target_os = "macos") {
            crate::ui::editor::document_column::DocumentChrome::Dock
        } else {
            crate::ui::editor::document_column::DocumentChrome::DockHoisted
        };
        let document_header = document_dock_px
            .is_some()
            .then(|| {
                self.editor
                    .update(cx, |ed, cx| ed.render_document_header(document_chrome, window, cx))
            })
            .flatten();
        let (overlays, document_column) = match document_dock_px {
            Some(w) => {
                self.editor.update(cx, |ed, _| {
                    ed.set_render_mode(crate::ui::editor::DocumentRenderMode::Docked {
                        width: w,
                        chrome: document_chrome,
                    });
                });
                (
                    Vec::new(),
                    Some(
                        div()
                            .flex_none()
                            .w(px(w))
                            .h_full()
                            .child(self.editor.clone())
                            .into_any_element(),
                    ),
                )
            }
            None => {
                let has_fill = self.editor.read(cx).has_visible_document();
                self.editor.update(cx, |ed, _| {
                    ed.set_render_mode(if has_fill {
                        crate::ui::editor::DocumentRenderMode::Fill
                    } else {
                        crate::ui::editor::DocumentRenderMode::Hidden
                    });
                });
                let overlays = if has_fill {
                    vec![
                        div()
                            .absolute()
                            .inset_0()
                            .child(self.editor.clone())
                            .into_any_element(),
                    ]
                } else {
                    Vec::new()
                };
                (overlays, None)
            }
        };
        let document_px = document_column
            .as_ref()
            .map_or(0., |_| document_dock_px.unwrap_or_default());

        let right_panel = self.render_right_panel(window, cx);
        // A docked document takes the same fork the right panel does: on
        // Windows and Linux the window controls live at the right end of the
        // title bar, so the bar has to span the workspace rather than sit
        // inside the terminal column with a column drawn to the right of it.
        let panel_below_title_bar =
            (right_panel.is_some() || document_column.is_some()) && !cfg!(target_os = "macos");
        let (column_title_bar, spanning_title_bar) = if panel_below_title_bar {
            (None, Some(title_bar))
        } else {
            (Some(title_bar), None)
        };
        let (column_overlays, hoisted_overlays) = if panel_below_title_bar {
            (Vec::new(), overlays)
        } else {
            (overlays, Vec::new())
        };
        let panel_px = if right_panel.is_some() {
            self.right_panel_px(window, cx)
        } else {
            0.
        };
        let terminal_column = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .when_some(column_title_bar, |this, bar| this.child(bar))
            .child(body_area)
            .children(column_overlays);
        let panel_row = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_row()
            .child(terminal_column)
            .when_some(document_column, |this, column| this.child(column))
            .when_some(right_panel, |this, panel| this.child(panel));
        let main_layout = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_row()
            .when_some(sidebar, |this, sidebar| this.child(sidebar))
            .child(match spanning_title_bar {
                Some(bar) => div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .relative()
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            // One patch per column below, rather than one for
                            // both: each carries the left border its own column
                            // carries, so the rule between the document and the
                            // detail panel runs the full height of the window
                            // instead of stopping at the title bar.
                            .when(panel_px > 0., |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .right_0()
                                        .w(px(panel_px))
                                        .bg(crate::ui::chrome::theme::workspace_surface_color(cx))
                                        .border_l(crate::ui::chrome::theme::hairline(window))
                                        .border_color(cx.theme().sidebar_border),
                                )
                            })
                            .when(document_px > 0., |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .right(px(panel_px))
                                        .w(px(document_px))
                                        .bg(crate::ui::chrome::theme::workspace_surface_color(cx))
                                        .border_l(crate::ui::chrome::theme::hairline(window))
                                        .border_color(cx.theme().sidebar_border),
                                )
                            })
                            .child(bar)
                            // The document's header, in the strip the spanning
                            // title bar leaves empty above its column. Drawn
                            // after the bar so it sits over the tab strip's
                            // slack — and stopping short of the trailing
                            // chrome, which is only in the way when the detail
                            // panel is closed and this column is the one at the
                            // window's right edge.
                            .when_some(document_header, |this, header| {
                                this.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .h(px(TITLE_BAR_HEIGHT))
                                        .right(px(panel_px))
                                        .w(px(document_px))
                                        .when(panel_px <= 0., |d| {
                                            d.pr(px(crate::ui::workspace::tab_strip::trailing_chrome_w(
                                                window.is_fullscreen(),
                                            )))
                                        })
                                        .child(header),
                                )
                            }),
                    )
                    .child(panel_row)
                    .children(hoisted_overlays.into_iter().map(|overlay| {
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .bottom_0()
                            .right(px(panel_px))
                            .child(overlay)
                    }))
                    .into_any_element(),
                None => panel_row.into_any_element(),
            })
            .into_any_element();

        let window_bg = crate::ui::chrome::theme::workspace_background(cx);
        let bg_image = window_background_image_layer(cx);

        let root =
            div()
                .id("anterm-root")
                .size_full()
                .flex()
                .flex_col()
                .bg(window_bg)
                .text_color(cx.theme().foreground)
                .on_modifiers_changed(cx.listener(Self::on_modifiers_changed));
        let focused_pane_id = self
            .tabs
            .get(self.active)
            .and_then(|t| t.pane.focused_or_first(window, cx))
            .map(|p| p.read(cx).pane_id);
        let ssh_prompt_overlay = self
            .ssh_prompt
            .read(cx)
            .is_visible_for(focused_pane_id)
            .then(|| self.ssh_prompt.clone());
        let root = Self::with_app_actions(root, cx)
                .children(bg_image)
                .child(main_layout)
                // Window-level because the strip lives in the title bar and the
                // sidebar down the side: the caret between two tabs is in
                // neither of the boxes the rest of the drag feedback is drawn
                // in.
                .when_some(detach_caret, |this, caret| this.child(caret))
                // Same reason, and the ssh prompt has more claim to it than any
                // of them: nothing in the window can proceed until the password
                // is answered, so the scrim has to cover the whole window and
                // not stop at the terminal area.
                .when_some(ssh_prompt_overlay, |this, modal| this.child(modal))
                .when_some(self.search.clone(), |this, search| this.child(search));

        if let Some(start) = prof {
            crate::ui::chrome::perf::record("window", start.elapsed());
        }
        root
    }
}
