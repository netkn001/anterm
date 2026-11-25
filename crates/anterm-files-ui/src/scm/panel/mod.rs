//! The source control panel body: four groups of file rows over the working
//! tree status, in the order git itself talks about them.
//!
//! Hosted by standalone [`ScmPanelView`] (`impl Render`, `impl EventEmitter<ScmEvent>`),
//! with changed-file rows virtualized through `gpui_kit::component::v_virtual_list`.

mod commit;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Focusable as _, Pixels, SharedString, Size, Window, div, prelude::*, px,
    relative, rems, size,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
    v_virtual_list,
};

use anterm_core::git::ops::GitOp;
use anterm_core::git::status::{DecoStatus, HeadState, RepoPath, StatusEntry, WorkingTreeStatus};
use anterm_i18n::{L10nKey, t, t_fmt, t_plural};

use anterm_ui_kit::git::data::status_of;
use anterm_ui_kit::rounding::{HAIRLINE, ROW_RADIUS, RoundedCorners as _, segment_corners};
use crate::right_panel::{
    META, META_MONO, ROW_INSET, SEARCH_H, action_strip, git_badge, info_chip, panel_empty_view,
    panel_search_box,
};
use crate::scm::ScmIntent;
use crate::scm::path::split_display_path;
use crate::scm::state::{RepoKey, ScmEvent, ScmGroup, ScmListItem, ScmPanelView};
use crate::scm::status::status_color;
use crate::scm::tree::{TreeRow, normalize_query, path_matches, tree_rows};
use anterm_ui_kit::host_ops::{HostId, SharedHost};
use anterm_ui_kit::window_chrome::{CONTENT_INSET, TILE_GLYPH_XS, TILE_SIZE_XS};

pub use commit::{
    RowVerb, commit_plan, commit_stages_everything, group_diff_source, group_verbs, head_label,
    in_group, operation_label, pushable_branch, row_status, row_verbs, starts_collapsed,
    tracking_chip, verb_op, writable_paths,
};
use commit::{
    branch_note, commit_half, commit_paint, field_fill, group_label, verb_all_tooltip,
    verb_tooltip,
};

#[cfg(test)]
pub mod row_probe {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static LAST_RENDERED: AtomicUsize = AtomicUsize::new(0);

    pub fn reset() {
        LAST_RENDERED.store(0, Ordering::SeqCst);
    }

    pub(super) fn record(count: usize) {
        LAST_RENDERED.store(count, Ordering::SeqCst);
    }

    pub fn last() -> usize {
        LAST_RENDERED.load(Ordering::SeqCst)
    }
}

/// Shared compact pitch for working-tree and commit-detail file rows.
pub(super) const ROW_H: f32 = 26.;

/// The status letter's column, from `git_badge`.
const BADGE_W: f32 = 10.;

/// The group header's disclosure chevron.
pub const GROUP_CHEVRON: f32 = 8.;
const _: () = assert!(GROUP_CHEVRON <= BADGE_W && BADGE_W - GROUP_CHEVRON <= 2.);

/// One level of the tree view, matching the file tree's indent step.
const TREE_INDENT: f32 = 14.;

/// The key context the message box installs, and the one `ScmCommit` is bound inside.
pub const COMMIT_KEY_CONTEXT: &str = "ScmCommit";

/// Past this many, the branch switcher scrolls instead of growing.
const BRANCHES_IN_MENU: usize = 12;

/// Untracked files past this many start folded.
const UNTRACKED_AUTO_COLLAPSE: usize = 20;

/// The message box rests at 56px and grows with content from there.
const MSG_LINE: f32 = 20.;
const MSG_PAD_X: f32 = 10.;
const MSG_PAD_Y: f32 = 8.;
const MSG_MIN_H: f32 = 56.;
const MSG_MAX_H: f32 = MSG_ROWS_MAX as f32 * MSG_LINE;
const MSG_ROWS: usize = 1;
const MSG_ROWS_MAX: usize = 6;

const COMMIT_H: f32 = ROW_H;
const COMMIT_CHEVRON_W: f32 = COMMIT_H - 2.;
const COMMIT_GLYPH: f32 = 11.;
const COMMIT_RADIUS: gpui::Pixels = px(6.);

const PINNED_TOP: f32 = if cfg!(target_os = "macos") { 0. } else { 8. };
const PINNED_GAP: f32 = 10.;
const PINNED_BOTTOM: f32 = 14.;
const PINNED_INSET: f32 = 14.;

const BRANCH_TILE: f32 = 26.;
const BRANCH_TILE_GLYPH: f32 = 12.;

const GROUP_GAP: f32 = 16.;
const GROUP_HEADER_H: f32 = 22.;
const GROUP_HEADING: f32 = 11.5 / 16.;
const NAME_FLOOR: f32 = 40.;

const PROBE_RETRY: Duration = Duration::from_secs(2);
const NOT_A_REPO_RETRY: Duration = Duration::from_secs(10);

/// What the panel knows about the directory the active pane is sitting in.
pub enum RepoLookup {
    Pending,
    NotARepo,
    Root(PathBuf),
}

impl gpui::Render for ScmPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scm_watch_status(cx);
        if self.override_tab != Some(self.ctx.active_tab) {
            self.repo_override = None;
            self.override_tab = None;
        }

        let Some((host, cwd)) = self.ctx.pane_target.clone() else {
            let body = panel_empty_view(
                t(L10nKey::PanelNoWorkingDirectory),
                Some(t(L10nKey::PanelNoWorkingDirectoryHint)),
                cx,
            );
            return self.scm_shell_static(Vec::new(), body, None);
        };

        let root = match self.scm_repo_root(&host, &cwd, cx) {
            RepoLookup::Pending => {
                let body = panel_empty_view(t(L10nKey::PanelLoading), None, cx);
                return self.scm_shell_static(Vec::new(), body, None);
            }
            RepoLookup::NotARepo => {
                let body = panel_empty_view(
                    t(L10nKey::PanelNotAGitRepo),
                    Some(t(L10nKey::PanelNotAGitRepoHint)),
                    cx,
                );
                return self.scm_shell_static(Vec::new(), body, None);
            }
            RepoLookup::Root(root) => root,
        };

        self.repo = Some(RepoKey {
            host: host.id(),
            root,
        });
        if self
            .repo_override
            .as_ref()
            .is_some_and(|k| anterm_ui_kit::host_registry::HostRegistry::get(cx, k.host).is_none())
        {
            self.repo_override = None;
            self.override_tab = None;
        }
        let repo = self
            .active_repo()
            .cloned()
            .expect("the pane's repository was just recorded");
        let host = match anterm_ui_kit::host_registry::HostRegistry::get(cx, repo.host) {
            Some(host) => host,
            None => host,
        };
        self.scm_probe(&host, &repo.root, cx);
        let Some(status) = self.scm_seen_status(repo.host, &repo.root, cx) else {
            let body = panel_empty_view(t(L10nKey::PanelLoading), None, cx);
            return self.scm_shell_static(Vec::new(), body, None);
        };

        let branch = self.scm_branch_row(&repo, &status, cx);
        let naming = self.scm_new_branch_row(&repo, cx);
        let switching = self.scm_checkout_branch_row(&repo, cx);
        let commit = self.scm_commit_box(&repo, &status, window, cx);
        let buttons = self.scm_commit_buttons(&repo, &status, cx);
        let mut pinned = vec![branch];
        pinned.extend(naming);
        pinned.extend(switching);
        pinned.push(commit);
        pinned.push(buttons);

        let detail = self.detail.clone();
        let detail_body = detail
            .as_ref()
            .and_then(|d| self.render_commit_detail(d, window, cx));
        let history = self.render_graph_section(&repo, window, cx);

        if let Some(detail_el) = detail_body {
            let scroller = div()
                .id("panel-scm-detail-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.detail_scroll)
                .child(detail_el);
            return v_flex()
                .flex_1()
                .min_h_0()
                .children(pinned)
                .child(anterm_ui_kit::scrollbar::with_vertical_scrollbar(
                    "panel-scm-detail-scrollbar",
                    scroller,
                    &self.detail_scroll,
                ))
                .children(history)
                .into_any_element();
        }

        if status.is_clean() {
            let body = panel_empty_view(
                t(L10nKey::PanelNoChanges),
                Some(t(L10nKey::PanelNoChangesHint)),
                cx,
            );
            return self.scm_shell_static(pinned, body, history);
        }

        pinned.push(
            div()
                .pb(px(PINNED_GAP))
                .child(self.scm_filter_row(window, cx))
                .into_any_element(),
        );
        let groups_list = self.scm_groups(&repo, &status, window, cx);
        v_flex()
            .flex_1()
            .min_h_0()
            .children(pinned)
            .child(groups_list)
            .children(history)
            .into_any_element()
    }
}

impl ScmPanelView {
    fn scm_shell_static(
        &self,
        pinned: Vec<AnyElement>,
        body: AnyElement,
        footer: Option<AnyElement>,
    ) -> AnyElement {
        v_flex()
            .flex_1()
            .min_h_0()
            .children(pinned)
            .child(div().flex_1().min_h_0().child(body))
            .children(footer)
            .into_any_element()
    }

    fn scm_branch_row(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.scm_load_branches(repo, cx);
        let mono = cx.theme().mono_font_family.clone();
        let theme = cx.theme();
        let (warning, muted, fg) = (theme.warning, theme.muted_foreground, theme.foreground);
        let detached = matches!(status.head, HeadState::Detached { .. });
        let unpushable = pushable_branch(&status.head).err();
        let busy = self.scm_network_busy(repo, cx);
        let others = self.scm_other_repos(repo);

        let mut notes: Vec<AnyElement> = Vec::new();
        if let Some(count) = others {
            notes.push(branch_note(&format!("+{count}"), muted, &mono));
        }
        if detached {
            notes.push(info_chip(
                t(L10nKey::ScmDetached),
                warning.opacity(0.16),
                warning,
                &mono,
            ));
        }
        if let Some(op) = status.operation {
            notes.push(info_chip(
                t(operation_label(op)),
                warning.opacity(0.16),
                warning,
                &mono,
            ));
        }
        if self.amend {
            notes.push(info_chip(
                t(L10nKey::ScmAmendBadge),
                warning.opacity(0.16),
                warning,
                &mono,
            ));
        }
        if let Some(text) = tracking_chip(
            status.upstream.as_deref(),
            status.ahead_behind,
            unpushable.is_none(),
        )
        .filter(|_| status.upstream.is_some())
        {
            notes.push(branch_note(&text, muted, &mono));
        }

        h_flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .min_h(rems(28. / 16.))
            .mt(px(PINNED_TOP))
            .pl(px(PINNED_INSET + ROW_INSET))
            .pr(px(PINNED_INSET))
            .child(
                Icon::empty()
                    .path("icons/git-branch.svg")
                    .xsmall()
                    .text_color(muted),
            )
            .child(
                div().flex_1().min_w(rems(3.)).ml(px(-6.)).child(
                    Button::new("scm-branch")
                        .ghost()
                        .small()
                        .dropdown_caret(true)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .line_height(relative(1.))
                                .truncate()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(head_label(&status.head)),
                        )
                        .w_full()
                        .h(rems(24. / 16.))
                        .pl(px(4.))
                        .rounded(px(5.))
                        .text_color(fg)
                        .when(detached, |s| s.font_family(mono.clone()))
                        .dropdown_menu_with_anchor(
                            gpui::Anchor::TopLeft,
                            self.scm_branch_menu(repo, status, cx),
                        ),
                ),
            )
            .when(!notes.is_empty(), |row| {
                row.child(
                    h_flex()
                        .gap(px(6.))
                        .min_w(px(0.))
                        .overflow_hidden()
                        .children(notes),
                )
            })
            .child(
                anterm_ui_kit::window_chrome::chrome_tile_sized(
                    Button::new("scm-sync").icon(if busy {
                        Icon::new(IconName::LoaderCircle)
                    } else {
                        Icon::empty().path("icons/git-sync.svg")
                    }),
                    BRANCH_TILE,
                    BRANCH_TILE_GLYPH,
                    false,
                    cx,
                )
                .rounded(px(6.))
                .disabled(busy || unpushable.is_some())
                .tooltip(match unpushable {
                    Some(reason) => t(reason),
                    None if status.upstream.is_some() => t(L10nKey::ScmSync),
                    None => t(L10nKey::ScmPublishBranch),
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.run_action(ScmIntent::Sync, window, cx);
                })),
            )
            .into_any_element()
    }

    fn scm_network_busy(&self, repo: &RepoKey, cx: &gpui::App) -> bool {
        cx.try_global::<anterm_ui_kit::git::data::ScmData>()
            .is_some_and(|data| data.network_slots(repo.host, &repo.root) > 0)
    }

    fn scm_other_repos(&self, current: &RepoKey) -> Option<usize> {
        let choices = self.scm_repo_choices();
        let count = choices.len().saturating_sub(1);
        (count > 0 && choices.contains(current)).then_some(count)
    }

    fn scm_load_branches(&mut self, repo: &RepoKey, cx: &mut Context<Self>) {
        let epoch = cx
            .try_global::<anterm_ui_kit::git::data::ScmData>()
            .map_or(0, |d| d.epoch(repo.host, &repo.root));
        if self
            .branches
            .get(repo)
            .is_some_and(|(at, _)| *at == epoch)
            || self.branches_loading.contains(repo)
        {
            return;
        }
        let Some(host) = anterm_ui_kit::host_registry::HostRegistry::get(cx, repo.host) else {
            return;
        };
        self.branches_loading.insert(repo.clone());
        let root = repo.root.clone();
        let key = repo.clone();
        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                anterm_core::git::git(
                    h,
                    &root,
                    &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
                )
            },
            move |this, out, cx| {
                this.branches_loading.remove(&key);
                let names = out
                    .unwrap_or_default()
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
                this.branches.insert(key, (epoch, names));
                cx.notify();
            },
        );
    }

    fn scm_branch_menu(
        &self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static + use<>
    {
        let view = cx.entity().downgrade();
        let repo = repo.clone();
        let current = match &status.head {
            HeadState::Branch { name, .. } | HeadState::Unborn { branch: name } => name.clone(),
            HeadState::Detached { .. } => String::new(),
        };
        let branches = self
            .branches
            .get(&repo)
            .map(|(_, names)| names.clone())
            .unwrap_or_default();
        let others = self.scm_repo_choices();
        let unpushable = pushable_branch(&status.head).is_err();

        move |menu, _window, _cx| {
            let mut menu = menu.min_w(px(200.));
            if branches.len() > BRANCHES_IN_MENU {
                menu = menu.scrollable(true).max_h(px(300.));
            }
            for name in &branches {
                let is_current = *name == current;
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(is_current)
                        .disabled(is_current)
                        .on_click({
                            let view = view.clone();
                            let repo = repo.clone();
                            let name = name.clone();
                            move |_, window, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.scm_op(
                                        repo.clone(),
                                        GitOp::CheckoutBranch { name: name.clone() },
                                        window,
                                        cx,
                                    )
                                });
                            }
                        }),
                );
            }
            menu = menu
                .separator()
                .item(PopupMenuItem::new(t(L10nKey::ScmCreateBranch)).on_click({
                    let view = view.clone();
                    move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.begin_create_branch(window, cx));
                    }
                }));
            for (label, intent) in [
                (L10nKey::ScmFetch, ScmIntent::Fetch),
                (L10nKey::ScmPull, ScmIntent::Pull),
                (L10nKey::ScmPush, ScmIntent::Push),
            ] {
                menu = menu.item(
                    PopupMenuItem::new(t(label))
                        .disabled(unpushable && matches!(intent, ScmIntent::Push))
                        .on_click({
                            let view = view.clone();
                            move |_, window, cx| {
                                let _ = view
                                    .update(cx, |this, cx| this.run_action(intent, window, cx));
                            }
                        }),
                );
            }
            if others.len() > 1 {
                menu = menu
                    .separator()
                    .item(PopupMenuItem::label(t(L10nKey::ScmSwitchRepository)));
                for other in &others {
                    let label = other
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| other.root.display().to_string());
                    menu = menu.item(PopupMenuItem::new(label).checked(*other == repo).on_click({
                        let view = view.clone();
                        let other = other.clone();
                        move |_, _window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.repo_override = Some(other.clone());
                                this.override_tab = Some(this.ctx.active_tab);
                                cx.notify();
                            });
                        }
                    }));
                }
            }
            menu
        }
    }

    fn scm_repo_choices(&self) -> Vec<RepoKey> {
        let mut out: Vec<RepoKey> = Vec::new();
        for ((host, _cwd), (_, root)) in &self.roots {
            let Some(root) = root else { continue };
            let key = RepoKey {
                host: *host,
                root: root.clone(),
            };
            if !out.contains(&key) {
                out.push(key);
            }
        }
        out.sort_by(|a, b| a.root.cmp(&b.root));
        out
    }

    pub fn begin_create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::ScmCreateBranch)));
        let handle = input.read(cx).focus_handle(cx);
        self.checkout_branch = None;
        self.new_branch = Some(input);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub fn begin_checkout_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t(L10nKey::ScmCheckoutBranch)));
        let handle = input.read(cx).focus_handle(cx);
        self.new_branch = None;
        self.checkout_branch = Some(input);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn scm_new_branch_row(&mut self, repo: &RepoKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.new_branch.clone()?;
        let repo = repo.clone();
        Some(
            h_flex()
                .id("scm-new-branch")
                .flex_none()
                .items_center()
                .h(px(SEARCH_H))
                .px(px(PINNED_INSET))
                .child(div().flex_1().min_w_0().child(Input::new(&input).xsmall()))
                .on_key_down(
                    cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "escape" => {
                                this.new_branch = None;
                                cx.notify();
                            }
                            "enter" => {
                                let Some(input) = this.new_branch.take() else {
                                    return;
                                };
                                let name = input.read(cx).value().trim().to_string();
                                cx.notify();
                                if name.is_empty() {
                                    return;
                                }
                                this.scm_op(
                                    repo.clone(),
                                    GitOp::CreateBranch {
                                        name,
                                        start: None,
                                        checkout: true,
                                    },
                                    window,
                                    cx,
                                );
                            }
                            _ => {}
                        }
                    }),
                )
                .into_any_element(),
        )
    }

    fn scm_checkout_branch_row(
        &mut self,
        repo: &RepoKey,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let input = self.checkout_branch.clone()?;
        let repo = repo.clone();
        Some(
            h_flex()
                .id("scm-checkout-branch")
                .flex_none()
                .items_center()
                .h(px(SEARCH_H))
                .px(px(PINNED_INSET))
                .child(div().flex_1().min_w_0().child(Input::new(&input).xsmall()))
                .on_key_down(
                    cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                        match ev.keystroke.key.as_str() {
                            "escape" => {
                                this.checkout_branch = None;
                                cx.notify();
                            }
                            "enter" => {
                                let Some(input) = this.checkout_branch.take() else {
                                    return;
                                };
                                let name = input.read(cx).value().trim().to_string();
                                cx.notify();
                                if name.is_empty() {
                                    return;
                                }
                                this.scm_op(
                                    repo.clone(),
                                    GitOp::CheckoutBranch { name },
                                    window,
                                    cx,
                                );
                            }
                            _ => {}
                        }
                    }),
                )
                .into_any_element(),
        )
    }

    fn scm_commit_box(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = self.scm_commit_input(repo, status, window, cx);
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        div()
            .key_context(COMMIT_KEY_CONTEXT)
            .flex_none()
            .px(px(PINNED_INSET))
            .pt(px(PINNED_GAP))
            .child(
                div()
                    .min_h(px(MSG_MIN_H))
                    .max_h(px(MSG_MAX_H))
                    .rounded(ROW_RADIUS)
                    .bg(gpui::rgb(match focused {
                        true => sf.hover,
                        false => field_fill(sf),
                    }))
                    .px(px(MSG_PAD_X))
                    .py(px(MSG_PAD_Y))
                    .child(Textarea::new(&input).appearance(false).xsmall()),
            )
            .into_any_element()
    }

    pub fn scm_commit_input(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TextareaState> {
        let input = match self.commit_input.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(MSG_ROWS, MSG_ROWS_MAX)
                        .placeholder(t(L10nKey::ScmCommitPlaceholder))
                });
                self.commit_input = Some(input.clone());
                input
            }
        };
        let text = input.read(cx).value().to_string();

        if self.commit_repo.as_ref() != Some(repo) {
            if let Some(previous) = self.commit_repo.take() {
                self.drafts.insert(previous, text);
            }
            let next = match self.drafts.get(repo) {
                Some(draft) => draft.clone(),
                None => status.prefilled_message.clone().unwrap_or_default(),
            };
            input.update(cx, |state, cx| state.set_value(next, window, cx));
            self.commit_repo = Some(repo.clone());
            return input;
        }

        if self.scm_commit_landed(repo, status, &text) {
            input.update(cx, |state, cx| state.set_value("", window, cx));
            return input;
        }
        if self.drafts.get(repo).map(String::as_str) != Some(text.as_str()) {
            self.drafts.insert(repo.clone(), text);
        }
        input
    }

    pub fn scm_commit_landed(
        &mut self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        text: &str,
    ) -> bool {
        let Some((sent_repo, before, message)) = &self.committing else {
            return false;
        };
        if sent_repo != repo || *before == status.head || message != text {
            return false;
        }
        self.committing = None;
        self.drafts.remove(repo);
        true
    }

    fn scm_commit_buttons(
        &self,
        repo: &RepoKey,
        status: &WorkingTreeStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let plan = commit_plan(status, self.amend, self.draft(repo));
        let repo_for_button = repo.clone();
        let live = plan.enabled;
        let staged = status.staged().count();
        let muted = cx.theme().muted_foreground;
        let paint = commit_paint(live, cx);
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(PINNED_INSET))
            .pt(px(PINNED_GAP))
            .pb(px(PINNED_BOTTOM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(t_plural(L10nKey::ScmStagedFileCount, staged, &[])),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .h(px(COMMIT_H))
                    .rounded(COMMIT_RADIUS)
                    .bg(paint.fill)
                    .hover(move |s| s.bg(paint.hover))
                    .child(
                        Button::new("scm-commit")
                            .custom(commit_half(paint.ink, cx))
                            .xsmall()
                            .label(t(plan.label))
                            .h_full()
                            .px(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .rounded_corners(segment_corners(0, 2, COMMIT_RADIUS, HAIRLINE))
                            .text_color(if live { paint.ink } else { muted })
                            .disabled(!live)
                            .when(!live, |b| b.tooltip(t(plan.reason)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.scm_commit(
                                    repo_for_button.clone(),
                                    this.amend,
                                    None,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(px(0.5))
                            .h(px(COMMIT_H - 12.))
                            .bg(paint.seam),
                    )
                    .child(self.scm_commit_menu(repo, paint.ink, cx)),
            )
            .into_any_element()
    }

    fn scm_commit_menu(
        &self,
        repo: &RepoKey,
        ink: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let amend = self.amend;
        Button::new("scm-commit-menu")
            .custom(commit_half(ink, cx))
            .icon(Icon::new(IconName::ChevronDown))
            .with_size(px(COMMIT_GLYPH / anterm_ui_kit::window_chrome::BUTTON_ICON_SCALE))
            .w(px(COMMIT_CHEVRON_W))
            .h_full()
            .rounded_corners(segment_corners(1, 2, COMMIT_RADIUS, HAIRLINE))
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, {
                let view = cx.entity().downgrade();
                let repo = repo.clone();
                move |menu, _window, _cx| {
                    let mut menu = menu.min_w(px(190.));
                    for (label, intent) in [
                        (L10nKey::ScmCommitButton, ScmIntent::Commit),
                        (L10nKey::ScmCommitAndPush, ScmIntent::CommitAndPush),
                        (L10nKey::ScmCommitAndSync, ScmIntent::CommitAndSync),
                    ] {
                        menu = menu.item(PopupMenuItem::new(t(label)).on_click({
                            let view = view.clone();
                            move |_, window, cx| {
                                let _ = view
                                    .update(cx, |this, cx| this.run_action(intent, window, cx));
                            }
                        }));
                    }
                    menu = menu.separator().item(
                        PopupMenuItem::new(t(L10nKey::ScmAmendLastCommit))
                            .checked(amend)
                            .on_click({
                                let view = view.clone();
                                move |_, _window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.amend = !this.amend;
                                        cx.notify();
                                    });
                                }
                            }),
                    );
                    menu.separator()
                        .item(PopupMenuItem::new(t(L10nKey::ScmStashAll)).on_click({
                            let view = view.clone();
                            let repo = repo.clone();
                            move |_, window, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.scm_stash_all(repo.clone(), window, cx)
                                });
                            }
                        }))
                }
            })
            .into_any_element()
    }

    pub fn scm_repo_root(
        &mut self,
        host: &SharedHost,
        cwd: &Path,
        cx: &mut Context<Self>,
    ) -> RepoLookup {
        let id = host.id();
        let key = (id, cwd.to_path_buf());
        match self.roots.get(&key) {
            Some((_, Some(root))) => {
                if cx
                    .try_global::<anterm_ui_kit::git::data::ScmData>()
                    .is_some_and(|d| d.is_known_not_a_repo(id, root))
                {
                    let root = root.clone();
                    self.forget_root(id, &root);
                } else {
                    return RepoLookup::Root(root.clone());
                }
            }
            Some((at, None)) if at.elapsed() < NOT_A_REPO_RETRY => return RepoLookup::NotARepo,
            _ => {}
        }
        if !self.root_lookups.insert(key.clone()) {
            return RepoLookup::Pending;
        }
        let dir = cwd.to_path_buf();
        anterm_ui_kit::host_ops::HostOps::run(
            host.clone(),
            cx,
            move |h| {
                anterm_core::git::git(
                    h,
                    &dir,
                    &["rev-parse", "--path-format=absolute", "--show-toplevel"],
                )
            },
            move |this, out, cx| {
                this.root_lookups.remove(&key);
                let root = out
                    .as_deref()
                    .and_then(|s| s.lines().next())
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| anterm_core::path_spelling::spelling_on_buf(id, l));
                this.roots.insert(key, (Instant::now(), root));
                cx.notify();
            },
        );
        RepoLookup::Pending
    }

    fn scm_probe(&mut self, host: &SharedHost, root: &Path, cx: &mut Context<Self>) {
        let key = (host.id(), root.to_path_buf());
        if status_of(cx, host.id(), root).is_none() {
            let now = Instant::now();
            match self.probe_attempt.get(&key) {
                Some(at) if now.duration_since(*at) < PROBE_RETRY => return,
                _ => {
                    self.probe_attempt.insert(key, now);
                }
            }
        }
        anterm_ui_kit::git::data::scm_refresh(host.clone(), root.to_path_buf(), cx);
    }

    fn scm_seen_status(
        &mut self,
        host: HostId,
        root: &Path,
        cx: &mut Context<Self>,
    ) -> Option<Arc<WorkingTreeStatus>> {
        let status = status_of(cx, host, root);
        self.seen = Some((
            (host, root.to_path_buf()),
            status.as_ref().map_or(0, |s| Arc::as_ptr(s) as usize),
        ));
        status
    }

    fn scm_watch_status(&mut self, cx: &mut Context<Self>) {
        if self.watch.is_some() {
            return;
        }
        self.watch = Some(cx.observe_global::<anterm_ui_kit::git::data::ScmData>(
            |this, cx| {
                let Some((key, seen)) = this.seen.clone() else {
                    return;
                };
                let now = status_of(cx, key.0, &key.1).map_or(0, |s| Arc::as_ptr(&s) as usize);
                if now != seen {
                    this.seen = Some((key, now));
                    cx.notify();
                }
            },
        ));
    }

    /// Build the flattened virtualized list of change groups, headers, dirs,
    /// files, and footer notes, rendered via `v_virtual_list`.
    fn scm_groups(
        &mut self,
        repo: &RepoKey,
        status: &Arc<WorkingTreeStatus>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.scm_filter_query(cx);
        let tree = cx.global::<anterm_ui_kit::config::Config>().scm_changes_tree;
        let rem = window.rem_size();
        let header_h = rems(GROUP_HEADER_H / 16.).to_pixels(rem) + px(1.);
        let row_h = rems(ROW_H / 16.).to_pixels(rem) + px(1.);
        let spacer_h = px(GROUP_GAP - 1.);
        let note_h = rems(ROW_H / 16.).to_pixels(rem);

        let mut matched = 0usize;
        let mut items: Vec<ScmListItem> = Vec::new();
        let mut item_sizes: Vec<Size<Pixels>> = Vec::new();

        for group in ScmGroup::ORDER {
            let entries: Vec<&StatusEntry> = status
                .entries
                .iter()
                .filter(|e| in_group(e, group))
                .filter(|e| {
                    query
                        .as_deref()
                        .is_none_or(|q| path_matches(e.path.as_str(), q))
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            matched += entries.len();
            if !items.is_empty() {
                items.push(ScmListItem::Spacer);
                item_sizes.push(size(px(0.), spacer_h));
            }
            let collapsed = self.group_collapsed(group, entries.len());
            items.push(ScmListItem::GroupHeader {
                group,
                count: entries.len(),
                collapsed,
                writable: writable_paths(&entries),
            });
            item_sizes.push(size(px(0.), header_h));

            if !collapsed {
                if tree {
                    let filtering = query.is_some();
                    let folded = &self.folded_dirs;
                    let rows = tree_rows(entries.iter().map(|e| e.path.as_str()), |key| {
                        !filtering && folded.contains(&(group, key.to_string()))
                    });
                    for row in rows {
                        match row {
                            TreeRow::Dir {
                                key,
                                label,
                                depth,
                                files,
                                collapsed,
                            } => {
                                items.push(ScmListItem::DirRow {
                                    group,
                                    key,
                                    label,
                                    depth,
                                    files,
                                    collapsed,
                                    foldable: !filtering,
                                });
                                item_sizes.push(size(px(0.), row_h));
                            }
                            TreeRow::File { index, depth } => {
                                items.push(ScmListItem::FileRow {
                                    group,
                                    entry: entries[index].clone(),
                                    depth: Some(depth),
                                });
                                item_sizes.push(size(px(0.), row_h));
                            }
                        }
                    }
                } else {
                    for entry in &entries {
                        items.push(ScmListItem::FileRow {
                            group,
                            entry: (*entry).clone(),
                            depth: None,
                        });
                        item_sizes.push(size(px(0.), row_h));
                    }
                }
            }
        }
        if query.is_some() && matched == 0 {
            items.push(ScmListItem::Note(
                t(L10nKey::ScmNoMatchingChanges).to_string(),
            ));
            item_sizes.push(size(px(0.), note_h));
        }
        if status.truncated {
            items.push(ScmListItem::Note(t_fmt(
                L10nKey::ScmTooManyChanges,
                &[
                    ("shown", &status.entries.len().to_string()),
                    ("total", &status.total_entries.to_string()),
                ],
            )));
            item_sizes.push(size(px(0.), note_h));
        }

        self.rendered_items = items;
        self.rendered_repo = Some(repo.clone());

        let list = v_virtual_list(
            cx.entity(),
            "panel-scm-groups-list",
            Rc::new(item_sizes),
            |this, range, _window, cx| {
                #[cfg(test)]
                row_probe::record(range.len());
                let Some(repo) = this.rendered_repo.clone() else {
                    return Vec::new();
                };
                range
                    .filter_map(|ix| this.rendered_items.get(ix))
                    .map(|item| this.render_list_item(&repo, item, cx))
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.groups_scroll)
        .flex_1()
        .w_full()
        .pb(px(12.));

        anterm_ui_kit::scrollbar::with_vertical_scrollbar(
            "panel-scm-scrollbar",
            list,
            &self.groups_scroll,
        )
        .into_any_element()
    }

    fn render_list_item(
        &self,
        repo: &RepoKey,
        item: &ScmListItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            ScmListItem::Spacer => div().w_full().h(px(GROUP_GAP - 1.)).into_any_element(),
            ScmListItem::GroupHeader {
                group,
                count,
                collapsed,
                writable,
            } => div()
                .w_full()
                .px(px(CONTENT_INSET))
                .pb(px(1.))
                .child(self.scm_group_header(repo, *group, *count, writable, *collapsed, cx))
                .into_any_element(),
            ScmListItem::DirRow {
                group,
                key,
                label,
                depth,
                files,
                collapsed,
                foldable,
            } => div()
                .w_full()
                .px(px(CONTENT_INSET))
                .pb(px(1.))
                .child(self.scm_dir_row(
                    *group,
                    key.clone(),
                    label.clone(),
                    *depth,
                    *files,
                    *collapsed,
                    *foldable,
                    cx,
                ))
                .into_any_element(),
            ScmListItem::FileRow {
                group,
                entry,
                depth,
            } => div()
                .w_full()
                .px(px(CONTENT_INSET))
                .pb(px(1.))
                .child(self.scm_file_row(repo, *group, entry, *depth, cx))
                .into_any_element(),
            ScmListItem::Note(text) => div()
                .w_full()
                .px(px(CONTENT_INSET))
                .child(self.scm_note(text.clone(), cx))
                .into_any_element(),
        }
    }

    fn scm_filter_row(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let input = match self.filter.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| {
                    InputState::new(window, cx).placeholder(t(L10nKey::ScmFilterChanges))
                });
                self.filter_sub =
                    Some(
                        cx.subscribe_in(&input, window, |_this, _input, ev, _window, cx| {
                            if matches!(ev, gpui_kit::component::input::InputEvent::Change) {
                                cx.notify();
                            }
                        }),
                    );
                self.filter = Some(input.clone());
                input
            }
        };
        let tree = cx.global::<anterm_ui_kit::config::Config>().scm_changes_tree;
        let (glyph, tip) = match tree {
            true => ("icons/list-flat.svg", t(L10nKey::ScmViewAsList)),
            false => ("icons/list-tree.svg", t(L10nKey::ScmViewAsTree)),
        };
        let toggle = anterm_ui_kit::window_chrome::chrome_tile_sized(
            Button::new("scm-changes-view").icon(Icon::empty().path(glyph)),
            anterm_ui_kit::window_chrome::TILE_SIZE_SM,
            anterm_ui_kit::window_chrome::TILE_GLYPH_SM,
            false,
            cx,
        )
        .rounded_md()
        .tooltip(tip)
        .on_click(cx.listener(|this, _, _window, cx| this.toggle_changes_tree(cx)))
        .into_any_element();
        panel_search_box(&input, Some(toggle), cx)
    }

    fn scm_filter_query(&self, cx: &gpui::App) -> Option<String> {
        normalize_query(&self.filter.as_ref()?.read(cx).value())
    }

    #[allow(clippy::too_many_arguments)]
    fn scm_dir_row(
        &self,
        group: ScmGroup,
        key: String,
        label: String,
        depth: usize,
        files: usize,
        collapsed: bool,
        foldable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let mono = cx.theme().mono_font_family.clone();
        let muted = cx.theme().muted_foreground;
        let id = SharedString::from(format!("scm-dir-{group:?}-{key}"));
        let selector = id.clone();
        let toggle = cx.listener({
            let key = key.clone();
            move |this, _, _window, cx| {
                this.toggle_dir(group, key.clone());
                cx.notify();
            }
        });
        h_flex()
            .id(id)
            .role(gpui::Role::TreeItem)
            .aria_label(SharedString::from(label.clone()))
            .aria_expanded(!collapsed)
            .debug_selector(move || selector.to_string())
            .items_center()
            .gap(px(8.))
            .min_h(rems(ROW_H / 16.))
            .w_full()
            .min_w_0()
            .px(px(ROW_INSET))
            .pl(px(ROW_INSET + depth as f32 * TREE_INDENT))
            .rounded(px(6.))
            .when(foldable, |row| {
                row.cursor_pointer()
                    .hover(|s| s.bg(gpui::rgb(sf.hover)))
                    .on_click(toggle)
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(key.clone()).build(window, cx)
            })
            .child(
                div()
                    .flex_none()
                    .w(px(BADGE_W))
                    .flex()
                    .justify_center()
                    .text_color(muted)
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(crate::right_panel::TEXT))
                    .text_color(gpui::rgb(sf.text_resting))
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(rems(META_MONO))
                    .font_family(mono)
                    .text_color(muted)
                    .child(files.to_string()),
            )
            .into_any_element()
    }

    fn scm_note(&self, text: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .px(px(ROW_INSET))
            .py(px(3.))
            .text_size(rems(META))
            .text_color(cx.theme().muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn scm_group_header(
        &self,
        repo: &RepoKey,
        group: ScmGroup,
        count: usize,
        writable: &[RepoPath],
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let id = SharedString::from(format!("scm-group-{group:?}"));
        let actions = self.scm_group_actions(&id, repo, group, writable, sf.hover, cx);
        h_flex()
            .id(id.clone())
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!(
                "{} {count}",
                t(group_label(group))
            )))
            .aria_expanded(!collapsed)
            .group(id)
            .relative()
            .items_center()
            .gap(px(6.))
            .min_h(rems(GROUP_HEADER_H / 16.))
            .px(px(ROW_INSET))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.toggle_group(group, count, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(GROUP_CHEVRON))
                    .flex()
                    .justify_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .size(px(GROUP_CHEVRON)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(GROUP_HEADING))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child(t(group_label(group))),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(rems(GROUP_HEADING))
                    .text_color(cx.theme().muted_foreground)
                    .child(count.to_string()),
            )
            .child(actions)
            .into_any_element()
    }

    fn scm_file_row(
        &self,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        depth: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar;
        let mono = cx.theme().mono_font_family.clone();
        let path = entry.path.as_str().to_string();
        let (name, dir) = split_display_path(&path);
        let (letter, deco) = row_status(entry, group);
        let source = group_diff_source(group);
        let selected = self.diff_focus_for(repo.host, &repo.root, &source) == Some(path.as_str());
        let id = SharedString::from(format!("scm-row-{group:?}-{path}"));
        let actions = self.scm_row_actions(
            &id,
            repo,
            group,
            entry,
            if selected { sf.selected } else { sf.hover },
            cx,
        );

        let selector = id.clone();
        h_flex()
            .id(id.clone())
            .role(gpui::Role::TreeItem)
            .aria_label(SharedString::from(format!(
                "{path}, {}",
                t(group_label(group))
            )))
            .aria_selected(selected)
            .debug_selector(move || selector.to_string())
            .group(id)
            .relative()
            .items_center()
            .gap(px(8.))
            .min_h(rems(ROW_H / 16.))
            .w_full()
            .min_w_0()
            .px(px(ROW_INSET))
            .when_some(depth, |row, depth| {
                row.pl(px(ROW_INSET + depth as f32 * TREE_INDENT))
            })
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .when(selected, |s| s.bg(gpui::rgb(sf.selected)))
            .on_click({
                let repo = repo.clone();
                let path = path.clone();
                cx.listener(move |_this, _, _window, cx| {
                    cx.emit(ScmEvent::OpenDiff {
                        host: repo.host,
                        root: repo.root.clone(),
                        source: source.clone(),
                        focus: Some(path.clone()),
                    });
                })
            })
            .tooltip({
                let full_path = path.clone();
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(full_path.clone()).build(window, cx)
                }
            })
            .context_menu({
                let view = cx.entity().downgrade();
                let repo = repo.clone();
                let entry = entry.clone();
                move |menu, _window, cx| {
                    Self::scm_row_context_menu(menu, &view, &repo, group, &entry, cx)
                }
            })
            .child(git_badge(letter, status_color(deco, cx), &mono))
            .child(
                div()
                    .flex_shrink(1.)
                    .min_w(px(NAME_FLOOR))
                    .truncate()
                    .text_size(rems(crate::right_panel::TEXT))
                    .text_color(if deco == DecoStatus::Conflict {
                        status_color(deco, cx)
                    } else if selected {
                        gpui::rgb(sf.text_selected).into()
                    } else {
                        gpui::rgb(sf.text_resting).into()
                    })
                    .when(selected, |s| s.font_weight(gpui::FontWeight::MEDIUM))
                    .when(deco == DecoStatus::Deleted, |s| s.line_through())
                    .child(name.to_string()),
            )
            .when(depth.is_none(), |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_start()
                        .text_right()
                        .text_size(rems(META))
                        .text_color(cx.theme().muted_foreground)
                        .child(dir.to_string()),
                )
            })
            .child(actions)
            .into_any_element()
    }

    fn scm_row_actions(
        &self,
        row: &SharedString,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        backing: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let writable = entry.path.pathspec().is_some();
        let path = entry.path.as_str();
        let mut actions = action_strip(row, backing);

        for &(verb, ref icon) in row_verbs(group) {
            let id = SharedString::from(format!("scm-{verb:?}-{group:?}-{path}"));
            let repo = repo.clone();
            let entry = entry.clone();
            actions = actions.child(
                self.scm_tile(id, icon.clone(), verb_tooltip(verb), writable, cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.scm_row_verb(verb, &repo, group, &entry, window, cx);
                    })),
            );
        }
        actions.into_any_element()
    }

    fn scm_group_actions(
        &self,
        row: &SharedString,
        repo: &RepoKey,
        group: ScmGroup,
        writable: &[RepoPath],
        backing: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let paths = writable.to_vec();
        let mut actions = action_strip(row, backing);

        for &(verb, ref icon) in group_verbs(group) {
            let id = SharedString::from(format!("scm-all-{verb:?}-{group:?}"));
            let repo = repo.clone();
            let paths = paths.clone();
            actions = actions.child(
                self.scm_tile(
                    id,
                    icon.clone(),
                    verb_all_tooltip(verb),
                    !paths.is_empty(),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    let Some(op) = verb_op(verb, group, paths.clone()) else {
                        return;
                    };
                    this.scm_op(repo.clone(), op, window, cx);
                })),
            );
        }
        actions.into_any_element()
    }

    fn scm_tile(
        &self,
        id: SharedString,
        icon: IconName,
        tooltip: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        anterm_ui_kit::window_chrome::chrome_tile_sized(
            Button::new(id).icon(Icon::new(icon.clone())),
            TILE_SIZE_XS,
            TILE_GLYPH_XS,
            false,
            cx,
        )
        .rounded(px(4.))
        .disabled(!enabled)
        .tooltip(if enabled {
            tooltip
        } else {
            t(L10nKey::ScmUnrepresentablePath)
        })
    }

    fn scm_row_verb(
        &mut self,
        verb: RowVerb,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if verb == RowVerb::OpenConflict {
            cx.emit(ScmEvent::OpenDiff {
                host: repo.host,
                root: repo.root.clone(),
                source: group_diff_source(group),
                focus: Some(entry.path.as_str().to_string()),
            });
            return;
        }
        let Some(op) = verb_op(verb, group, vec![entry.path.clone()]) else {
            return;
        };
        self.scm_op(repo.clone(), op, window, cx);
    }

    fn scm_row_context_menu(
        menu: PopupMenu,
        view: &gpui::WeakEntity<Self>,
        repo: &RepoKey,
        group: ScmGroup,
        entry: &StatusEntry,
        cx: &gpui::App,
    ) -> PopupMenu {
        let danger = cx.theme().danger;
        let rel = entry.path.as_str().to_string();
        let absolute = repo.root.join(&rel);
        let source = group_diff_source(group);
        let staged = group == ScmGroup::Staged;

        let mut menu = menu
            .min_w(px(200.))
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextOpen)).on_click({
                    let view = view.clone();
                    let absolute = absolute.clone();
                    move |_, _window, cx| {
                        let _ = view.update(cx, |_, cx| {
                            cx.emit(ScmEvent::OpenFile(absolute.clone()));
                        });
                    }
                }),
            )
            .item(PopupMenuItem::new(t(L10nKey::ScmOpenChanges)).on_click({
                let view = view.clone();
                let repo = repo.clone();
                let rel = rel.clone();
                move |_, _window, cx| {
                    let _ = view.update(cx, |_, cx| {
                        cx.emit(ScmEvent::OpenDiff {
                            host: repo.host,
                            root: repo.root.clone(),
                            source: source.clone(),
                            focus: Some(rel.clone()),
                        });
                    });
                }
            }))
            .separator()
            .item(
                PopupMenuItem::new(if staged {
                    t(L10nKey::ScmUnstage)
                } else {
                    t(L10nKey::ScmStage)
                })
                .on_click({
                    let view = view.clone();
                    let repo = repo.clone();
                    let paths = vec![entry.path.clone()];
                    move |_, window, cx| {
                        let op = if staged {
                            GitOp::Unstage {
                                paths: paths.clone(),
                            }
                        } else {
                            GitOp::Stage {
                                paths: paths.clone(),
                            }
                        };
                        let _ =
                            view.update(cx, |this, cx| this.scm_op(repo.clone(), op, window, cx));
                    }
                }),
            )
            .separator()
            .item(
                PopupMenuItem::new(t(L10nKey::FileTreeContextCopyPath)).on_click({
                    let text = match repo.host == HostId::LOCAL {
                        true => anterm_ui_kit::path_display::native_separators(&absolute)
                            .display()
                            .to_string(),
                        false => absolute.display().to_string(),
                    };
                    move |_, _window, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    }
                }),
            );

        if repo.host == HostId::LOCAL {
            menu = menu.item(
                PopupMenuItem::new(crate::right_panel::reveal_label()).on_click({
                    let absolute = absolute.clone();
                    move |_, _window, cx| {
                        cx.reveal_path(&anterm_ui_kit::path_display::native_separators(&absolute))
                    }
                }),
            );
        }

        if let Some(op) = verb_op(RowVerb::Discard, group, vec![entry.path.clone()]) {
            menu = menu.separator().item(
                PopupMenuItem::element(move |_window, _cx| {
                    div().text_color(danger).child(t(L10nKey::ScmDiscard))
                })
                .on_click({
                    let view = view.clone();
                    let repo = repo.clone();
                    move |_, window, cx| {
                        let op = op.clone();
                        let _ =
                            view.update(cx, |this, cx| this.scm_op(repo.clone(), op, window, cx));
                    }
                }),
            );
        }
        menu
    }

    pub fn refresh_from_tab_status(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(seen) = cx
            .try_global::<anterm_ui_kit::git::status::GitStatusCache>()
            .and_then(|cache| cache.status_for(repo.host, &repo.root))
        else {
            return;
        };
        if self.last_tab_status.as_ref() == Some(&seen) {
            return;
        }
        self.last_tab_status = Some(seen);
        anterm_ui_kit::git::data::scm_invalidate(repo.host, &repo.root, cx);
        cx.notify();
    }

    pub fn toggle_group(&mut self, group: ScmGroup, count: usize, cx: &mut Context<Self>) {
        let collapsed = self.group_collapsed(group, count);
        self.set_group_collapsed(group, !collapsed);
        cx.notify();
    }
}
