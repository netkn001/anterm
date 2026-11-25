//! Find in files for the right panel's Files tab: the "In file contents"
//! half of its search, across the active tab's project, on whichever machine
//! that project lives.
//!
//! Runs as an independent [`PanelSearchView`] GPUI entity that owns its search
//! toggles, debounce/generation state, and collapsed file groups, emitting
//! [`WorkspaceIntent::OpenFile`] when a result row is clicked.

mod model;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anterm_core::host::{ContentHit, ContentLimits, ContentQuery};
use anterm_i18n::{L10nKey, t, t_fmt};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, HighlightStyle, MouseButton, Render,
    SharedString, StyledText, Subscription, Window, div, px, rems,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};

use crate::right_panel::{META, ROW_FILL_RADIUS, ROW_GLYPH, ROW_INSET, TEXT_MONO};
use anterm_ui_kit::host_ops::{HostId, HostOps};
use anterm_ui_kit::host_registry::HostRegistry;
use anterm_ui_kit::intent::WorkspaceIntent;

use model::{FileGroup, Outcome, SearchKey, SearchRun};

/// How long typing has to pause before the search goes out. A little longer
/// than the name search's: every keystroke that does get through reads files.
const DEBOUNCE: Duration = Duration::from_millis(250);

const FILE_ROW_H: f32 = 26.;
const HIT_ROW_H: f32 = 22.;
const CHEVRON_W: f32 = 10.;
const ROW_GAP: f32 = 6.;

/// Where a hit's excerpt starts: under its file's name, past the chevron and
/// the icon.
const HIT_LEAD: f32 = ROW_INSET + CHEVRON_W + ROW_GAP + ROW_GLYPH + ROW_GAP;

/// Snapshot of the active workspace target supplied by the Files panel so
/// [`PanelSearchView`] never reaches into `AntermApp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelSearchTarget {
    /// An SSH pane's files are SFTP's, and content search cannot run there.
    SshPane,
    /// Project roots are still being resolved asynchronously.
    Resolving,
    /// The active tab has no directory on its host.
    NoFolder,
    /// Ready to search across `roots` on `host`.
    Ready {
        host: HostId,
        roots: Vec<PathBuf>,
        home: Option<PathBuf>,
    },
}

/// What the body of the search section displays.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Body {
    SshPane,
    NoFolder,
    Resolving,
    Idle(String),
    Searching,
    Results,
    NoMatches(String),
    BadPattern(String),
    ServerTooOld,
    Failed(String),
}

/// Detached GPUI view owning the right panel's "In file contents" search.
pub struct PanelSearchView {
    input: Entity<InputState>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    run: SearchRun,
    /// Files whose hits are folded away. Cleared by a new search, kept by a
    /// refresh of the same one.
    collapsed: HashSet<PathBuf>,
    /// The field takes focus on the next frame — set whenever the tab is
    /// brought forward without a `&mut Window`.
    focus_pending: bool,
    /// Last target snapshot supplied by the Files panel so option toggles can
    /// re-run immediately without waiting for `AntermApp` to re-render.
    last_target: PanelSearchTarget,
    searching: bool,
    _input_sub: Subscription,
}

impl EventEmitter<WorkspaceIntent> for PanelSearchView {}

impl PanelSearchView {
    /// Creates the content search view backed by the Files tab's shared `input`.
    pub fn new(
        input: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.subscribe_in(&input, window, |this, _input, ev, _window, cx| match ev {
            InputEvent::PressEnter { .. } => this.refresh(cx),
            InputEvent::Change => {
                let target = this.last_target.clone();
                this.sync_query_for_target(&target, cx);
                cx.notify();
            }
            _ => {}
        });
        Self {
            input,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            run: SearchRun::default(),
            collapsed: HashSet::new(),
            focus_pending: false,
            last_target: PanelSearchTarget::NoFolder,
            searching: false,
            _input_sub: sub,
        }
    }

    pub fn request_focus(&mut self) {
        self.focus_pending = true;
    }

    pub fn consume_pending_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.focus_pending) {
            self.input.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    /// Synchronizes the active search target and returns whether the content
    /// search section currently has visible content to display.
    ///
    /// Does not call `cx.notify()` synchronously so it is safe to call while
    /// building a frame; asynchronous search completions notify `PanelSearchView`
    /// directly.
    pub fn sync_target(
        &mut self,
        target: PanelSearchTarget,
        searching: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.searching = searching;
        self.last_target = target.clone();
        let body = self.sync_query_for_target(&target, cx);
        searching && !matches!(body, Body::Idle(_))
    }

    fn query(&self, cx: &App) -> ContentQuery {
        ContentQuery {
            pattern: self.input.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
            show_hidden: false,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((generation, key)) = self.run.refresh() else {
            return;
        };
        self.spawn_search(generation, key, Duration::ZERO, cx);
        cx.notify();
    }

    fn retarget(&mut self, key: Option<SearchKey>, cx: &mut Context<Self>) {
        if self.run.wanted() != key.as_ref() {
            self.collapsed.clear();
        }
        if let Some(generation) = self.run.retarget(key.clone())
            && let Some(key) = key
        {
            self.spawn_search(generation, key, DEBOUNCE, cx);
        }
    }

    fn spawn_search(
        &mut self,
        generation: u64,
        key: SearchKey,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let _ = this.update(cx, |this, cx| {
                if !this.run.is_current(generation) {
                    return;
                }
                let Some(host) = HostRegistry::lookup(cx, key.host) else {
                    this.run.accept(
                        generation,
                        Outcome::Failed(t(L10nKey::PanelSearchHostGone).to_string()),
                    );
                    cx.notify();
                    return;
                };
                let SearchKey { roots, query, .. } = key;
                HostOps::run(
                    host,
                    cx,
                    move |h| {
                        Outcome::from_result(h.search_content(
                            &roots,
                            &query,
                            &ContentLimits::default(),
                        ))
                    },
                    move |this, outcome, cx| {
                        if this.run.accept(generation, outcome) {
                            cx.notify();
                        }
                    },
                );
            });
        })
        .detach();
    }

    fn sync_query_for_target(
        &mut self,
        target: &PanelSearchTarget,
        cx: &mut Context<Self>,
    ) -> Body {
        let (host, roots, home) = match target {
            PanelSearchTarget::SshPane => {
                self.retarget(None, cx);
                return Body::SshPane;
            }
            PanelSearchTarget::Resolving => {
                return Body::Resolving;
            }
            PanelSearchTarget::NoFolder => {
                self.retarget(None, cx);
                return Body::NoFolder;
            }
            PanelSearchTarget::Ready { host, roots, home } => (*host, roots.clone(), home.clone()),
        };
        if roots.is_empty() {
            self.retarget(None, cx);
            return Body::NoFolder;
        }
        let query = self.query(cx);
        if query.pattern.is_empty() {
            self.retarget(None, cx);
            let root = roots
                .iter()
                .map(|r| {
                    anterm_ui_kit::path_display::abbreviate_home(
                        &r.to_string_lossy(),
                        home.as_deref(),
                    )
                    .into_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Body::Idle(root);
        }
        let pattern = query.pattern.clone();
        self.retarget(
            Some(SearchKey {
                host,
                roots,
                query,
            }),
            cx,
        );
        self.body_for_pattern(pattern)
    }

    fn body_for_pattern(&self, pattern: String) -> Body {
        let run = &self.run;
        let Some(landed) = run.landed().filter(|l| Some(&l.key) == run.wanted()) else {
            return match run.landed() {
                Some(l)
                    if matches!(l.outcome, Outcome::Found(_))
                        && !l.groups.is_empty()
                        && run.wanted().is_some_and(|w| w.host == l.key.host) =>
                {
                    Body::Results
                }
                _ => Body::Searching,
            };
        };
        match &landed.outcome {
            Outcome::Found(_) if landed.groups.is_empty() => Body::NoMatches(pattern),
            Outcome::Found(_) => Body::Results,
            Outcome::BadPattern(e) => Body::BadPattern(e.clone()),
            Outcome::ServerTooOld => Body::ServerTooOld,
            Outcome::Failed(e) => Body::Failed(e.clone()),
        }
    }

    fn current_body(&self, cx: &App) -> Body {
        match &self.last_target {
            PanelSearchTarget::SshPane => Body::SshPane,
            PanelSearchTarget::Resolving => Body::Resolving,
            PanelSearchTarget::NoFolder => Body::NoFolder,
            PanelSearchTarget::Ready { roots, home, .. } => {
                if roots.is_empty() {
                    return Body::NoFolder;
                }
                let pattern = self.input.read(cx).value().to_string();
                if pattern.is_empty() {
                    let root = roots
                        .iter()
                        .map(|r| {
                            anterm_ui_kit::path_display::abbreviate_home(
                                &r.to_string_lossy(),
                                home.as_deref(),
                            )
                            .into_owned()
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Body::Idle(root);
                }
                self.body_for_pattern(pattern)
            }
        }
    }

    fn tally(&self, body: &Body) -> Option<String> {
        let landed = self.run.landed()?;
        match (body, &landed.outcome) {
            (Body::Results, Outcome::Found(_)) => {
                Some(model::hit_count(&landed.groups).to_string())
            }
            _ => None,
        }
    }

    /// Renders the `Aa` / `ab` / `.*` option toggles embedded inside the Files
    /// tab's shared search well.
    pub fn render_toggles(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let toggle = |id: &'static str,
                      label: &'static str,
                      tip: L10nKey,
                      on: bool,
                      flip: fn(&mut PanelSearchView)| {
            Button::new(id)
                .label(label)
                .ghost()
                .xsmall()
                .selected(on)
                .when(!on, |b| b.text_color(muted))
                .accessibility_label(t(tip))
                .tooltip(t(tip))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    flip(this);
                    let target = this.last_target.clone();
                    this.sync_query_for_target(&target, cx);
                    cx.notify();
                }))
        };
        h_flex()
            .flex_none()
            .gap_0p5()
            .child(toggle(
                "panel-search-case",
                "Aa",
                L10nKey::SearchMatchCase,
                self.case_sensitive,
                |s| s.case_sensitive = !s.case_sensitive,
            ))
            .child(toggle(
                "panel-search-word",
                "ab",
                L10nKey::PanelSearchWholeWord,
                self.whole_word,
                |s| s.whole_word = !s.whole_word,
            ))
            .child(toggle(
                "panel-search-regex",
                ".*",
                L10nKey::SearchUseRegex,
                self.regex,
                |s| s.regex = !s.regex,
            ))
            .into_any_element()
    }

    fn render_rows(&self, body: Body, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let danger = cx.theme().danger;
        let note = |text: String, hint: Option<String>, ink: Option<gpui::Hsla>, cx: &App| {
            let muted = cx.theme().muted_foreground;
            v_flex()
                .flex_none()
                .px(px(ROW_INSET))
                .py_1()
                .gap(gpui::rems(3. / 16.))
                .text_size(rems(crate::right_panel::TEXT))
                .text_color(ink.unwrap_or(muted))
                .child(text)
                .children(hint.map(|h| div().text_size(rems(META)).text_color(muted).child(h)))
                .into_any_element()
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        match body {
            Body::SshPane => rows.push(note(
                t(L10nKey::PanelSearchSshPane).into(),
                Some(t(L10nKey::PanelSearchSshPaneHint).into()),
                None,
                cx,
            )),
            Body::NoFolder => rows.push(note(
                t(L10nKey::PanelSearchNoFolder).into(),
                Some(t(L10nKey::PanelSearchNoFolderHint).into()),
                None,
                cx,
            )),
            Body::Resolving | Body::Searching => rows.push(note(
                t(L10nKey::PanelSearchSearching).into(),
                None,
                None,
                cx,
            )),
            Body::Idle(root) => rows.push(note(
                t(L10nKey::PanelSearchIdle).into(),
                Some(root),
                None,
                cx,
            )),
            Body::NoMatches(query) => rows.push(note(
                t_fmt(L10nKey::PanelSearchNoMatches, &[("query", &query)]),
                None,
                None,
                cx,
            )),
            Body::BadPattern(e) => rows.push(note(
                t_fmt(L10nKey::PanelSearchBadPattern, &[("e", first_line(&e))]),
                None,
                Some(danger),
                cx,
            )),
            Body::ServerTooOld => rows.push(note(
                t(L10nKey::PanelSearchServerTooOld).into(),
                Some(t(L10nKey::PanelSearchServerTooOldHint).into()),
                Some(danger),
                cx,
            )),
            Body::Failed(e) => rows.push(note(
                t_fmt(L10nKey::PanelSearchFailed, &[("e", &e)]),
                None,
                Some(danger),
                cx,
            )),
            Body::Results => rows.extend(self.render_results(cx)),
        }
        rows
    }

    fn render_results(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(landed) = self.run.landed() else {
            return Vec::new();
        };
        let Outcome::Found(found) = &landed.outcome else {
            return Vec::new();
        };
        let muted = cx.theme().muted_foreground;
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.run.running() {
            rows.push(
                div()
                    .flex_none()
                    .px(px(ROW_INSET))
                    .pb_1()
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(t(L10nKey::PanelSearchSearching))
                    .into_any_element(),
            );
        }
        for (index, group) in landed.groups.iter().enumerate() {
            let collapsed = self.collapsed.contains(&group.path);
            rows.push(self.render_file_row(index, group, collapsed, cx));
            if !collapsed {
                for (n, hit) in group.hits.iter().enumerate() {
                    rows.push(self.render_hit_row(index, n, hit, cx));
                }
            }
        }
        if found.truncated {
            rows.push(
                div()
                    .flex_none()
                    .px(px(ROW_INSET))
                    .pt_1p5()
                    .text_size(rems(META))
                    .italic()
                    .text_color(muted)
                    .child(t(L10nKey::PanelSearchTruncated))
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_file_row(
        &self,
        index: usize,
        group: &FileGroup,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().popover;
        let muted = cx.theme().muted_foreground;
        let path = group.path.clone();
        h_flex()
            .id(("panel-search-file", index))
            .flex_none()
            .items_center()
            .gap(px(ROW_GAP))
            .h(px(FILE_ROW_H))
            .px(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .child(
                div()
                    .flex_none()
                    .w(px(CHEVRON_W))
                    .flex()
                    .justify_center()
                    .child(
                        Icon::new(match collapsed {
                            true => IconName::ChevronRight,
                            false => IconName::ChevronDown,
                        })
                        .size(px(CHEVRON_W))
                        .text_color(muted),
                    ),
            )
            .child(
                Icon::new(IconName::File)
                    .size(px(ROW_GLYPH))
                    .text_color(muted),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap_1p5()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .text_color(cx.theme().sidebar_foreground)
                            .child(SharedString::from(group.name.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(rems(META))
                            .text_color(muted)
                            .child(SharedString::from(group.dir.clone())),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(gpui::rems(5. / 16.))
                    .rounded_sm()
                    .bg(cx.theme().muted)
                    .text_size(rems(crate::right_panel::META_MONO))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(muted)
                    .child(group.hits.len().to_string()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if !this.collapsed.remove(&path) {
                        this.collapsed.insert(path.clone());
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn render_hit_row(
        &self,
        file: usize,
        n: usize,
        hit: &ContentHit,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().popover;
        let mark = HighlightStyle {
            background_color: Some(cx.theme().yellow.opacity(0.35)),
            ..Default::default()
        };
        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = hit
            .ranges
            .iter()
            .filter(|r| {
                (r.end as usize) <= hit.text.len()
                    && hit.text.is_char_boundary(r.start as usize)
                    && hit.text.is_char_boundary(r.end as usize)
                    && r.start < r.end
            })
            .map(|r| (r.start as usize..r.end as usize, mark))
            .collect();
        let text =
            StyledText::new(SharedString::from(hit.text.clone())).with_highlights(highlights);
        let (path, line, column) = (hit.path.clone(), hit.line, hit.column);
        let tip = t_fmt(
            L10nKey::PanelSearchLineTooltip,
            &[("line", &line.to_string()), ("column", &column.to_string())],
        );
        div()
            .id(("panel-search-hit", file * 1_000_000 + n))
            .flex_none()
            .flex()
            .items_center()
            .h(px(HIT_ROW_H))
            .pl(px(HIT_LEAD))
            .pr(px(ROW_INSET))
            .rounded(ROW_FILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(rems(TEXT_MONO))
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().sidebar_foreground)
                    .child(text),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _, _window, cx| {
                    cx.emit(WorkspaceIntent::OpenFile {
                        path: path.clone(),
                        line: Some(line),
                        column: Some(column),
                    });
                }),
            )
            .into_any_element()
    }
}

impl Render for PanelSearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.searching {
            return div().into_any_element();
        }
        let body = self.current_body(cx);
        if matches!(body, Body::Idle(_)) {
            return div().into_any_element();
        }
        let tally = self.tally(&body);
        v_flex()
            .flex_none()
            .child(crate::file_tree::search_section_heading(
                t(L10nKey::PanelSearchInContents),
                tally,
                cx,
            ))
            .children(self.render_rows(body, cx))
            .into_any_element()
    }
}

/// A regex parser's error is a small diagram over several lines; the panel
/// has room for its verdict, which is the last one.
fn first_line(e: &str) -> &str {
    e.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(e)
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use anterm_ui_kit::config::Config;
    use gpui::{Focusable as _, TestAppContext, VisualTestContext};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anterm-psearch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn mount_view(
        cx: &mut TestAppContext,
        root: PathBuf,
    ) -> (Entity<InputState>, Entity<PanelSearchView>, VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(Config::default());
            anterm_ui_kit::presets::load_registry(cx);
            anterm_ui_kit::theme::apply_theme(None, cx);
            let _ = HostRegistry::local(cx);
        });
        let mut input_slot = None;
        let mut view_slot = None;
        let window = cx.add_window(|window, cx| {
            let input = cx.new(|cx| InputState::new(window, cx));
            let view = cx.new(|cx| PanelSearchView::new(input.clone(), window, cx));
            view.update(cx, |v, cx| {
                v.sync_target(
                    PanelSearchTarget::Ready {
                        host: HostId::LOCAL,
                        roots: vec![root],
                        home: None,
                    },
                    true,
                    cx,
                );
            });
            input_slot = Some(input);
            view_slot = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let vcx = VisualTestContext::from_window(window.into(), cx);
        (input_slot.unwrap(), view_slot.unwrap(), vcx)
    }

    fn wait_landed(view: &Entity<PanelSearchView>, vcx: &mut VisualTestContext) -> Outcome {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            vcx.executor().advance_clock(DEBOUNCE);
            vcx.run_until_parked();
            let got = view.read_with(vcx, |v, _| {
                let wanted = v.run.wanted();
                v.run
                    .landed()
                    .filter(|l| !v.run.running() && Some(&l.key) == wanted)
                    .map(|l| l.outcome.clone())
            });
            if let Some(outcome) = got {
                return outcome;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "panel search never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    #[gpui::test]
    fn standalone_view_searches_groups_toggles_and_clears(cx: &mut TestAppContext) {
        let root = scratch("standalone");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "fn needle() {}\nlet x = 1;\n// Needle\n",
        )
        .unwrap();
        std::fs::write(root.join("notes.md"), "a needle here\n").unwrap();

        let (input, view, mut vcx) = mount_view(cx, root.clone());

        // Pending focus hands focus to the shared input.
        view.update_in(&mut vcx, |v, window, cx| {
            v.request_focus();
            v.consume_pending_focus(window, cx);
        });
        vcx.run_until_parked();
        assert!(vcx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window)));

        // Case-insensitive search lands grouped hits.
        input.update_in(&mut vcx, |i, window, cx| {
            i.set_value("needle", window, cx);
            cx.emit(InputEvent::Change);
        });
        assert!(matches!(wait_landed(&view, &mut vcx), Outcome::Found(_)));
        let lines: Vec<(String, Vec<u32>)> = view.read_with(&vcx, |v, _| {
            v.run
                .landed()
                .unwrap()
                .groups
                .iter()
                .map(|g| (g.name.clone(), g.hits.iter().map(|h| h.line).collect()))
                .collect()
        });
        assert_eq!(
            lines,
            vec![
                ("notes.md".to_string(), vec![1]),
                ("lib.rs".to_string(), vec![1, 3]),
            ]
        );

        // Enabling case-sensitivity re-searches immediately without external driver.
        view.update(&mut vcx, |v, cx| {
            v.case_sensitive = true;
            let target = v.last_target.clone();
            v.sync_query_for_target(&target, cx);
        });
        assert!(matches!(wait_landed(&view, &mut vcx), Outcome::Found(_)));
        let lines_cs: Vec<(String, Vec<u32>)> = view.read_with(&vcx, |v, _| {
            v.run
                .landed()
                .unwrap()
                .groups
                .iter()
                .map(|g| (g.name.clone(), g.hits.iter().map(|h| h.line).collect()))
                .collect()
        });
        assert_eq!(
            lines_cs,
            vec![
                ("notes.md".to_string(), vec![1]),
                ("lib.rs".to_string(), vec![1]),
            ]
        );

        // Invalid regex reports BadPattern.
        view.update(&mut vcx, |v, _| v.regex = true);
        input.update_in(&mut vcx, |i, window, cx| {
            i.set_value("(", window, cx);
            cx.emit(InputEvent::Change);
        });
        assert!(matches!(
            wait_landed(&view, &mut vcx),
            Outcome::BadPattern(_)
        ));

        // Clearing the field clears landed results.
        input.update_in(&mut vcx, |i, window, cx| {
            i.set_value("", window, cx);
            cx.emit(InputEvent::Change);
        });
        vcx.run_until_parked();
        assert!(view.read_with(&vcx, |v, _| v.run.landed().is_none()));

        let _ = std::fs::remove_dir_all(&root);
    }
}
