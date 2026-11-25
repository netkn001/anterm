use super::*;

use alacritty_terminal::index::{Column, Line, Point};
use gpui_kit::component::WindowExt as _;

use anterm_ui_kit::config::LinkFileOpen;
use anterm_ui_kit::platform::open::{open_file_path, reveal_file_path};
use crate::search::LinkTarget;

/// Which path dialect a pane's output is written in.
///
/// A pane running on this machine spells paths the way this OS does, and that
/// is the end of it: `/etc` printed by a `cmd.exe` pane sitting on `C:` means
/// `C:\etc`, exactly as `cd /etc` would there. Reading it as a rooted POSIX
/// path would underline a file this machine has not got, and a link that
/// cannot be opened is worse than no link.
///
/// A pane whose paths live somewhere else is asked instead — by the only thing
/// that host ever says about its own spelling, the directory it reports. A
/// `/`-rooted cwd is a POSIX host's. A pane that has not said where it is
/// falls to POSIX: there is no local drive to measure it from either way, and
/// every host anterm installs a server on over SSH or WSL spells paths that way.
pub(super) fn link_path_style(
    paths_are_local: bool,
    host_cwd: Option<&std::path::Path>,
) -> crate::search::PathStyle {
    use crate::search::PathStyle;
    match paths_are_local {
        true => PathStyle::NATIVE,
        false => host_cwd.map_or(PathStyle::Posix, PathStyle::of_dir),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HoveredLink {
    /// The cells to underline, one run per row. A link in a table cell
    /// shares its rows with other cells, so its extent is not always one
    /// stretch of the grid from a start to an end.
    pub runs: Vec<(Point, Point)>,
    /// Whether the modifier that would open this link is down.
    ///
    /// A link underlines as soon as the pointer reaches it, so the user can
    /// see there is something there without holding anything. Only once the
    /// modifier is down does it look and behave like something clickable:
    /// promising a hand cursor over a link a plain click will not follow is
    /// the kind of small lie that teaches people to stop trusting the
    /// underline.
    pub armed: bool,
}

enum LoopbackOpen {
    Forwarded(String),
    ForwardFailed(String),
    NotLoopback,
}

/// What sits under a cell, once the grid has been read.
enum GridLink {
    /// An OSC 8 hyperlink the emitter declared, with the extent it declared,
    /// one run per row.
    Hyperlink(String, Vec<(Point, Point)>),
    /// The logical line the cell belongs to, the grid point of every character
    /// in it, and which of those the cell is.
    Text(String, Vec<Point>, usize),
}

/// The outcome of asking what a cell links to.
pub(super) enum LinkAt {
    /// A link, and the cells it covers as one run per row.
    Found(LinkTarget, Vec<(Point, Point)>),
    /// A token that parses as a path, that nothing has answered for.
    /// `pending` separates the two reasons: the path is not there, or the host
    /// that would know has not replied yet.
    Unresolved {
        candidate: crate::search::FileCandidate,
        pending: bool,
    },
    None,
}

pub use anterm_ui_kit::daemon_rpc::PortRoute;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LoopbackPlan {
    Direct,
    ForwardOnPane(u64),
}

pub(super) fn port_route_of(plan: &LoopbackPlan, local: bool) -> PortRoute {
    match plan {
        LoopbackPlan::ForwardOnPane(_) => PortRoute::Forward,
        LoopbackPlan::Direct if local => PortRoute::Direct,
        LoopbackPlan::Direct => PortRoute::Blocked,
    }
}

pub(crate) fn loopback_plan(
    enabled: bool,
    remote_kind: Option<anterm_core::daemon::protocol::RemoteKind>,
    pane_id: u64,
) -> LoopbackPlan {
    if !enabled {
        return LoopbackPlan::Direct;
    }
    match remote_kind {
        Some(anterm_core::daemon::protocol::RemoteKind::NativeSsh) => {
            LoopbackPlan::ForwardOnPane(pane_id)
        }
        _ => LoopbackPlan::Direct,
    }
}

/// Whether a file link's menu offers "Open with Default App" beside "Open".
///
/// Only for a file on this machine: the OS opener is handed a local path, and
/// a remote pane's path names nothing here — or worse, this machine's copy.
/// And not when "Open" already goes to the OS association, where the two rows
/// would do the same thing.
pub(super) fn offers_default_app_open(host_is_local: bool, open_mode: LinkFileOpen) -> bool {
    host_is_local && open_mode != LinkFileOpen::System
}

pub(super) fn run_file_command(
    template: &str,
    path: &std::path::Path,
    line: Option<u32>,
    column: Option<u32>,
) -> std::io::Result<()> {
    let argv = expand_file_command_template(template, path, line, column);
    let Some((program, args)) = argv.split_first() else {
        // Sanitize maps a blank template to None, so the only way here is a
        // template whose tokens all expand to nothing (a lone `{line}` on a
        // link with no line number). Still a config error worth reporting.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "link_file_command expanded to nothing",
        ));
    };
    std::process::Command::new(program).args(args).spawn()?;
    Ok(())
}

pub(super) fn expand_file_command_template(
    template: &str,
    path: &std::path::Path,
    line: Option<u32>,
    column: Option<u32>,
) -> Vec<String> {
    let path = path.to_string_lossy();
    template
        .split_whitespace()
        .filter_map(|token| expand_file_command_token(token, &path, line, column))
        .collect()
}

fn expand_file_command_token(
    token: &str,
    path: &str,
    line: Option<u32>,
    column: Option<u32>,
) -> Option<String> {
    let mut out = String::with_capacity(token.len());
    let mut rest = token;
    while let Some(open) = rest.find('{') {
        let Some(close_rel) = rest[open..].find('}') else {
            break;
        };
        let close = open + close_rel;
        out.push_str(&rest[..open]);
        let value = match &rest[open + 1..close] {
            "path" => Some(path.to_string()),
            "line" => line.map(|l| l.to_string()),
            "column" => column.map(|c| c.to_string()),
            other => Some(format!("{{{other}}}")),
        };
        out.push_str(&value?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

impl TerminalView {
    /// Same shape as `warn_paste_upload_failed`: one toast per failed open, so
    /// a broken `link_file_command` surfaces as a config problem instead of a
    /// "dead link" (#542). Spawn is all that is reported — a spawned opener
    /// that exits non-zero is nobody's to see.
    fn warn_file_open_failed(
        &self,
        path: &std::path::Path,
        reason: &std::io::Error,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        log::warn!("failed to open file link {}: {reason}", path.display());
        let path = path.display().to_string();
        let reason = reason.to_string();
        window.push_notification(
            anterm_ui_kit::host_ops::failure(
                anterm_i18n::t_fmt(
                    anterm_i18n::L10nKey::LinkFileOpenFailed,
                    &[("path", path.as_str()), ("error", reason.as_str())],
                ),
                &reason,
            ),
            cx,
        );
    }

    fn grid_line(
        term: &alacritty_terminal::Term<crate::remote::EventProxy>,
        row: usize,
    ) -> Option<Line> {
        let line = Line(row as i32 - term.grid().display_offset() as i32);
        (line >= term.topmost_line() && line <= term.bottommost_line()).then_some(line)
    }

    pub fn open_link_at(
        &mut self,
        col: usize,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !cx.global::<Config>().link_url {
            return false;
        }
        let include_loopback = self.can_forward_loopback(cx);
        match self.resolve_link_at(col, row, true, include_loopback, cx) {
            LinkAt::Found(LinkTarget::Url(url), ..) => self.open_url(&url, window, cx),
            LinkAt::Found(
                LinkTarget::File {
                    path,
                    line,
                    column,
                    is_dir,
                },
                ..,
            ) => self.open_file_link(path, line, column, is_dir, window, cx),
            LinkAt::Unresolved { candidate, pending } => {
                return self.report_unresolved_link(&candidate, pending, window, cx);
            }
            LinkAt::None => return false,
        }
        true
    }

    /// Latches the file link under a right mouse-down for the context menu.
    ///
    /// Resolving here rather than in the menu builder is what lets the menu
    /// name a real file: the builder runs a turn later, with no event and no
    /// pointer, and asking the grid then would be asking about wherever the
    /// mouse has since gone.
    pub fn record_menu_link(&mut self, col: usize, row: usize, cx: &mut Context<Self>) {
        // The same switch that decides whether a path underlines and whether a
        // click follows one. Without this the menu would go on offering to
        // open files in a pane where link detection is turned off.
        if !cx.global::<Config>().link_url {
            self.links.menu_link = None;
            return;
        }
        let include_loopback = self.can_forward_loopback(cx);
        self.links.menu_link = match self.resolve_link_at(col, row, true, include_loopback, cx) {
            LinkAt::Found(target @ LinkTarget::File { .. }, ..) => Some(target),
            _ => None,
        };
    }

    /// Drops a latched link, for a right click the application is taking.
    pub fn forget_menu_link(&mut self) {
        self.links.menu_link = None;
    }

    /// The path the context menu is about, if it is about one.
    pub(super) fn menu_link_path(&self) -> Option<&std::path::Path> {
        match self.links.menu_link.as_ref()? {
            LinkTarget::File { path, .. } => Some(path),
            LinkTarget::Url(_) => None,
        }
    }

    pub(super) fn open_menu_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(LinkTarget::File {
            path,
            line,
            column,
            is_dir,
        }) = self.links.menu_link.clone()
        else {
            return;
        };
        self.open_file_link(path, line, column, is_dir, window, cx);
    }

    /// "Open with Default App": the file link under the menu, handed to the
    /// OS association whatever `link_file_open` says "Open" does.
    pub(super) fn open_menu_link_with_default_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The menu only offers this for a local pane; a key binding could
        // still fire it on a remote one, where the path means nothing here.
        if !self.session.host_id.is_local() {
            return;
        }
        let Some(path) = self.menu_link_path().map(std::path::Path::to_path_buf) else {
            return;
        };
        if let Err(e) = open_file_path(&path) {
            self.warn_file_open_failed(&path, &e, window, cx);
        }
    }

    pub(super) fn reveal_menu_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.menu_link_path().map(std::path::Path::to_path_buf) else {
            return;
        };
        if let Err(e) = reveal_file_path(&path) {
            self.warn_file_open_failed(&path, &e, window, cx);
        }
    }

    pub(super) fn copy_menu_link_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.menu_link_path() else {
            return;
        };
        let text = path.to_string_lossy().into_owned();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Hands a resolved file link to whatever the user wants opening files.
    ///
    /// The built-in editor is the default because it is the one place that can
    /// honour `line` and `column`, and the only one that can open a file that
    /// lives on another machine at all — an external command would be handed a
    /// path that means nothing here.
    fn open_file_link(
        &mut self,
        path: std::path::PathBuf,
        line: Option<u32>,
        column: Option<u32>,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cfg = cx.global::<Config>();
        let mode = cfg.file_open_mode();
        let command = cfg.link_file_command.clone();
        // A path that was just resolved on another machine means nothing to a
        // local opener: `open` and a user's `code --goto` would both be handed
        // a path this filesystem never had, throwing away the one thing that
        // made the link right and silently showing this machine's copy when
        // one happens to exist. The editor is the only one that can reach the
        // file, so it takes the click whatever the setting says.
        let mode = match self.session.host_id.is_local() {
            true => mode,
            false => LinkFileOpen::Internal,
        };
        // Both external arms have to answer for a failed spawn (#542): a
        // misspelled `link_file_command` or a missing opener used to hit only
        // the log, and the link read as dead — while the path was only ever
        // underlined because it verifiably exists. The built-in editor has
        // its own error path downstream of OpenFileRequested.
        let outcome = match (mode, command) {
            (LinkFileOpen::Command, Some(template)) => {
                run_file_command(&template, &path, line, column)
            }
            (LinkFileOpen::System, _) => open_file_path(&path),
            // Told to run a command, with no command left to run: falling back
            // to the built-in editor beats the click doing nothing.
            (LinkFileOpen::Internal | LinkFileOpen::Command, _) => {
                cx.emit(OpenFileRequested {
                    path,
                    line,
                    column,
                    is_dir,
                });
                return;
            }
        };
        if let Err(e) = outcome {
            self.warn_file_open_failed(&path, &e, window, cx);
        }
    }

    /// Says why a path-shaped token did not open anything.
    ///
    /// Silence here is what makes the whole feature feel broken: a relative
    /// path measured from somewhere other than this pane's directory looks
    /// exactly like one that works, right up until the click does nothing.
    /// Returns whether the click was spoken for.
    fn report_unresolved_link(
        &mut self,
        candidate: &crate::search::FileCandidate,
        pending: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let roots = self.link_roots(cx);
        // A word that is not written like a path was never a link, and saying
        // so on every modifier-click over ordinary output would be noise.
        if !candidate.looks_like_a_path(roots.style) {
            return false;
        }
        // The host has not answered yet. The underline is the promise that it
        // has; before then, accusing it of losing the file would be a guess.
        if pending {
            return false;
        }
        // An absolute or `~`-rooted path was never measured from anywhere, so
        // naming a directory it was "looked for under" would send the user to
        // somewhere nothing was ever asked about.
        let rooted = candidate.is_rooted(roots.style);
        let root = match rooted {
            true => None,
            false => roots.dirs.into_iter().next(),
        };
        let message = match root {
            Some(root) => t_fmt(
                L10nKey::LinkFileNotUnder,
                &[
                    ("path", &candidate.path),
                    ("dir", &root.display().to_string()),
                ],
            ),
            None if rooted => t_fmt(L10nKey::LinkFileMissing, &[("path", &candidate.path)]),
            None => t_fmt(L10nKey::LinkFileNoDirectory, &[("path", &candidate.path)]),
        };
        window.push_notification(message, cx);
        true
    }

    fn open_url(&self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.forwarded_loopback_url(url, cx) {
            LoopbackOpen::Forwarded(url) => cx.open_url(&url),
            LoopbackOpen::NotLoopback => cx.open_url(url),
            LoopbackOpen::ForwardFailed(reason) => {
                window.push_notification(reason, cx);
            }
        }
    }

    fn forwarded_loopback_url(&self, url: &str, cx: &mut Context<Self>) -> LoopbackOpen {
        let plan = self.loopback_plan(cx);
        if matches!(plan, LoopbackPlan::Direct) {
            return LoopbackOpen::NotLoopback;
        }
        let Some(loopback) = crate::backend::loopback::parse_loopback_url(url) else {
            return LoopbackOpen::NotLoopback;
        };
        let forwarded = match &plan {
            LoopbackPlan::ForwardOnPane(_) => self
                .forward_route()
                .ensure_loopback(loopback.forward_host(), loopback.port),
            LoopbackPlan::Direct => unreachable!("handled above"),
        };
        match forwarded {
            Ok(forward) => LoopbackOpen::Forwarded(loopback.forwarded_url(forward.local_port)),
            Err(e) => {
                log::warn!("failed to forward loopback URL {url}: {e}");
                LoopbackOpen::ForwardFailed(t_fmt(
                    L10nKey::LoopbackForwardFailed,
                    &[
                        ("port", &loopback.port.to_string()),
                        ("error", &e.to_string()),
                    ],
                ))
            }
        }
    }

    /// How forward requests about this pane reach the daemon that owns them.
    pub fn forward_route(&self) -> anterm_ui_kit::daemon_rpc::ForwardRoute {
        anterm_ui_kit::daemon_rpc::ForwardRoute::new(self.pane_id)
    }

    pub(super) fn loopback_plan(&self, cx: &gpui::App) -> LoopbackPlan {
        loopback_plan(
            cx.global::<Config>().ssh_loopback_forward,
            self.terminal.remote_context().map(|r| r.kind),
            self.pane_id,
        )
    }

    fn can_forward_loopback(&self, cx: &gpui::App) -> bool {
        !matches!(self.loopback_plan(cx), LoopbackPlan::Direct)
    }

    /// How a loopback port this pane is serving can be reached from here.
    ///
    /// The Ports list asks this about every listener it found, and it is a
    /// different question from "is this pane local": a remote pane's :3000 is
    /// perfectly reachable once a forward exists, and building that forward is
    /// something this app already knows how to do. Answering only "local or
    /// not" is what left the list showing a port it refused to open.
    pub fn port_route(&self, cx: &gpui::App) -> PortRoute {
        port_route_of(&self.loopback_plan(cx), self.host_id().is_local())
    }

    pub fn hover_link_at(
        &mut self,
        col: usize,
        row: usize,
        armed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // A mouse crossing a pane lands on the same cell many times over.
        // Nothing about the answer depends on where inside the cell the
        // pointer is, so the work is worth doing once.
        if self.links.last_hover_cell == Some((col, row)) && self.links.link_modifier_down == armed {
            return self.links.hovered_link.is_some();
        }
        self.links.last_hover_cell = Some((col, row));
        self.links.link_modifier_down = armed;
        if !cx.global::<Config>().link_url {
            self.clear_hovered_link(cx);
            return false;
        }
        // A full-screen application drew what is on the grid and is watching
        // the mouse itself, so pointing things out inside it is anterm drawing
        // on somebody else's window. Holding the modifier says the user wants
        // anterm's reading of the screen anyway, and then it is theirs to have.
        if !armed && self.on_alt_screen() {
            if self.links.hovered_link.take().is_some() {
                self.notify_canvas(cx);
            }
            return false;
        }
        // Most of a screen is blanks, and reading one cell costs a fraction
        // of lifting a whole soft-wrapped logical line out of the grid.
        if self.cell_is_blank(col, row) {
            if self.links.hovered_link.take().is_some() {
                self.notify_canvas(cx);
            }
            return false;
        }
        let include_loopback = self.can_forward_loopback(cx);
        let next = self.link_span_at(col, row, armed, include_loopback, cx);
        if next != self.links.hovered_link {
            self.links.hovered_link = next;
            self.notify_canvas(cx);
        }
        self.links.hovered_link.is_some()
    }

    /// Whether the cell under the pointer holds anything a link could be made
    /// of. Uses `try_lock_unfair` so mouse motion over heavy output never
    /// blocks the UI thread.
    pub(super) fn cell_is_blank(&self, col: usize, row: usize) -> bool {
        use alacritty_terminal::term::cell::Flags;

        let Some(term) = self.terminal.term.try_lock_unfair() else {
            return true;
        };
        let Some(line) = Self::grid_line(&term, row) else {
            return true;
        };
        if col >= term.columns() {
            return true;
        }
        let cell = &term.grid()[line][Column(col)];
        if cell.flags.intersects(
            Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::WIDE_CHAR,
        ) {
            return false;
        }
        cell.c.is_whitespace()
    }

    pub fn refresh_link_hover(&mut self, armed: bool, cx: &mut Context<Self>) -> bool {
        let Some((col, row)) = self.links.last_hover_cell else {
            self.links.link_modifier_down = armed;
            return false;
        };
        self.hover_link_at(col, row, armed, cx)
    }

    /// Runs the hover again from scratch, for when the answer may have
    /// changed under a mouse that never moved: a probe landing, a repository
    /// root arriving.
    fn recompute_link_hover(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((col, row)) = self.links.last_hover_cell.take() else {
            return false;
        };
        self.hover_link_at(col, row, self.links.link_modifier_down, cx)
    }

    pub fn link_modifier_down(&self) -> bool {
        self.links.link_modifier_down
    }

    pub fn clear_hovered_link(&mut self, cx: &mut Context<Self>) {
        self.links.last_hover_cell = None;
        if self.links.hovered_link.take().is_some() {
            self.notify_canvas(cx);
        }
    }

    fn link_span_at(
        &mut self,
        col: usize,
        row: usize,
        armed: bool,
        include_loopback: bool,
        cx: &mut Context<Self>,
    ) -> Option<HoveredLink> {
        match self.resolve_link_inner(col, row, true, include_loopback, true, cx) {
            LinkAt::Found(_, runs) => Some(HoveredLink { runs, armed }),
            LinkAt::Unresolved { .. } | LinkAt::None => None,
        }
    }

    pub(super) fn resolve_link_at(
        &mut self,
        col: usize,
        row: usize,
        include_files: bool,
        include_loopback: bool,
        cx: &mut Context<Self>,
    ) -> LinkAt {
        self.resolve_link_inner(col, row, include_files, include_loopback, false, cx)
    }

    fn resolve_link_inner(
        &mut self,
        col: usize,
        row: usize,
        include_files: bool,
        include_loopback: bool,
        nonblocking: bool,
        cx: &mut Context<Self>,
    ) -> LinkAt {
        let Some(line) = self.link_line_at(col, row, nonblocking) else {
            return LinkAt::None;
        };
        let (text, points, click_idx) = match line {
            GridLink::Hyperlink(uri, runs) => {
                return LinkAt::Found(LinkTarget::Url(uri), runs);
            }
            GridLink::Text(text, points, click_idx) => (text, points, click_idx),
        };

        let files = include_files && self.cwd_is_on_host();
        let roots = match files {
            true => self.link_roots(cx),
            false => crate::search::LinkRoots::default(),
        };
        let local = self.session.host_id.is_local();
        self.links.link_probes.retarget(self.session.host_id);
        let probes = &mut self.links.link_probes;
        let mut pending = false;
        let mut probe = |path: &std::path::Path, require_file: bool| {
            let answer = match (local, nonblocking) {
                (true, true) => probes.probe_local_cached(path, require_file),
                (true, false) => crate::search::local_probe(path, require_file),
                (false, _) => probes.probe(path, require_file),
            };
            pending |= matches!(answer, crate::search::Probe::Unknown);
            answer
        };
        let link = crate::search::link_at(&text, click_idx, &roots, files, &mut probe);
        self.flush_link_probes(cx);

        let link = link.or_else(|| {
            include_loopback.then(|| {
                crate::backend::loopback::loopback_url_span_at(&text, click_idx).map(|(start, end, url)| {
                    crate::search::LinkMatch {
                        start,
                        end,
                        target: LinkTarget::Url(url),
                    }
                })
            })?
        });
        match link {
            Some(link) => {
                let runs = if nonblocking {
                    let Some(term) = self.terminal.term.try_lock_unfair() else {
                        return LinkAt::None;
                    };
                    crate::links::smart_select::row_runs(&term, &points[link.start..=link.end])
                } else {
                    let term = self.terminal.term.lock();
                    crate::links::smart_select::row_runs(&term, &points[link.start..=link.end])
                };
                LinkAt::Found(link.target, runs)
            }
            None => match files
                .then(|| crate::search::unresolved_candidate(&text, click_idx, roots.style))
                .flatten()
            {
                Some(candidate) => LinkAt::Unresolved { candidate, pending },
                None => LinkAt::None,
            },
        }
    }

    /// The logical line under a cell, or the OSC 8 hyperlink that cell
    /// declares — which wins outright, because the emitter said what it meant.
    fn link_line_at(&self, col: usize, row: usize, nonblocking: bool) -> Option<GridLink> {
        let term = if nonblocking {
            self.terminal.term.try_lock_unfair()?
        } else {
            self.terminal.term.lock()
        };
        let line = Self::grid_line(&term, row)?;
        if col >= term.columns() {
            return None;
        }
        let click = Point::new(line, Column(col));

        if let Some(hl) = term.grid()[line][Column(col)].hyperlink() {
            let uri = hl.uri().to_string();
            if let Some((start, end)) = crate::links::smart_select::hyperlink_run(&term, click) {
                let runs = crate::links::smart_select::grid_runs(start, end, term.columns());
                return Some(GridLink::Hyperlink(uri, runs));
            }
        }

        let (text, points, click_idx) = crate::links::smart_select::logical_line_at(&term, click, true)?;
        Some(GridLink::Text(text, points, click_idx))
    }

    /// The directories a relative path in this pane is measured from, best
    /// first: where the work is happening, then the repository around it.
    ///
    /// Cargo, tsc and friends name files from the workspace root while the
    /// shell sits in a member directory, so one root resolves only half of
    /// what a build prints. The pane's own directory stays first, so a name
    /// that exists in both is the near one.
    pub(super) fn link_roots(&mut self, cx: &mut Context<Self>) -> crate::search::LinkRoots {
        let local_home = self.session.host_id.is_local();
        let style = self.link_path_style();
        let Some(cwd) = self.effective_host_cwd() else {
            return crate::search::LinkRoots {
                dirs: Vec::new(),
                local_home,
                style,
            };
        };
        self.request_link_repo_root(&cwd, cx);
        let mut dirs = vec![cwd.clone()];
        if let Some((asked_for, Some(root))) = &self.links.link_repo_root
            && *asked_for == cwd
            && *root != cwd
        {
            dirs.push(root.clone());
        }
        crate::search::LinkRoots {
            dirs,
            local_home,
            style,
        }
    }

    /// Which path dialect this pane's output is written in — see
    /// [`link_path_style`].
    pub(super) fn link_path_style(&self) -> crate::search::PathStyle {
        link_path_style(self.paths_are_local(), self.effective_host_cwd().as_deref())
    }

    fn request_link_repo_root(&mut self, cwd: &std::path::Path, cx: &mut Context<Self>) {
        if self.links.link_repo_root_pending
            || self
                .links
                .link_repo_root
                .as_ref()
                .is_some_and(|(asked_for, _)| asked_for == cwd)
        {
            return;
        }
        // The git-status probe asks this same question about this same
        // directory on its own schedule and files the answer per host, so a
        // hit there costs nothing and saves a round trip. Only a hit: the
        // cache cannot tell "asked, and there is no repository" apart from
        // "never asked", and treating the second as the first would nail a
        // pane to one root forever.
        let cached = cx
            .try_global::<anterm_ui_kit::git::status::GitStatusCache>()
            .and_then(|cache| cache.repo_root_for(self.session.host_id, cwd))
            .map(std::path::Path::to_path_buf);
        if let Some(root) = cached {
            self.links.link_repo_root = Some((cwd.to_path_buf(), Some(root)));
            return;
        }
        let Some(host) = self.host(cx) else {
            return;
        };
        self.links.link_repo_root_pending = true;
        let cwd = cwd.to_path_buf();
        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            {
                let cwd = cwd.clone();
                move |h| h.repo_root(&cwd).ok().flatten()
            },
            move |view, root, cx| {
                view.links.link_repo_root_pending = false;
                view.links.link_repo_root = Some((cwd, root));
                view.recompute_link_hover(cx);
            },
        );
    }

    /// Asks the host about every path a lookup could not answer for, in one
    /// call, and re-runs the hover once the replies are in — the mouse may
    /// have been resting on the link the whole time it took.
    fn flush_link_probes(&mut self, cx: &mut Context<Self>) {
        if self.session.host_id.is_local() {
            return;
        }
        // The host comes first: `take_wanted` moves the paths into the
        // in-flight set on the promise that a call is about to carry them, and
        // a host that has gone away between the hover and here would break
        // that promise permanently — those paths would stay `Unknown` for the
        // life of the pane, which looks exactly like the silent nothing this
        // whole path exists to get rid of.
        let Some(host) = self.host(cx) else {
            return;
        };
        let wanted = self.links.link_probes.take_wanted();
        if wanted.is_empty() {
            return;
        }
        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                use crate::links::link_probe::Existence;
                wanted
                    .into_iter()
                    .map(|path| {
                        let existence = match h.stat(&path) {
                            Ok(meta) if meta.is_dir => Existence::Dir,
                            Ok(_) => Existence::File,
                            Err(_) => Existence::Missing,
                        };
                        (path, existence)
                    })
                    .collect::<Vec<_>>()
            },
            |view, answers, cx| {
                if view.links.link_probes.land(answers) {
                    view.recompute_link_hover(cx);
                }
            },
        );
    }
}
