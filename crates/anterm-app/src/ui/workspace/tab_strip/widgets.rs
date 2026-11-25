use gpui::{
    App, Context, Edges, Pixels, SharedString, Window, div, prelude::*, px,
};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::core::shells::DetectedShell;
use crate::daemon::protocol::ShellSpec;
use crate::ui::workspace::app::{SpawnWhere, AntermApp};
use anterm_i18n::{L10nKey, t, t_fmt};
use crate::ui::chrome::search::SearchTab;

/// One duration and one curve for every transition the app runs, so a fade and
/// a slide read as the same hand. Long enough to be seen as movement, short
/// enough that nobody waits on it.
pub(crate) const TRANSITION_MS: u64 = 140;
pub(crate) const REORDER_SLIDE_MS: u64 = TRANSITION_MS;
pub(super) const CHIP_GAP: f32 = 6.;

pub(crate) const GRAB_HANDLE_W: f32 = 80.;

const KEEP_SEGMENTS: usize = 3;

/// Builds a launch specification without recomputing argument ownership locally.
/// The inventory may originate from a remote host, so only its transported
/// metadata can distinguish anterm launch defaults from user-authored arguments.
pub(super) fn shell_spec(shell: &DetectedShell) -> ShellSpec {
    ShellSpec {
        program: shell.program.clone(),
        args: shell.args.clone(),
        args_are_anterm_defaults: shell.args_are_anterm_defaults,
    }
}

/// Shared with the switcher's tab column and the CLI, which name tabs of
/// workspaces this process does not own and have to cut the same head off the
/// same titles.
pub(crate) use anterm_core::tab_view::strip_host_prefix;

/// `home` is the home directory of the machine `path` is on, from the
/// workspace's host; `None` leaves the path spelled out (#580).
pub(crate) fn abbreviate_home<'a>(
    path: &'a str,
    home: Option<&std::path::Path>,
) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    if path.starts_with('~') {
        return Cow::Borrowed(path);
    }
    // The shared comparison: separators normalized, case folded — a Windows
    // pane whose cwd spells itself `C:/Users/…` shortens under a
    // `C:\Users\…` home too (#544).
    crate::ui::files::path_display::abbreviate_home(path, home)
}

/// The separator a path spells itself with. A path carrying a single `\` is
/// a Windows path and has to be put back together with `\`: rejoining it with
/// `/` would make one tab spell its location two ways, `C:\Users\dev\app`
/// while it fits and `C:/…/app` once it has to be elided.
fn path_separator(path: &str) -> char {
    if path.contains('\\') { '\\' } else { '/' }
}

fn join_segments(segments: &[&str], sep: char) -> String {
    segments.join(sep.encode_utf8(&mut [0u8; 4]) as &str)
}

pub(crate) fn short_title(raw: &str, home: Option<&std::path::Path>) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let after_host = strip_host_prefix(raw);
    let after_host = after_host.trim();
    if after_host.is_empty() {
        return String::new();
    }
    let abbreviated = abbreviate_home(after_host, home);
    let path: &str = abbreviated.as_ref();

    enum Kind {
        Home,
        Absolute,
        Relative,
    }
    let (kind, body) = if let Some(rest) = path.strip_prefix("~/") {
        (Kind::Home, rest)
    } else if path == "~" {
        return "~".to_string();
    } else if let Some(rest) = path.strip_prefix('/') {
        (Kind::Absolute, rest)
    } else {
        (Kind::Relative, path)
    };

    // Both separators: Windows shells report `C:\Users\…` while git and the
    // terminal integration use `/`, and a path must be cut on either one.
    let segments: Vec<&str> = body.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return match kind {
            Kind::Home => "~",
            Kind::Absolute => "/",
            Kind::Relative => "",
        }
        .to_string();
    }

    let sep = path_separator(path);
    let depth = segments.len() + usize::from(matches!(kind, Kind::Home));
    let mut label = if depth > KEEP_SEGMENTS {
        let tail = &segments[segments.len() - KEEP_SEGMENTS..];
        format!("…{sep}{}", join_segments(tail, sep))
    } else {
        match kind {
            Kind::Home => format!("~{sep}{}", join_segments(&segments, sep)),
            Kind::Absolute => format!("/{}", join_segments(&segments, sep)),
            Kind::Relative => join_segments(&segments, sep),
        }
    };
    // Clamped on cluster boundaries, or a label ending in an emoji comes back
    // holding half of one.
    let cells = clusters(&label);
    if cells.len() > 40 {
        label = format!("{}…", cells[..40].concat());
    }
    label
}

/// The one place a tab gets its displayed name, whichever surface is asking.
///
/// `label()` ranks the evidence — a given name, then the title the pane is
/// showing, then an agent, then the working directory, then the process it is
/// running — and this renders whatever came back. Both callers arrive with a
/// [`TabView`](crate::ui::remote::machine_mirror::TabView): the switcher reads one out
/// of the machine tree for a window it does not own, and the strip builds one
/// from its own live panes in
/// [`Tab::label_view`](crate::ui::workspace::app::Tab::label_view).
///
/// They used to rank their own evidence, and disagreed where it mattered most:
/// a pane with a working directory and no title — every non-PowerShell shell
/// anterm ships integration for reports OSC 7 and no OSC 0 — was listed by the
/// switcher as `~/repo/anterm` and by the strip that owned it as "anterm", the
/// app's own name (#740).
pub(crate) fn label_of(
    view: &crate::ui::remote::machine_mirror::TabView,
    index: usize,
    home: Option<&std::path::Path>,
) -> String {
    use crate::ui::remote::machine_mirror::TabLabel;

    let unnamed = || {
        t_fmt(
            L10nKey::TabUnnamedShell,
            &[("n", &((index + 1).to_string()))],
        )
    };
    // A path can shorten away to nothing (a bare "user@host:"), and the process
    // name the tree carries ("zsh") is still worth more than a number.
    //
    // Through `stated_title` because a tab of *this* window has no process name
    // to offer: `Tab::label_view` fills that slot with the placeholder a pane
    // answers to before anything has spoken, and printing the app's own name
    // here is the one thing #740 exists to stop. Nothing to say falls to the
    // number, which is what the strip showed before it shared this renderer.
    let shortened = |raw: &str| match short_title(raw, home) {
        shortened if !shortened.trim().is_empty() => shortened,
        _ => match crate::terminal::view::stated_title(&view.title) {
            Some(title) => title.to_string(),
            None => unnamed(),
        },
    };
    match view.label() {
        TabLabel::Named(name) => name.to_string(),
        // Through `short_title` because a title is so often a path: the shell
        // integration writes `user@host:~/dir`, and a tab spelling that out in
        // full where the one beside it says "…/dir" would be the same
        // disagreement in a new place.
        TabLabel::Osc(title) => shortened(title),
        TabLabel::Agent(agent) => agent.display_name().to_string(),
        TabLabel::Cwd(cwd) => shortened(cwd),
        TabLabel::Process(title) => title.to_string(),
        TabLabel::Unknown => unnamed(),
    }
}

/// What a row can add on hover: the name behind the one [`label_of`] cut down,
/// or `None` when it cut nothing and the tooltip would only repeat the row.
///
/// The comparison has to happen on the *same* spelling, which is the whole
/// trick here. `label_of` abbreviates a path under the home before it elides
/// it, and this returns the abbreviated form too, so a raw `/Users/x/repo`
/// measured against a label of `~/repo` looks like a difference that isn't
/// one — and every tab named after a directory inside the home would hang a
/// tooltip saying exactly what it already says. Abbreviate first, compare
/// after.
pub(super) fn tooltip_of(
    view: &crate::ui::remote::machine_mirror::TabView,
    index: usize,
    home: Option<&std::path::Path>,
) -> Option<SharedString> {
    use crate::ui::remote::machine_mirror::TabLabel;

    // The other rungs are never shortened: a given name and a process name are
    // printed whole, and an agent's is a word.
    let raw = match view.label() {
        TabLabel::Osc(title) => title,
        TabLabel::Cwd(cwd) => cwd,
        _ => return None,
    };
    let full = abbreviate_home(raw.trim(), home);
    if full.trim().is_empty() || full.as_ref() == label_of(view, index, home).as_str() {
        return None;
    }
    Some(SharedString::from(full.into_owned()))
}

/// Width of `text` shaped in `font` at `size`, in pixels.
///
/// The window's text system caches shaped runs, so measuring the same labels
/// across frames is cheap. The sidebar elides against real glyph widths
/// instead of guessing at character counts — that is the only way a mixed
/// CJK/Latin label can be squeezed without tearing mid-token.
pub(crate) use anterm_ui_kit::window_chrome::measure_text;

/// Elides a path from the front when it cannot fit `max_width`, keeping the
/// root marker (drive letter, `~`, or the leading slash) and every trailing
/// segment that fits.
///
/// The tail is what a user identifies a tab by — the file or directory they
/// are actually working on — so it is never torn: whole segments drop off the
/// front first (a half-eaten directory name reads as noise), and when even
/// the last segment is too wide, only that segment is elided character by
/// character, still tail-first.
pub(crate) fn elide_path_keep_tail(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    path: &str,
    max_width: f32,
) -> SharedString {
    let path = path.trim();
    if path.is_empty() || measure_text(text_system, font, size, path) <= max_width {
        return SharedString::from(path);
    }
    let sep = path_separator(path);
    let segments: Vec<&str> = path.split(['/', '\\']).collect();
    // A leading slash splits into an empty first segment; `~` and drive
    // letters (`E:`) carry the same "where this tree lives" weight, and a
    // leading `…` means `short_title` already elided once — that marker is
    // replaced by the new elision instead of stacking two ellipses. Keep
    // whichever marker there is so the result never reads as a bare
    // relative path.
    let root: &str = match segments.first() {
        Some(&"") => "/",
        Some(&"~") => "~",
        Some(&"…") => "",
        Some(head) if head.ends_with(':') => head,
        _ => "",
    };
    let root_kept = segments
        .first()
        .is_some_and(|s| s.is_empty() || *s == "~" || *s == "…" || s.ends_with(':'));
    let prefix = if root.is_empty() {
        format!("…{sep}")
    } else if root == "/" {
        // The absolute-path root is already the slash itself.
        format!("/…{sep}")
    } else {
        format!("{root}{sep}…{sep}")
    };
    // Drop whole segments from the front until the remaining tail fits. The
    // width only shrinks as segments leave, so the first fit is the widest
    // one — greedy is optimal here.
    //
    // With a root marker, `head = 1` would spell the root, the ellipsis, and
    // then every remaining segment — strictly wider than the original that
    // already failed to fit — so that candidate is skipped rather than
    // measured.
    let mut head = if root_kept { 2 } else { 0 };
    while head < segments.len() {
        let candidate = if head == 0 {
            join_segments(&segments, sep)
        } else {
            format!("{prefix}{}", join_segments(&segments[head..], sep))
        };
        if measure_text(text_system, font, size, &candidate) <= max_width {
            return SharedString::from(candidate);
        }
        if head + 1 >= segments.len() {
            break;
        }
        head += 1;
    }
    // Even the last segment alone is too wide: keep its tail after the
    // ellipsis, with no slash so the reader sees the segment was torn.
    elide_tail_clusters(
        text_system,
        font,
        size,
        segments[segments.len() - 1],
        max_width,
    )
}

/// Characters a token is allowed to break on. Space is in the set because
/// this also elides labels a human typed — `Backend server logs` — not just
/// branch names, and a word boundary is the cut a reader forgives.
const TOKEN_BREAKS: [char; 5] = ['-', '_', '/', '.', ' '];

/// Splits `text` into grapheme clusters — what a reader counts as one
/// character, and the only place a label may be cut.
///
/// Slicing by `char` passes every width check and still tears the result:
/// `👨‍👩‍👧` loses the joiner holding it together, `❤️` loses the variation
/// selector that makes it an emoji (and the orphan then attaches itself to the
/// ellipsis), and `🇨🇳` leaves behind a lone regional indicator that renders
/// as a bare letter.
pub(super) fn clusters(text: &str) -> Vec<&str> {
    text.graphemes(true).collect()
}

/// The head this token would rather keep: six clusters, extended to just past
/// the next break so the cut lands on a boundary (`window-…` rather than
/// `window…`). When no break is within reach the plain six is kept — running
/// on to the cap would spend the whole budget on a prefix and leave the tail,
/// which is what identifies the token, with nothing.
fn preferred_head(clusters: &[&str]) -> usize {
    let base = clusters.len().min(6);
    let cap = clusters.len().min(12);
    if base >= cap {
        return base;
    }
    match clusters[base..cap]
        .iter()
        .position(|c| c.chars().next().is_some_and(|c| TOKEN_BREAKS.contains(&c)))
    {
        Some(offset) => base + offset + 1,
        None => base,
    }
}

/// Elides the middle of a single token (a branch name, a shell name, a label
/// the user typed) so both its head and its identifying tail survive:
/// `window-transparency-backdrop` reads `window-…backdrop` in a narrow
/// sidebar instead of losing its tail to a trailing ellipsis.
///
/// A head that fits but leaves no room for a tail is worse than no head at
/// all, so the preferred head is given up for a shorter one when that is what
/// it takes to keep a few trailing clusters; only when even a three-cluster
/// head cannot buy a tail does this fall back to a tail-only elision.
pub(crate) fn elide_keep_edges(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    let text = text.trim();
    if text.is_empty() || measure_text(text_system, font, size, text) <= max_width {
        return SharedString::from(text);
    }
    let cells = clusters(text);
    let shaped = |head_n: usize, tail_n: usize| -> f32 {
        let mut s = cells[..head_n].concat();
        s.push('…');
        s.push_str(&cells[cells.len() - tail_n..].concat());
        measure_text(text_system, font, size, &s)
    };
    // Width is monotone in the tail length, so a binary search finds the
    // longest tail that still fits behind a given head.
    let longest_tail = |head_n: usize| -> usize {
        let (mut lo, mut hi) = (0usize, cells.len() - head_n);
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            if shaped(head_n, mid) <= max_width {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    };
    /// Enough trailing clusters to tell two sibling branches apart.
    const MIN_TAIL: usize = 3;
    let preferred = preferred_head(&cells);
    let mut candidates = vec![preferred, 6, 3];
    candidates.retain(|&h| h > 0 && h <= cells.len());
    candidates.dedup();
    let mut best: Option<(usize, usize)> = None;
    for head_n in candidates {
        if shaped(head_n, 0) > max_width {
            continue;
        }
        let tail = longest_tail(head_n);
        if tail >= MIN_TAIL.min(cells.len() - head_n) {
            best = Some((head_n, tail));
            break;
        }
        if best.is_none_or(|(_, best_tail)| tail > best_tail) {
            best = Some((head_n, tail));
        }
    }
    let Some((head, tail)) = best.filter(|&(_, tail)| tail > 0) else {
        // No head buys a tail worth showing; a bare tail says more.
        return elide_tail_clusters(text_system, font, size, text, max_width);
    };
    let mut out = cells[..head].concat();
    out.push('…');
    out.push_str(&cells[cells.len() - tail..].concat());
    SharedString::from(out)
}

/// Elides a row label. A path keeps its tail — the file or directory being
/// worked on — while anything else keeps both edges.
///
/// A shell title is not always a path: `npm run dev`, `man git-log`, or a name
/// the user typed into the rename box. Running those through the path rule
/// drops their head, which is the part that names them, and `… server logs`
/// says less than the CSS truncation this replaced.
pub(crate) fn elide_label(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    if text.contains('/') || text.contains('\\') {
        elide_path_keep_tail(text_system, font, size, text, max_width)
    } else {
        elide_keep_edges(text_system, font, size, text, max_width)
    }
}

/// Keeps the longest head of `text` that fits before a trailing ellipsis —
/// the plain cut a reader expects, for a string read left to right like a
/// branch name.
pub(crate) fn elide_end_clusters(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    if measure_text(text_system, font, size, text) <= max_width {
        return SharedString::from(text.to_string());
    }
    let budget = max_width - measure_text(text_system, font, size, "…");
    if budget <= 0. {
        return SharedString::from("…");
    }
    let cells = clusters(text);
    let (mut lo, mut hi) = (0usize, cells.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if measure_text(text_system, font, size, &cells[..mid].concat()) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out = cells[..lo].concat();
    out.push('…');
    SharedString::from(out)
}

/// Keeps the longest tail of `text` that fits after a bare ellipsis. Shared
/// by the path and token elisions as their last resort.
pub(crate) fn elide_tail_clusters(
    text_system: &gpui::WindowTextSystem,
    font: &gpui::Font,
    size: f32,
    text: &str,
    max_width: f32,
) -> SharedString {
    // Nothing to cut. The search below is bounded by the budget *after* the
    // ellipsis, so without this a branch that fit came back as "…" plus the
    // whole of itself — the rail's group header printed `…feat/v4-redesign`
    // with room to spare.
    if measure_text(text_system, font, size, text) <= max_width {
        return SharedString::from(text.to_string());
    }
    let budget = max_width - measure_text(text_system, font, size, "…");
    if budget <= 0. {
        return SharedString::from("…");
    }
    let cells = clusters(text);
    let (mut lo, mut hi) = (0usize, cells.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let s = cells[cells.len() - mid..].concat();
        if measure_text(text_system, font, size, &s) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return SharedString::from("…");
    }
    let mut out = String::with_capacity(1 + text.len());
    out.push('…');
    out.push_str(&cells[cells.len() - lo..].concat());
    SharedString::from(out)
}

#[derive(Clone)]
pub(crate) struct DragTab;

impl Render for DragTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// What a chrome tile says on hover: what it does, then the chord that does it.
/// The tile's own name is no use as a tooltip — the workspace head already
/// wears it as its label.
///
/// This used to be `chord_hint`, which pasted the two together into one string
/// — `"Hide sidebar  \u{2318}B"` — and handed that to `Button::tooltip`. Inside the
/// card the chord then wore the label's own size and colour, so the tooltip
/// read as one odd sentence rather than as a name with a shortcut beside it.
/// `Tooltip` has a `key_binding` slot that already renders a chord the way a
/// chord should look — set apart on the right, a size down, in
/// `muted_foreground` — and all that was missing was a way to hand `Button` a
/// built tooltip instead of a string, which is what `tooltip_element` is for.
pub(crate) fn chord_tooltip(
    what: impl Into<SharedString>,
    action: &str,
    cx: &gpui::App,
) -> impl Fn(&mut Window, &mut App) -> gpui::Entity<Tooltip> + 'static {
    let what: SharedString = what.into();
    // Resolved now, while there is a `cx`: the builder below runs on hover, and
    // is handed only the window it is drawing into.
    let kbd = crate::ui::workspace::home::key_stroke(action, cx).map(Kbd::new);
    move |_window, cx| cx.new(|_| Tooltip::new(what.clone()).key_binding(kbd.clone()))
}

pub(crate) use crate::ui::chrome::window_chrome::{
    RAIL_TILE_GLYPH, RAIL_TILE_RADIUS, ROW_STATUS_DOT, ROW_STATUS_SLOT, chrome_tile,
    chrome_tile_sized,
};

/// WCAG 2.2 SC 2.5.8 puts the desktop floor for a pointer target at 24×24, and
/// gpui-component renders an icon-only `.xsmall()` button as a 20×20 box (18×18
/// where the chrome overrode it). Grow only the box: the glyph keeps its size,
/// so the chrome looks unchanged and simply stops being fiddly to hit.
pub(crate) use anterm_ui_kit::window_chrome::{MIN_TARGET, hit_target};

/// The narrowest a chip gets: its `min_w`, which flex-shrink cannot go under.
pub(super) const CHIP_MIN_W: f32 = 100.;

/// How wide the tab strip is drawn: the content width less what the row
/// reserves beside it (the lead, plus the window buttons or a docked column),
/// never below `floor`.
///
/// Under Linux client-side decorations `viewport_w` measures the whole surface,
/// shadow included, while the title bar row sits inside the frame padding —
/// so both paddings come off before anything else does. Sizing the strip to
/// the raw viewport overran the frame by exactly that much and clipped the
/// close button (#1169). `window_paddings` answers `Edges::all(0)` under
/// server-side decorations and on a maximized or tiled CSD window, so
/// everywhere else this is the width the strip always had.
pub(super) fn strip_width(viewport_w: Pixels, pad: Edges<Pixels>, reserved: f32, floor: f32) -> Pixels {
    (viewport_w - pad.left - pad.right - px(reserved)).max(px(floor))
}

/// The run of chips to draw when they cannot all fit.
///
/// The row clips what overflows, so past a certain tab count the chips at the
/// end simply were not drawn — including, right after ⌘T, the tab that was
/// just opened and made active. Slide the run instead: keep it anchored at the
/// first tab until the active one would fall off the right edge, then move it
/// by as little as it takes to hold the active chip.
pub(super) fn visible_chips(order: &[usize], active: usize, avail: f32) -> Vec<usize> {
    let fits = ((avail / (CHIP_MIN_W + CHIP_GAP)).floor() as usize).max(1);
    if order.len() <= fits {
        return order.to_vec();
    }
    let at = order.iter().position(|&i| i == active).unwrap_or(0);
    let start = at.saturating_sub(fits - 1).min(order.len() - fits);
    order[start..start + fits].to_vec()
}



/// The right panel's tabs, left to right, by name. Info leads as the default
/// and the pane's overview; then the project's files, and its version
/// control (Changes). Search is not among them: the Files tab searches names
/// and contents in one field.
pub(crate) use anterm_ui_kit::window_chrome::{trailing_chrome_tiles_w, trailing_chrome_w};



/// How many saved hosts the New Tab menu names.
///
/// Sorted by frecency, so the ones actually used are the ones that fit. A menu
/// is not a search field — past a handful the list stops being scannable, and
/// the search's Hosts tab already lists every host and can filter. The row that
/// closes the section is where the rest are.
pub(super) const MENU_HOSTS: usize = 6;

/// How many shells the New Tab menu names, the default among them.
///
/// The same reasoning as [`MENU_HOSTS`], at a smaller number: a stock macOS box
/// reports nine shells and almost nobody opens more than one or two of them.
/// The default always leads — it is the answer to "what do I get if I just
/// click" — and the rest of the rows go to whatever has actually been opened,
/// by frecency. Everything else is one row away, in the search.
pub(super) const MENU_SHELLS: usize = 3;

/// How wide the New Tab menu is allowed to get.
const MENU_W: Pixels = px(360.);

/// How tall, before it starts scrolling.
///
/// The lists are capped — [`MENU_SHELLS`] shells, [`MENU_AGENTS`] agents and
/// [`MENU_HOSTS`] hosts — so the menu's full hand is a fixed number of rows:
/// those, the three headings, the row closing each section, the separators and
/// the modifier hint, at the 26px a row occupies. The Agents section is absent
/// when nothing is offered, and this still leaves room above the full hand for
/// a seam row or two more, so it always arrives whole and never scrolls on its
/// own. It is capped again against the window in [`NewTabMenu::build`], since
/// a menu taller than what it hangs off is worse than one that scrolls.
const MENU_H: Pixels = px(720.);

/// How this platform spells the key that turns a New Tab row into a split.
fn split_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Alt"
    }
}

/// What the New Tab menu offers, read off the app when the menu is opened.
///
/// The builder runs on the popup's own entity, so what a row needs to name
/// itself is taken here and carried in. Taken on open rather than on render:
/// the strip redraws on every frame a terminal paints, and the host list is
/// worth sorting once per menu, not once per frame — and a host saved while
/// the window sat still is in the list the next time the `+` is pressed.
pub(super) struct NewTabMenu {
    pub(super) app: gpui::WeakEntity<AntermApp>,
    /// The shells the menu names, by label — at most [`MENU_SHELLS`].
    pub(super) shells: Vec<SharedString>,
    pub(super) default_shell: SharedString,
    /// The inventory holds shells the menu does not name, so the section
    /// closes with a row into the palette.
    pub(super) more_shells: bool,
    /// Saved host, its display name, and the `user@host:port` beside it —
    /// empty when the name already says it.
    pub(super) hosts: Vec<(uuid::Uuid, SharedString, SharedString)>,
}

impl NewTabMenu {
    pub(super) fn build(&self, menu: PopupMenu, window: &Window) -> PopupMenu {
        // Whatever [`MENU_H`] asks for, a menu still has to fit the window it
        // hangs off — on a short one the list gives way, not the window.
        //
        // Measured off the viewport, not `window_bounds()`: that one answers
        // "how should this window be reopened after it is closed", so on macOS
        // a fullscreen window reports the small bounds it would restore to,
        // not the screen it currently fills. A terminal spends much of its
        // life fullscreen, and reading that would cap the menu at 80% of a
        // window nobody is looking at — putting back the scrollbar and the
        // cut-off `Local` this whole change is here to remove.
        let ceiling = MENU_H.min(window.viewport_size().height * 0.8);
        let mut menu = menu
            .min_w(px(240.))
            // A menu is a list of names, not a place to read a full address.
            // Left to itself the panel widens to its longest row — one saved
            // host with a descriptive name and a long `user@host` drags every
            // other row out with it and the menu stops looking like chrome.
            .max_w(MENU_W)
            // Both lists are capped, but a short window is not. Past the height
            // of the window an un-scrollable menu simply loses its last rows —
            // and the last rows here are the SSH section.
            .scrollable(true)
            // Our own ceiling rather than `PopupMenu`'s built-in 450px, so the
            // one that applies is the one [`MENU_H`] reasons about.
            .max_h(ceiling)
            .item(PopupMenuItem::label(t(L10nKey::TabMenuLocalShells)));
        for label in &self.shells {
            let app = self.app.clone();
            let open = label.clone();
            let row = if *label == self.default_shell {
                let label = label.clone();
                PopupMenuItem::element(move |_window, cx| {
                    menu_row(label.clone(), t(L10nKey::ShellDefault).into(), cx)
                })
            } else {
                PopupMenuItem::new(label.clone())
            };
            menu = menu.item(row.on_click(move |_, window, cx| {
                let at = SpawnWhere::from_modifiers(window.modifiers());
                if let Some(app) = app.upgrade() {
                    app.update(cx, |this, cx| this.open_listed_shell(&open, at, window, cx));
                }
            }));
        }
        // No inventory yet — the machine has not answered, or this is a host
        // that reports none. The default shell is still openable.
        if self.shells.is_empty() {
            let app = self.app.clone();
            menu = menu.item(PopupMenuItem::new(t(L10nKey::AppMenuNewTab)).on_click(
                move |_, window, cx| {
                    let at = SpawnWhere::from_modifiers(window.modifiers());
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |this, cx| this.open_shell(None, at, window, cx));
                    }
                },
            ));
        }
        // The rest of the inventory is in the search, already filtered to
        // it — the same seam the SSH section closes with. Absent when the
        // rows above are the whole inventory: a row into a list of nothing
        // new would be one more thing to read for no gain.
        if self.more_shells {
            let app = self.app.clone();
            menu = menu.item(PopupMenuItem::new(t(L10nKey::TabMenuOtherShells)).on_click(
                move |_, window, cx| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |this, cx| {
                            this.open_search(SearchTab::Shells, "", window, cx)
                        });
                    }
                },
            ));
        }

        menu = menu
            .item(PopupMenuItem::separator())
            .item(PopupMenuItem::label(t(L10nKey::CmdGroupSsh)));
        for (id, name, endpoint) in &self.hosts {
            let (id, name, endpoint) = (*id, name.clone(), endpoint.clone());
            let app = self.app.clone();
            // Every host row is a custom element, note or not: a plain item
            // renders its label as bare text with nothing to elide against,
            // and a host saved on its address alone is *named* `user@host:port`
            // — the longest string in the menu, on the row least able to cut
            // it. [`menu_row`] drops the right half when there is no note.
            let row = PopupMenuItem::element(move |_window, cx| {
                menu_row(name.clone(), endpoint.clone(), cx)
            });
            menu = menu.item(row.on_click(move |_, window, cx| {
                let at = SpawnWhere::from_modifiers(window.modifiers());
                if let Some(app) = app.upgrade() {
                    app.update(cx, |this, cx| {
                        this.connect_ssh_profile_at(id, at, window, cx)
                    });
                }
            }));
        }
        // With no hosts saved, the row that closes the section is the one that
        // gets you your first — the list of hosts is not somewhere to send
        // someone who has none.
        let app = self.app.clone();
        let empty = self.hosts.is_empty();
        let last = if empty {
            L10nKey::TabMenuAddHost
        } else {
            L10nKey::TabMenuAllHosts
        };
        menu = menu.item(PopupMenuItem::new(t(last)).on_click(move |_, window, cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |this, cx| {
                    if empty {
                        this.open_new_ssh_host(window, cx);
                    } else {
                        this.open_search(SearchTab::Hosts, "", window, cx);
                    }
                });
            }
        }));

        // The one place ⌥ is spelled out. Nothing else in the app teaches it,
        // and a modifier nobody is told about is a feature nobody has. No rule
        // above it: a separator divides two lists of things to pick, and this
        // is a footnote about the list it follows, not a section of its own.
        menu.item(PopupMenuItem::label(t_fmt(
            L10nKey::TabMenuSplitHint,
            &[("key", split_modifier())],
        )))
    }
}

/// Every shell in the inventory, most likely first: the default, then whatever
/// has been opened often and recently, then the rest in the inventory's own
/// order. Shared by the palette and the New Tab menu so the same shell leads
/// both lists.
pub(crate) fn shells_by_frecency<'a>(
    shells: &'a [DetectedShell],
    default: &str,
    usage: &std::collections::HashMap<String, crate::core::config::ProfileUsage>,
    now: u64,
) -> Vec<&'a DetectedShell> {
    let score = |s: &DetectedShell| usage.get(&s.label).map_or(0.0, |u| u.score(now));
    let mut sorted: Vec<&DetectedShell> = shells.iter().collect();
    // Stable, so shells nobody has opened keep the order the inventory chose.
    sorted.sort_by(|a, b| {
        (b.label == default)
            .cmp(&(a.label == default))
            .then_with(|| {
                score(b)
                    .partial_cmp(&score(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    sorted
}

/// The shells the menu names, off the front of [`shells_by_frecency`]'s list:
/// its head — the default, whenever the inventory has it — and then only
/// shells that have actually been opened, [`MENU_SHELLS`] rows in all.
///
/// A shell nobody has opened is not named just because there is room: a menu
/// that filled its spare rows with `csh` and `tcsh` would be the nine-row list
/// again, only shorter.
pub(super) fn menu_shells<'a>(
    sorted: &[&'a DetectedShell],
    default: &str,
    usage: &std::collections::HashMap<String, crate::core::config::ProfileUsage>,
    now: u64,
) -> Vec<&'a DetectedShell> {
    sorted
        .iter()
        .take(MENU_SHELLS)
        .enumerate()
        .filter(|(i, s)| {
            *i == 0 || s.label == default || usage.get(&s.label).is_some_and(|u| u.score(now) > 0.0)
        })
        .map(|(_, s)| *s)
        .collect()
}

/// The hosts the menu names, in the order they were handed over — frecency,
/// so the ones that fit are the ones actually used.
///
/// A host saved without a name has nothing to show but where it goes, so it
/// leads with the endpoint rather than leaving the row blank — and then drops
/// the endpoint beside it, because a row that says `root@build.lan` twice
/// tells the reader nothing the first half did not.
pub(super) fn menu_hosts(
    profiles: Vec<crate::core::ssh_profile::SshProfile>,
) -> Vec<(uuid::Uuid, SharedString, SharedString)> {
    profiles
        .into_iter()
        .take(MENU_HOSTS)
        .map(|p| {
            let endpoint = crate::core::ssh_profile::to_connect_string(&p);
            let name = if p.name.trim().is_empty() {
                endpoint.clone()
            } else {
                p.name.clone()
            };
            let note = if name == endpoint {
                String::new()
            } else {
                endpoint
            };
            (p.id, SharedString::from(name), SharedString::from(note))
        })
        .collect()
}

/// A menu row that names a thing on the left and says what it is on the right.
///
/// Both halves are cut rather than allowed to push: the panel stops at
/// [`MENU_W`], and a descriptive host name next to a long `user@host:port`
/// asks for more than that. Which half gives way is the whole point — the name
/// is what the reader is picking by, so it keeps its width up to
/// [`MENU_NAME_W`], and the endpoint takes whatever is left and elides first.
/// Sized the other way round (name growing from nothing, endpoint at its
/// natural width) a long address squeezes the name down to `..` and the row
/// names nothing at all.
///
/// Both caps are fixed lengths, never a share of the row. The panel is sized
/// to fit its content, so while the text is laid out no row has a width yet:
/// anything capped by `relative(..)` or by flex shrinking measures at full
/// length, is squeezed afterwards, and is clipped mid-glyph at the panel edge
/// (`git@ssh.github.con`) instead of ending in an ellipsis. A pixel cap is
/// known up front, so the text elides against it. The two caps and the row's
/// insets add up to [`MENU_W`].
///
/// An empty note drops the right half entirely rather than leaving a zero-width
/// child to hold the `gap_3` open — the name is then free to use the full row,
/// still eliding at the panel edge.
fn menu_row(label: SharedString, note: SharedString, cx: &gpui::App) -> impl IntoElement + use<> {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_shrink_0()
                .max_w(match note.is_empty() {
                    true => MENU_W - MENU_ROW_INSET,
                    false => MENU_NAME_W,
                })
                .truncate()
                .child(label),
        )
        .when(!note.is_empty(), |this| {
            // Pushed right by the cell, not by `text_right`: a line set
            // with `truncate` ignores its alignment and hugs the name.
            this.child(
                h_flex().flex_1().justify_end().child(
                    div()
                        .max_w(MENU_NOTE_W)
                        .truncate()
                        .text_color(muted)
                        .child(note),
                ),
            )
        })
}

/// The most of a [`menu_row`] its name keeps before it elides — the larger
/// share, since the name is what the row is picked by.
const MENU_NAME_W: Pixels = px(172.);

/// The most the endpoint beside a name gets: room for `user@host:port` on a
/// typical host, and the gap and the insets take the rest of [`MENU_W`].
const MENU_NOTE_W: Pixels = px(150.);

/// What the panel spends on either side of a row's content: its own 5px
/// inset and the row's 8px padding, twice.
const MENU_ROW_INSET: Pixels = px(26.);

/// The words behind the status dot's colour.
pub(crate) fn agent_status_label(
    status: Option<crate::core::cli_agent::AgentStatus>,
) -> Option<&'static str> {
    use crate::core::cli_agent::AgentStatus;
    match status? {
        AgentStatus::Idle => None,
        AgentStatus::Working => Some(t(L10nKey::AgentStatusWorking)),
        AgentStatus::Waiting => Some(t(L10nKey::AgentStatusWaiting)),
        AgentStatus::Done => Some(t(L10nKey::AgentStatusDone)),
    }
}
