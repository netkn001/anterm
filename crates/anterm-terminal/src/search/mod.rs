use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::search::{Match, RegexSearch};
use gpui::{Context, Entity, Subscription, Window, div, prelude::*, px};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, Size,
};

use super::view::TerminalView;
use anterm_i18n::{L10nKey, t};

mod links;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use links::{file_candidates_at, spelled_for, url_at};
pub(super) use links::{
    FileCandidate, LinkMatch, LinkRoots, LinkTarget, PathStyle, Probe, is_url_char, link_at,
    local_probe, unresolved_candidate, url_span_at,
};

const MAX_MATCHES: usize = 10_000;

/// How long a printing pane has to stay quiet before an open search bar
/// rescans it. Short enough that a command's output is re-counted by the time
/// the eye gets back to the bar, long enough that a flood costs one scan per
/// pause rather than one per frame.
pub(super) const SCAN_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(120);

/// How many debounce windows a pane that never stops printing may push the
/// rescan out before it gets one anyway.
///
/// A debounce alone waits for a pause, and a coding agent streaming a reply
/// does not give one for a minute at a time — the count would sit frozen for
/// the whole flood and the highlights would sit on text that has since been
/// rewritten under them. Rebasing keeps a highlight under text that *scrolled*;
/// only a scan catches up with text that changed in place.
pub(super) const SCAN_LAG_LIMIT: u32 = 4;

/// The search bar floats over the top of the grid rather than pushing it down,
/// so these two decide how many rows it hides. Keep them next to the `.top()`
/// and `.h()` that use them — a match parked under the bar is on screen and
/// still unreadable, which is the one thing "next match" must never do.
pub(super) const BAR_TOP: f32 = 8.;
pub(super) const BAR_WIDTH: f32 = 400.;
pub(super) const BAR_HEIGHT: f32 = 34.;

pub struct SearchState {
    pub input: Entity<InputState>,
    pub matches: Vec<Match>,
    pub current_index: Option<usize>,
    /// Scrollback depth the `matches` lines are counted against. Match points
    /// are grid-relative, so this is what turns one back into the absolute row
    /// it named — see [`TerminalView::refresh_matches_after_output`]. It is the
    /// depth at the last scan until output scrolls the grid, which moves the
    /// lines and this together (see
    /// [`TerminalView::rebase_matches_after_scroll`]) so the anchors hold.
    pub(crate) scanned_history: usize,
    _subs: Vec<Subscription>,
}

impl SearchState {
    pub fn current(&self) -> Option<&Match> {
        self.current_index.and_then(|i| self.matches.get(i))
    }
}

/// One read of the grid: every match in it, whether the pattern compiled, and
/// how deep the scrollback was at the time.
struct Scan {
    matches: Vec<Match>,
    regex_error: bool,
    history: usize,
}

impl Scan {
    fn empty(regex_error: bool) -> Self {
        Self {
            matches: Vec::new(),
            regex_error,
            history: 0,
        }
    }
}

/// A match point's row counted from the top of scrollback, which survives the
/// grid scrolling under it — the same anchor a command mark or a placed image
/// uses, with the same caveat once the scrollback is full and the discard count
/// stops being observable.
fn anchor_row(history: usize, point: &Point) -> i64 {
    history as i64 + point.line.0 as i64
}

fn scan_matches_on_term<T: EventListener>(
    term_lock: &alacritty_terminal::sync::FairMutex<Term<T>>,
    pattern: &str,
) -> Scan {
    let mut matches: Vec<Match> = Vec::new();
    let Ok(mut regex) = RegexSearch::new(pattern) else {
        return Scan::empty(true);
    };
    let term = term_lock.lock();
    let grid = term.grid();
    let history = grid.history_size();
    let mut origin = Point::new(grid.topmost_line(), Column(0));

    while matches.len() < MAX_MATCHES {
        let Some(m) = term.search_next(&mut regex, origin, Direction::Right, Side::Left, None)
        else {
            break;
        };
        if matches.last().is_some_and(|last| m.start() <= last.start()) {
            break;
        }
        origin = m.end().add(grid, Boundary::None, 1);
        let wrapped = origin <= *m.end();
        matches.push(m);
        if wrapped {
            break;
        }
    }
    Scan {
        matches,
        regex_error: false,
        history,
    }
}

impl TerminalView {
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fresh = self.search_state.active.is_none();
        // When the query is seeded from the grid selection, that selection is
        // the thing being searched for — not a casualty of opening the bar.
        // `recompute_matches` clears it because its other callers are the user
        // *changing* the query (typing, toggles), where the old selection no
        // longer names anything; here the query IS the selection, so it is
        // put back after the scan (#584).
        let mut seeded_selection = None;
        if fresh {
            let seed = self.selected_search_seed();
            if seed.is_some() {
                seeded_selection = self.terminal.term.lock().selection.clone();
            }
            let seed = seed.unwrap_or_else(|| self.search_state.last_query.clone());
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(t(L10nKey::SearchFind))
                    .default_value(seed)
            });
            let subs = vec![cx.subscribe_in(&input, window, Self::on_search_event)];
            self.search_state.active = Some(SearchState {
                input,
                matches: Vec::new(),
                current_index: None,
                scanned_history: 0,
                _subs: subs,
            });
        }
        if let Some(input) = self.search_state.active.as_ref().map(|s| s.input.clone()) {
            input.update(cx, |state, cx| state.focus(window, cx));
            // The box keeps the last query on purpose — reopening on the word
            // you just looked for is most of what a find bar is for — so the
            // caret must not treat it as text to type around.
            anterm_ui_kit::prefill::select_all_when_drawn(&input, window, cx);
        }
        if fresh {
            self.recompute_matches(cx);
            if let Some(selection) = seeded_selection {
                self.terminal.term.lock().selection = Some(selection);
            }
        }
        cx.notify();
    }

    fn selected_search_seed(&self) -> Option<String> {
        let text = self.terminal.term.lock().selection_to_string()?;
        let trimmed = text.trim_matches(['\n', '\r']);
        if trimmed.is_empty() || trimmed.contains('\n') {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.search_state.active.as_ref() {
            self.search_state.last_query = s.input.read(cx).value().to_string();
        }
        self.search_state.active = None;
        self.search_state.focused = false;
        self.search_state.regex_error = false;
        // The grid selection stays: it is the user's, not the bar's. The same
        // discipline `refresh_matches_after_output` states — the search bar
        // opening and closing around a selection must not erase it (#584).
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        _input: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => self.recompute_matches(cx),
            InputEvent::PressEnter { shift, .. } => {
                let dir = if *shift {
                    Direction::Left
                } else {
                    Direction::Right
                };
                self.step_match(dir, cx);
            }
            InputEvent::Focus => {
                self.search_state.focused = true;
                cx.notify();
            }
            InputEvent::Blur => {
                self.search_state.focused = false;
                cx.notify();
            }
        }
    }

    /// Every match of the current query in the whole grid, and whether the
    /// pattern failed to compile.
    ///
    /// A match point is a line *of the grid it was read from*, and output
    /// scrolling the grid makes every one of them name a different line —
    /// which is what [`TerminalView::rebase_matches_after_scroll`] undoes
    /// between scans. Nothing here caches, and both callers re-read the grid.
    fn scan_matches(&self, query: &str) -> Scan {
        if query.is_empty() {
            return Scan::empty(false);
        }
        let pattern = self.effective_search_pattern(query);
        scan_matches_on_term(&self.terminal.term, &pattern)
    }

    /// The match a fresh query starts on: the last one at or above the bottom
    /// of what is on screen, so Enter walks forward from where the eye is.
    fn match_nearest_the_viewport(&self, matches: &[Match]) -> Option<usize> {
        if matches.is_empty() {
            return None;
        }
        let term = self.terminal.term.lock();
        let grid = term.grid();
        let display_offset = grid.display_offset() as i32;
        let bottom = Point::new(
            Line(grid.screen_lines() as i32 - 1 - display_offset),
            grid.last_column(),
        );
        Some(
            matches
                .iter()
                .rposition(|m| *m.start() <= bottom)
                .unwrap_or(0),
        )
    }

    /// Note that output changed the grid an open search bar is describing.
    ///
    /// Rebasing runs lock-free against the atomic history mirror on every
    /// wakeup; the full regex rescan is debounced and executed on the
    /// background executor so scrolling through 10,000+ scrollback lines during
    /// active agent output never stalls the UI thread.
    pub(super) fn note_output_under_search(&mut self, cx: &mut Context<Self>) {
        if self.search_state.active.is_none() {
            return;
        }
        self.rebase_matches_after_scroll(cx);
        self.search_state.scan_epoch = self.search_state.scan_epoch.wrapping_add(1);
        if self.search_state.scan_armed {
            return;
        }
        self.search_state.scan_armed = true;
        cx.spawn(async move |this, cx| {
            let mut waited = 0;
            loop {
                let Ok(epoch) = this.update(cx, |view, _| view.search_state.scan_epoch) else {
                    break;
                };
                cx.background_executor().timer(SCAN_DEBOUNCE).await;
                waited += 1;
                let request = this.update(cx, |view, cx| {
                    if view.search_state.scan_epoch != epoch && waited < SCAN_LAG_LIMIT {
                        return Ok(None);
                    }
                    view.search_state.scan_armed = false;
                    let Some(query) = view
                        .search_state
                        .active
                        .as_ref()
                        .map(|s| s.input.read(cx).value().to_string())
                    else {
                        return Err(());
                    };
                    let pattern = if query.is_empty() {
                        String::new()
                    } else {
                        view.effective_search_pattern(&query)
                    };
                    Ok(Some((
                        view.terminal.term.clone(),
                        query,
                        pattern,
                        view.search_state.scan_epoch,
                    )))
                });
                match request {
                    Ok(Ok(None)) => continue,
                    Ok(Ok(Some((term_arc, query, pattern, req_epoch)))) => {
                        let query_empty = query.is_empty();
                        let scan = cx
                            .background_spawn(async move {
                                if query_empty {
                                    Scan::empty(false)
                                } else {
                                    scan_matches_on_term(&term_arc, &pattern)
                                }
                            })
                            .await;
                        let _ = this.update(cx, |view, cx| {
                            // Drop stale results if search closed or the query changed.
                            let Some(live_query) = view
                                .search_state
                                .active
                                .as_ref()
                                .map(|s| s.input.read(cx).value().to_string())
                            else {
                                return;
                            };
                            if live_query != query {
                                return;
                            }
                            let _ = req_epoch;
                            view.apply_output_scan(scan, cx);
                        });
                        break;
                    }
                    _ => break,
                }
            }
        })
        .detach();
    }

    fn rebase_matches_after_scroll(&mut self, cx: &mut Context<Self>) {
        let history = self.terminal.history_size_nonblocking();
        let Some(s) = self.search_state.active.as_mut() else {
            return;
        };
        let drift = history as i64 - s.scanned_history as i64;
        if drift == 0 {
            return;
        }
        if drift < 0 {
            self.refresh_matches_after_output(cx);
            return;
        }

        let drift = drift as i32;
        let topmost = -(history as i32);
        let moved = |p: &Point| Point::new(Line(p.line.0 - drift), p.column);
        let gone = s
            .matches
            .partition_point(|m| m.start().line.0 - drift < topmost);
        s.matches.drain(..gone);
        for m in &mut s.matches {
            let (start, end) = (moved(m.start()), moved(m.end()));
            *m = start..=end;
        }
        s.current_index = s.current_index.and_then(|i| i.checked_sub(gone));
        s.scanned_history = history;
    }

    fn apply_output_scan(&mut self, scan: Scan, cx: &mut Context<Self>) {
        let previous = self
            .search_state
            .active
            .as_ref()
            .and_then(|s| Some((*s.current()?.start(), s.scanned_history)))
            .map(|(p, history)| (anchor_row(history, &p), p.column));
        let current_index = previous
            .and_then(|(row, column)| {
                scan.matches.iter().position(|m| {
                    anchor_row(scan.history, m.start()) == row && m.start().column == column
                })
            })
            .or_else(|| self.match_nearest_the_viewport(&scan.matches));

        if let Some(s) = self.search_state.active.as_mut() {
            s.matches = scan.matches;
            s.current_index = current_index;
            s.scanned_history = scan.history;
        }
        self.search_state.regex_error = scan.regex_error;
        cx.notify();
    }

    /// Re-run the query against the grid as it now stands, keeping the count
    /// and the highlights honest while the pane is still printing.
    pub(super) fn refresh_matches_after_output(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self
            .search_state
            .active
            .as_ref()
            .map(|s| s.input.read(cx).value().to_string())
        else {
            return;
        };
        let scan = self.scan_matches(&query);
        self.apply_output_scan(scan, cx);
    }

    pub(super) fn recompute_matches(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self
            .search_state
            .active
            .as_ref()
            .map(|s| s.input.read(cx).value().to_string())
        else {
            return;
        };

        let scan = self.scan_matches(&query);
        let current_index = self.match_nearest_the_viewport(&scan.matches);

        if let Some(s) = self.search_state.active.as_mut() {
            s.matches = scan.matches;
            s.current_index = current_index;
            s.scanned_history = scan.history;
        }
        self.search_state.regex_error = scan.regex_error;

        let current = self.search_state.active.as_ref().and_then(|s| s.current().cloned());
        let hidden = self.rows_behind_the_search_bar();
        let mut term = self.terminal.term.lock();
        term.selection = None;
        if let Some(m) = current {
            scroll_match_into_view(&mut term, &m, hidden);
        }
        self.terminal.publish_term_state(&term);
        drop(term);
        cx.notify();
    }

    pub(super) fn step_match(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let current = {
            let Some(s) = self.search_state.active.as_mut() else {
                return;
            };
            if s.matches.is_empty() {
                return;
            }
            let len = s.matches.len();
            let cur = s.current_index.unwrap_or(0);
            let next = match direction {
                Direction::Right => (cur + 1) % len,
                Direction::Left => (cur + len - 1) % len,
            };
            s.current_index = Some(next);
            s.matches[next].clone()
        };
        let hidden = self.rows_behind_the_search_bar();
        let mut term = self.terminal.term.lock();
        scroll_match_into_view(&mut term, &current, hidden);
        self.terminal.publish_term_state(&term);
        drop(term);
        cx.notify();
    }

    fn rows_behind_the_search_bar(&self) -> i32 {
        rows_under_the_bar(self.render.line_height.as_f32())
    }

    fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        if self.search_state.active.is_none() {
            return;
        }
        self.search_state.case_sensitive = !self.search_state.case_sensitive;
        self.recompute_matches(cx);
    }

    fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        if self.search_state.active.is_none() {
            return;
        }
        self.search_state.regex = !self.search_state.regex;
        self.recompute_matches(cx);
    }

    fn effective_search_pattern(&self, query: &str) -> String {
        let base = if self.search_state.regex {
            query.to_string()
        } else {
            regex_escape(query)
        };
        if self.search_state.case_sensitive {
            format!("(?-i){base}")
        } else {
            base
        }
    }

    pub(super) fn render_search_bar(
        &self,
        state: &SearchState,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let popover = theme.popover;
        let danger = theme.red;

        let total = state.matches.len();
        let has_query = !state.input.read(cx).value().is_empty();
        let has_matches = !state.matches.is_empty();
        let regex_error = self.search_state.regex_error;
        let case_on = self.search_state.case_sensitive;
        let regex_on = self.search_state.regex;

        let field = Input::new(&state.input)
            .appearance(false)
            .with_size(Size::Small);

        let count = has_query.then(|| {
            let current = if has_matches {
                state.current_index.map(|i| i + 1).unwrap_or(0)
            } else {
                0
            };
            // The scan stops at MAX_MATCHES. Without the mark, a query that hit
            // the ceiling reads as if the scrollback held exactly that many.
            let more = match total >= MAX_MATCHES {
                true => "+",
                false => "",
            };
            div()
                .flex_none()
                .text_xs()
                .text_color(muted)
                .child(format!("{current}/{total}{more}"))
        });

        let case_toggle = Button::new("search-case")
            .label("Aa")
            .ghost()
            .small()
            .selected(case_on)
            .tooltip(t(L10nKey::SearchMatchCase))
            .on_click(cx.listener(|this, _, _window, cx| {
                this.toggle_search_case(cx);
            }));
        let regex_toggle = Button::new("search-regex")
            .label(".*")
            .ghost()
            .small()
            .selected(regex_on)
            .tooltip(t(L10nKey::SearchUseRegex))
            .on_click(cx.listener(|this, _, _window, cx| {
                this.toggle_search_regex(cx);
            }));

        let divider = div().flex_none().w(px(1.)).h(px(16.)).bg(border);

        let prev = Button::new("search-prev")
            .icon(IconName::ChevronUp)
            .ghost()
            .small()
            .tooltip(t(L10nKey::AppMenuFindPrevious))
            .disabled(!has_matches)
            .on_click(cx.listener(|this, _, _window, cx| {
                this.step_match(Direction::Left, cx);
            }));
        let next = Button::new("search-next")
            .icon(IconName::ChevronDown)
            .ghost()
            .small()
            .tooltip(t(L10nKey::AppMenuFindNext))
            .disabled(!has_matches)
            .on_click(cx.listener(|this, _, _window, cx| {
                this.step_match(Direction::Right, cx);
            }));
        let close = Button::new("search-close")
            .icon(IconName::Close)
            .ghost()
            .small()
            .tooltip(t(L10nKey::Close))
            .on_click(cx.listener(|this, _, window, cx| {
                this.close_search(window, cx);
            }));

        div()
            .occlude()
            .flex()
            .items_center()
            .gap_1p5()
            .w(px(400.))
            .h(px(BAR_HEIGHT))
            .pl_3()
            .pr_1()
            .rounded_lg()
            .border_1()
            // Focus is shown by the caret, not by the frame: an accent border
            // over the bar's drop shadow read as a glow.
            .border_color(if regex_error { danger } else { border })
            .bg(popover)
            .shadow_md()
            .child(div().flex_1().min_w_0().child(field))
            .children(count)
            .child(case_toggle)
            .child(regex_toggle)
            .child(divider)
            .child(prev)
            .child(next)
            .child(close)
    }
}

/// How many grid rows the floating bar covers, at this line height.
fn rows_under_the_bar(line_height: f32) -> i32 {
    if line_height <= 0. {
        return 0;
    }
    ((BAR_TOP + BAR_HEIGHT) / line_height).ceil() as i32
}

#[derive(Debug, PartialEq, Eq)]
enum Reveal {
    /// Already clear of the bar and inside the viewport.
    Stay,
    /// Below the viewport — alacritty's own minimal scroll lands it on the
    /// last row, which nothing covers.
    ToPoint,
    /// Above the readable area. `scroll_to_point` would park it on row 0, the
    /// row most likely to be behind the bar, so walk back this many lines and
    /// land it just clear instead.
    Back(i32),
}

fn reveal(line: i32, display_offset: i32, screen_lines: i32, hidden: i32) -> Reveal {
    let top = -display_offset + hidden;
    let bottom = screen_lines - 1 - display_offset;
    if line > bottom {
        Reveal::ToPoint
    } else if line < top {
        Reveal::Back(top - line)
    } else {
        Reveal::Stay
    }
}

fn scroll_match_into_view<T: EventListener>(term: &mut Term<T>, m: &Match, hidden: i32) {
    let grid = term.grid();
    let display_offset = grid.display_offset() as i32;
    let screen_lines = grid.screen_lines() as i32;
    match reveal(m.start().line.0, display_offset, screen_lines, hidden) {
        Reveal::Stay => {}
        Reveal::ToPoint => term.scroll_to_point(*m.start()),
        Reveal::Back(lines) => term.scroll_display(Scroll::Delta(lines)),
    }
}

fn regex_escape(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for c in query.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
