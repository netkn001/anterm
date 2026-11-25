//! One commit, in the panel's own body.
//!
//! Replacing the body rather than opening a third kind of container: the
//! panel already knows how to draw a list of changed files, and the only
//! things a commit adds above it are its message and who wrote it. The file
//! rows are the same rows, minus the buttons — nothing here should be able to
//! drift from what the working tree shows.
//!
//! The patch itself still goes to the full-screen overlay. 260px is not a
//! place to read a diff.
//!
//! This is also where the panel pays back what the graph gave up. A history
//! row has about 26 characters beside its lanes and this repository's subjects
//! run to a median of 64, so the graph shows shape and this shows text: the
//! whole subject, the body, every ref, the parents, how many lines moved, and
//! the files.
//!
//! What it is *not* is a container of its own. It is drawn flush with the
//! panel, on the panel's fill, in the panel's type scale — the same dense,
//! low-chrome language as every other body the right panel shows. A
//! second-level view announces itself by the way back at the top of it and by
//! being the only thing on screen; it does not need a raised card, a larger
//! ramp or filled tokens to say so, and an earlier round that gave it all
//! three read as a foreign design pasted into the app.
//!
//! A round after that tried a larger ramp on its own — 14/12/11.5 — and it was
//! turned down for reading too big in a 260px column and too loud beside the
//! graph. That verdict was reached against a panel where everything else was
//! 12/11/10.5, so what it actually measured was the *step*, not the size: this
//! view a size above its neighbours. The neighbours have since moved. The whole
//! right panel runs on `right_panel.rs`'s 14/13/12/11 and tracks `ui_font_size`
//! with it, and being at 12 is now the thing that makes a tab read as a foreign
//! design. So the sizes here are 14/12/11 — and the rule the old note was
//! defending is unchanged and still the point: **this view is never a step
//! above the rows in `panel.rs`.** It is on the ramp with them.

use std::sync::Arc;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px, rems};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

use anterm_core::git::diff::CommitLabel;
use anterm_core::git::log::{Commit, CommitFile, RefKind};
use anterm_core::git::status::DecoStatus;

use anterm_ui_kit::git::diff::DiffSource;
use crate::right_panel::{
    HEADING, META, META_MONO, ROW_INSET, TEXT, TEXT_INSET, git_badge, info_chip,
};
use crate::scm::path::{relative_time, split_display_path};
use crate::scm::state::{CommitDetailView, RepoKey, ScmEvent, ScmPanelView};
use crate::scm::status::{status_color, status_glyph};
use anterm_ui_kit::window_chrome::CONTENT_INSET;
use anterm_i18n::{L10nKey, t, t_plural};

// A file row, the same height as the working tree's, and inset the same way:
// the two lists sit in one column and have to read as one grid, so a reader who
// opens a commit never feels the pitch change under them.
//
// The inset is `right_panel`'s, because every tab of the panel is the same list
// of rows seen from a different angle. The pitch is `panel.rs`'s, because these
// rows *are* that list pointed at another tree — this file used to restate it
// with a comment asking the next reader to keep the two in step, which is the
// same kind of promise that let the panel's type ramp drift.
use crate::scm::panel::ROW_H;

/// How much of the body is shown before it folds. Four lines is a paragraph;
/// past that it is a changelog, and the file list is what the reader came for.
const BODY_LINES: usize = 4;

/// The body's leading and its top padding, both in rems, because the fold is a
/// height and a height that does not track the text it folds is a clip through
/// the middle of a line the moment `ui_font_size` moves.
///
/// `19/16` is what `phi()` already resolves [`META`] to, so nothing moves by
/// naming it — what it buys is a fold that lands on a line boundary.
const BODY_LINE_H: f32 = 19. / 16.;
const BODY_PAD_T: f32 = 2. / 16.;

/// And how much of the subject, which wraps rather than folding. Three lines
/// of [`TEXT`] in 260px is around 78 characters — longer than every subject in
/// this repository but a handful, and a cap for the ones that are a paragraph.
const SUBJECT_LINES: usize = 3;

/// The radius on every pill in this view — the rows, the way back, the object
/// ids: the 6px the Changes tab's 26px rows use.
const PILL_RADIUS: gpui::Pixels = px(6.);

/// How tall the header's two affordances and the parent links are: a row's
/// height less a couple of pixels either side, so a hovered pill reads as the
/// target inside its row rather than as the whole row lighting up.
const PILL_H: f32 = ROW_H - 4.;

/// The pause before the file list — the one the Changes tab leaves between
/// two groups — and the height of the summary line that heads it, which is a
/// group header there too.
const GROUP_GAP: f32 = 16.;
const GROUP_HEADER_H: f32 = 22.;

/// A file row's name keeps at least this much before the directory beside it
/// has given up all of its width. `panel.rs`'s floor, restated because the
/// two rows are the same row.
const NAME_FLOOR: f32 = 40.;

// Which step of the right panel's ramp does what in this view.
//
// The names come from `right_panel.rs` and the sizes are not this view's to
// choose. This file used to hold three of its own — `SUBJECT_SIZE`,
// `SECONDARY_SIZE`, `TOKEN_SIZE`, at 12/11/10.5 — which was the panel's ramp as
// it stood on the day this file was written and stopped being it a few hours
// later.
//
// `TEXT` is the loudest thing on screen: the subject, and the way back, which
// is the same size worn quietly. `META` is everything that qualifies it — the
// byline, the message body, the parents' label, the file count, the waiting
// notes. One step of difference is all a qualifier needs in a column this
// narrow; the separation is carried by weight and colour, not by the gap.
// `META_MONO` is the token size — sha-like strings, ref chips, the diff counts
// — which is barely a choice made here: `git_badge` and `info_chip` are already
// set in it, and an object id has to read as the same kind of thing in this
// view as it does in the graph and on a file row. `TEXT_MONO` is a path, and
// only a path.
//
// Emphasis in this panel is weight and colour rather than size, and a fill only
// where it carries a meaning of its own — HEAD, and a tag. The subject is a
// step up in weight against the full foreground while everything under it is
// muted, and that separation is all it needs; it is what a card was briefly and
// wrongly asked to do. Nothing in this view is bigger than the working tree's
// rows are — which is now a statement about `panel.rs`'s rows rather than about
// a pair of numbers that happened to match.

impl ScmPanelView {
    /// Show one commit, replacing the working tree in the panel body.
    pub fn open_commit_detail(
        &mut self,
        repo: RepoKey,
        oid: String,
        seed: Option<Commit>,
        cx: &mut Context<Self>,
    ) {
        self.detail = Some(CommitDetailView::new(repo, oid, seed));
        cx.notify();
    }

    pub fn close_commit_detail(&mut self, cx: &mut Context<Self>) {
        if self.detail.take().is_some() {
            cx.notify();
        }
    }

    /// The commit detail body, shown in place of the file groups.
    pub fn render_commit_detail(
        &mut self,
        detail: &CommitDetailView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.active_repo() != Some(&detail.repo) {
            self.detail = None;
            return None;
        }
        self.load_commit_detail(detail, cx);

        let mono = cx.theme().mono_font_family.clone();
        let muted = cx.theme().muted_foreground;
        let mut body = v_flex()
            .pb_3()
            .child(self.detail_header_row(detail, &mono, cx));

        match detail.commit.as_deref() {
            Some(commit) => {
                body = body
                    .child(self.detail_message(detail, commit, cx))
                    .children(self.detail_refs(commit, &mono, cx))
                    .children(self.detail_parents(detail, commit, &mono, cx))
                    .child(self.detail_files(detail, commit, &mono, cx));
            }
            None => {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .min_h(px(ROW_H))
                        .px(px(TEXT_INSET))
                        .text_size(rems(META))
                        .text_color(muted)
                        .child(if detail.loaded {
                            t(L10nKey::ScmCommitNotFound)
                        } else {
                            t(L10nKey::PanelLoading)
                        }),
                );
            }
        }
        Some(body.into_any_element())
    }

    fn load_commit_detail(&mut self, detail: &CommitDetailView, cx: &mut Context<Self>) {
        if detail.loading || detail.loaded {
            return;
        }
        let Some(host) = anterm_ui_kit::host_registry::HostRegistry::get(cx, detail.repo.host)
        else {
            return;
        };
        if let Some(open) = self.detail.as_mut() {
            open.loading = true;
        }
        let root = detail.repo.root.clone();
        let oid = detail.oid.clone();
        let seeded = detail.commit.is_some();
        let key = (detail.repo.clone(), detail.oid.clone());
        anterm_ui_kit::host_ops::HostOps::run(
            host,
            cx,
            move |h| {
                use anterm_core::git::log;
                let commit = (!seeded)
                    .then(|| log::load_commit(h, &root, &oid))
                    .flatten();
                (commit, log::commit_files(h, &root, &oid))
            },
            move |this, (commit, files), cx| {
                let Some(open) = this
                    .detail
                    .as_mut()
                    .filter(|d| (d.repo.clone(), d.oid.clone()) == key)
                else {
                    return;
                };
                open.loading = false;
                open.loaded = true;
                if let Some(commit) = commit {
                    open.commit = Some(Arc::new(commit));
                }
                match files {
                    Some(files) => open.files = Some(Arc::new(files)),
                    None => open.files_failed = true,
                }
                cx.notify();
            },
        );
    }

    /// The way back, and the object id.
    ///
    /// The back affordance belongs in `panel_title`'s trailing slot, where the
    /// diff overlay puts its own. It is here instead because the title is
    /// rendered by the panel and this function only produces the body — see
    /// the note in `render_panel_scm`. Being the first row of the body it
    /// scrolls with the content, which is the one thing lost by the move.
    ///
    /// Both halves of the row are chrome and are drawn as chrome: muted, no
    /// resting fill, the hover doing all the work of saying they are hit
    /// targets. The loudest text in this view has to be the subject — the row
    /// above it is a way out and a string to copy, and neither is what the
    /// reader came to read. The oid is set in the same mono at the same token
    /// size as the parent links below, so the two read as the same kind of
    /// thing.
    ///
    /// Both pills pad themselves by `ROW_INSET` inside a row inset by
    /// `CONTENT_INSET`, so the oid lands on the same `TEXT_INSET` column as
    /// every line of text under them (the chevron's box, 2px wider, reaches
    /// that much further out).
    fn detail_header_row(
        &self,
        detail: &CommitDetailView,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let oid = detail.oid.clone();
        let hover_bg = gpui::rgb(panel_surface(cx).hover);
        let muted = cx.theme().muted_foreground;
        h_flex()
            .items_center()
            .gap_1()
            .h(px(ROW_H))
            .px(px(CONTENT_INSET))
            .child(
                h_flex()
                    .id("scm-detail-back")
                    .items_center()
                    .gap_0p5()
                    .h(px(PILL_H))
                    .pl(px(ROW_INSET - 2.))
                    .pr(px(ROW_INSET))
                    .rounded(PILL_RADIUS)
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .on_click(cx.listener(|this, _, _window, cx| this.close_commit_detail(cx)))
                    // `small()`, which is 14px of glyph beside a 12px label —
                    // an icon needs a little more box than the text it labels
                    // to read as the same size, and it is the width of
                    // `git_badge`'s cell as well.
                    .child(Icon::new(IconName::ChevronLeft).small().text_color(muted))
                    .child(
                        div()
                            .text_size(rems(TEXT))
                            .text_color(muted)
                            .child(t(L10nKey::ScmBackToChanges)),
                    ),
            )
            .child(div().flex_1().min_w_0())
            .child(
                h_flex()
                    .id("scm-detail-sha")
                    .flex_none()
                    .items_center()
                    .h(px(PILL_H))
                    .px(px(ROW_INSET))
                    .rounded(PILL_RADIUS)
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .text_size(rems(META_MONO))
                    .text_color(muted)
                    .font_family(mono.clone())
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(t(L10nKey::ScmCopyCommitSha))
                            .build(window, cx)
                    })
                    .on_click(move |_, _window, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(oid.clone()));
                    })
                    .child(short_oid(&detail.oid).to_string()),
            )
            .into_any_element()
    }

    /// Subject, byline, body.
    fn detail_message(
        &self,
        detail: &CommitDetailView,
        commit: &Commit,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = commit.body.trim();
        let lines = body.lines().count();
        let folded = !detail.body_expanded && lines > BODY_LINES;
        v_flex()
            .px(px(TEXT_INSET))
            .pt_1()
            .pb_1p5()
            .gap_1()
            .child(
                // Wrapping, not truncating: this view exists because the graph
                // row could only show the first 26 characters. It carries the
                // weight and the full foreground while everything under it is
                // muted, and that is the whole of its emphasis — it sits on
                // the same step as the file rows below it. Medium, the weight
                // the rest of the panel gives a title, not semibold.
                div()
                    .text_size(rems(TEXT))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .line_clamp(SUBJECT_LINES)
                    .child(SharedString::from(commit.summary.clone())),
            )
            .child(
                div()
                    .text_size(rems(META))
                    .font_features(crate::diff_overlay::tabular())
                    .text_color(cx.theme().muted_foreground)
                    .child(byline(commit, now_unix())),
            )
            .when(!body.is_empty(), |this| {
                this.child(
                    // The secondary size — the same one the byline above it
                    // and the parents below it are set in. A size of its own
                    // bought nothing and cost the reader a fourth step in a
                    // view eight lines tall.
                    //
                    // The fold is a clipped height, not `line_clamp`. That
                    // helper only limits the *wrap* boundaries inside one
                    // logical line: `shape_text` splits on `\n` first and
                    // shapes every hard line it finds, so a clamp of four over
                    // a thirty-line commit message laid out all thirty and the
                    // fold did visibly nothing. A commit body is the one string
                    // in this panel that is always hard-wrapped, which is
                    // exactly the case the helper does not cover.
                    //
                    // Truncating the string as well keeps the shaper off the
                    // lines nobody can see; the height is what guarantees the
                    // fold, because those four lines can each wrap again in a
                    // column this narrow.
                    div()
                        .pt(rems(BODY_PAD_T))
                        .text_size(rems(META))
                        .line_height(rems(BODY_LINE_H))
                        .text_color(cx.theme().muted_foreground)
                        .when(folded, |d| {
                            d.max_h(rems(BODY_PAD_T + BODY_LINE_H * BODY_LINES as f32))
                                .overflow_hidden()
                        })
                        .debug_selector(|| "scm-detail-body".into())
                        .child(SharedString::from(if folded {
                            body.lines().take(BODY_LINES).collect::<Vec<_>>().join("\n")
                        } else {
                            body.to_string()
                        })),
                )
                // A control, not a link: muted like the rest of the chrome,
                // lifted to the foreground and medium under the pointer. The
                // accent stays with focus rings and the primary actions.
                .when(lines > BODY_LINES, |this| {
                    let fg = cx.theme().foreground;
                    this.child(
                        div()
                            .id("scm-detail-body-fold")
                            .debug_selector(|| "scm-detail-fold".into())
                            .w_full()
                            .py(px(1.))
                            .cursor_pointer()
                            .text_size(rems(META))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(cx.theme().muted_foreground)
                            .hover(move |s| s.text_color(fg))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                if let Some(open) = this.detail.as_mut() {
                                    open.body_expanded = !open.body_expanded;
                                    cx.notify();
                                }
                            }))
                            .child(t(if folded {
                                L10nKey::ScmShowMore
                            } else {
                                L10nKey::ScmShowLess
                            })),
                    )
                })
            })
            .into_any_element()
    }

    /// Every ref pointing here, wrapped over as many lines as it takes.
    ///
    /// The graph row shows one chip and a `+N`; there is no reason to hide any
    /// of them once there is a whole column to put them in.
    ///
    /// Exactly two of them get a fill, and they are the two that mean
    /// something. HEAD is where you are: the faint neutral pill the graph
    /// gives it, under the full foreground — one emphasised token on the row,
    /// and no accent spent on it. A tag is yellow because a tag is yellow
    /// everywhere in git. Everything else — the other local
    /// branches, every remote-tracking ref — is a bare muted span: no fill, and
    /// therefore no padding either, because padding exists to hold text off a
    /// background and there is no background to hold it off. A ref name is
    /// already a word with spaces around it; drawing a box around every one of
    /// them turns a list of names into a wall of blocks, which is what the
    /// panel's language is trying not to be.
    ///
    /// The fill is the foreground at 6%, the same patch the graph's HEAD chip
    /// wears, so one ref looks the same in both places. The foreground over
    /// it, not the muted ink the graph uses: the graph has a filled bead in
    /// the gutter to say "you are here", and this view has only the chip.
    ///
    /// What this must never go back to is the bug that predated all of it: the
    /// fallback arm painted `theme.accent` at *full* opacity under muted text,
    /// which made `origin/main` louder than the branch you were actually on and
    /// left HEAD looking like the footnote.
    fn detail_refs(
        &self,
        commit: &Commit,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if commit.refs.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (warning, fg, muted) = (theme.warning, theme.foreground, theme.muted_foreground);
        let mut row = h_flex()
            .flex_wrap()
            .items_center()
            // Six, not four: most of these are bare words now, and words need a
            // little more air between them than chips whose fills already say
            // where one ends and the next begins.
            .gap_1p5()
            .px(px(TEXT_INSET))
            .pb_1p5();
        for deco in &commit.refs {
            row = row.child(match deco.kind {
                RefKind::Tag => info_chip(&deco.short, warning.opacity(0.16), warning, mono),
                _ if deco.is_head => info_chip(&deco.short, fg.opacity(0.06), fg, mono),
                _ => ref_span(&deco.short, muted, mono),
            });
        }
        Some(row.into_any_element())
    }

    /// The parents, as links. Following one is the only way to walk history
    /// backwards from a commit the graph's window does not reach.
    ///
    /// The resting row ink and nothing else at rest — no accent, the way no
    /// other chrome in the panel spends one. A filled pill would make the
    /// parents a second block competing with the ref chips, and this is a
    /// link, not a state. The hover fill is what says the oid is a target, and
    /// it is the same fill, radius and height the header's two affordances
    /// use. The oids themselves are token-sized mono, matching the sha in the
    /// header so that every object id in this view is one recognisable shape.
    fn detail_parents(
        &self,
        detail: &CommitDetailView,
        commit: &Commit,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if commit.parents.is_empty() {
            return None;
        }
        let sf = panel_surface(cx);
        let hover_bg = gpui::rgb(sf.hover);
        let (muted, link) = (cx.theme().muted_foreground, gpui::rgb(sf.text_resting));
        // The label sits on the text column and each oid's pill pads back out
        // by `ROW_INSET`, so the row starts one inset short of it: the first
        // link's text then lines up with the label's gap, not its pill edge.
        let mut row = h_flex()
            .flex_wrap()
            .items_center()
            .gap_0p5()
            .px(px(TEXT_INSET))
            .pb_1()
            .child(
                div()
                    .mr(px(ROW_INSET - 2.))
                    .text_size(rems(META))
                    .text_color(muted)
                    .child(t(L10nKey::ScmCommitParents)),
            );
        for parent in &commit.parents {
            let repo = detail.repo.clone();
            let oid = parent.clone();
            row = row.child(
                h_flex()
                    .id(SharedString::from(format!("scm-detail-parent-{parent}")))
                    .items_center()
                    .h(px(PILL_H))
                    .px(px(ROW_INSET))
                    .rounded(PILL_RADIUS)
                    .cursor_pointer()
                    .hover(|s| s.bg(hover_bg))
                    .text_size(rems(META_MONO))
                    .font_family(mono.clone())
                    .text_color(link)
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        // No seed: a parent is by definition one step past
                        // whatever the caller had in hand.
                        this.open_commit_detail(repo.clone(), oid.clone(), None, cx);
                    }))
                    .child(short_oid(parent).to_string()),
            );
        }
        Some(row.into_any_element())
    }

    /// The summary line and the rows under it.
    ///
    /// While the read is out there is no summary line at all. The count used to
    /// come from `unwrap_or_default()` and so said "no files changed" for as
    /// long as it took git to answer, which is a claim rather than a wait.
    fn detail_files(
        &self,
        detail: &CommitDetailView,
        commit: &Commit,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(files) = detail.files.clone() else {
            let note = if detail.files_failed {
                L10nKey::ScmDetailFilesFailed
            } else {
                L10nKey::PanelLoading
            };
            return self.detail_note(t(note).to_string(), cx);
        };
        let list = v_flex().child(self.detail_summary(&files, mono, cx));
        // The label rides along on the source so the overlay's header can say
        // which commit it is showing, and it is deliberately not part of that
        // source's identity — the same commit opened from here and from
        // anywhere else has to stay one overlay.
        let source = DiffSource::Commit {
            rev: detail.oid.clone(),
            label: Some(CommitLabel {
                subject: commit.summary.clone(),
                author: commit.author.name.clone(),
                at: commit.author.at.unix,
            }),
        };
        // The rows sit in the working tree's own column: laid out on
        // `CONTENT_INSET` and padding themselves out by `ROW_INSET` onto
        // `TEXT_INSET`, so a hovered row's background is wider than its text.
        let mut rows = v_flex().px(px(CONTENT_INSET));
        for file in files.iter() {
            rows = rows.child(self.detail_file_row(detail, &source, file, mono, cx));
        }
        list.child(rows).into_any_element()
    }

    /// `12 files changed  +340 −118`.
    ///
    /// How much the commit is, in one line: the count in words and the size in
    /// numbers. The counts are the only place in this view where green and red
    /// appear, which is what lets them be read without a legend — and they are
    /// mono so the two columns of digits line up against the counts the diff
    /// overlay shows for the same commit, where the reader is going next.
    ///
    /// `−` is U+2212, not a hyphen, matching every other count and gutter mark
    /// in the diff views: the ASCII one sits too high and too short beside a
    /// `+` of the same size.
    ///
    /// Still not `panel_subtitle`: that helper puts anything in its trailing
    /// slot hard against the right edge, because the slot was built for a
    /// button. The counts are not a control off in the corner — they qualify
    /// the words and have to sit next to them, which is the one thing the
    /// layout round got right and the user asked to keep.
    ///
    /// What it *is* copied on is the Changes tab's group header, which is what
    /// it is: the line that heads a list of files. 16px of air above it and no
    /// rule — the working tree tells its groups apart by the pause before each
    /// one, and a hairline here would be the only one left in the tab — then
    /// 22px of 11.5px medium, muted, sentence case. "3 files changed" is a
    /// sentence about the commit, never "3 FILES CHANGED".
    fn detail_summary(
        &self,
        files: &[CommitFile],
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (added_ink, removed_ink) = (theme.success, theme.danger);
        let counts = diff_totals(files);
        h_flex()
            .items_center()
            .gap_1p5()
            .mt(px(GROUP_GAP))
            .mb(px(1.))
            .min_h(rems(GROUP_HEADER_H / 16.))
            .px(px(TEXT_INSET))
            .child(
                div()
                    .text_size(rems(HEADING))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .font_features(crate::diff_overlay::tabular())
                    .text_color(muted)
                    .child(t_plural(L10nKey::ScmFilesChanged, files.len(), &[])),
            )
            .when_some(counts, |this, (added, removed)| {
                this.child(
                    h_flex()
                        .items_center()
                        .gap(gpui::rems(5. / 16.))
                        .text_size(rems(META_MONO))
                        .font_family(mono.clone())
                        .child(div().text_color(added_ink).child(format!("+{added}")))
                        .child(div().text_color(removed_ink).child(format!("−{removed}"))),
                )
            })
            .into_any_element()
    }

    /// The working tree's file row, minus the hover buttons.
    ///
    /// A copy of `scm_file_row`, which is the wrong way round and known to be:
    /// the two have to stay pixel-identical and nothing here enforces that.
    /// They differ only in what they are built from — a `StatusEntry` against
    /// a [`CommitFile`] — and in the buttons, so the shared version is a
    /// function over `(letter, deco, path)` plus an optional trailing element.
    fn detail_file_row(
        &self,
        detail: &CommitDetailView,
        source: &DiffSource,
        file: &CommitFile,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sf = panel_surface(cx);
        let deco = crate::diff_overlay::deco_status(file.status);
        let (name, dir) = split_display_path(&file.path);
        let selected = self.diff_focus_for(detail.repo.host, &detail.repo.root, source)
            == Some(file.path.as_str());

        h_flex()
            .id(SharedString::from(format!("scm-detail-file-{}", file.path)))
            .items_center()
            .gap_2()
            .min_h(rems(ROW_H / 16.))
            .w_full()
            .min_w_0()
            .px(px(ROW_INSET))
            .rounded(PILL_RADIUS)
            .cursor_pointer()
            .hover(|s| s.bg(gpui::rgb(sf.hover)))
            .when(selected, |s| s.bg(gpui::rgb(sf.selected)))
            .on_click({
                let repo = detail.repo.clone();
                let source = source.clone();
                let path = file.path.clone();
                cx.listener(move |_this, _, _window, cx| {
                    cx.emit(ScmEvent::OpenDiff {
                        host: repo.host,
                        root: repo.root.clone(),
                        source: source.clone(),
                        focus: Some(path.clone()),
                    });
                })
            })
            .child(git_badge(status_glyph(deco), status_color(deco, cx), mono))
            // The name takes its own width first and only then shrinks, down
            // to a floor; the directory beside it gets whatever is left.
            .child(
                div()
                    .flex_shrink(1.)
                    .min_w(px(NAME_FLOOR))
                    .truncate()
                    // Match the working-tree list: UI names, secondary paths.
                    .text_size(rems(TEXT))
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
            // Flush right, giving way from its *start*: the folder nearest the
            // file is the part that says where it is. The same column the
            // working tree's rows keep, in the plain caption ink — no extra
            // transparency on top of a colour already floored for legibility.
            .child(
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
            .into_any_element()
    }

    fn detail_note(&self, text: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .min_h(px(ROW_H))
            .mt(px(GROUP_GAP))
            .px(px(TEXT_INSET))
            .text_size(rems(META))
            .text_color(cx.theme().muted_foreground)
            .child(text)
            .into_any_element()
    }
}

/// The surface every hover and selection in this view is computed against.
///
/// The sidebar's, because the right panel is the sidebar and this view paints
/// nothing under itself. A fill derived from `window` would be a step off a
/// base that is not there, and `list_hover` is the popover's.
fn panel_surface(cx: &gpui::App) -> anterm_ui_kit::presets::Surface {
    cx.global::<anterm_ui_kit::presets::Surfaces>().sidebar
}

/// One unemphasised ref, as bare text.
///
/// The counterpart to `info_chip` for the arm that has no fill: same mono, same
/// size, no padding and no radius, so a row of ordinary refs reads as a row of
/// words rather than a row of empty boxes. `flex_none` because the row wraps
/// and a ref name must break between names, never inside one.
fn ref_span(text: &str, ink: gpui::Hsla, mono: &SharedString) -> AnyElement {
    div()
        .flex_none()
        .text_size(rems(META_MONO))
        .font_family(mono.clone())
        .text_color(ink)
        .child(text.to_string())
        .into_any_element()
}

/// The commit's line delta, summed over the files git was able to count.
///
/// The numbers are already in hand: `commit_files` joins `--numstat` against
/// `--name-status`, so every [`CommitFile`] arrives carrying its own `added`
/// and `removed`. Nothing extra is read to draw this line.
///
/// `None` means there is nothing worth printing, which is two cases wearing
/// one answer. A binary file has no counts at all — `--numstat` prints `-` for
/// both columns and the fields come through as [`None`] — so a commit that
/// only touched binaries sums to zero out of zero. A pure rename does have
/// counts, and they are `0` and `0`. Either way `+0 −0` is a measurement of
/// nothing, and a line of type that answers a question nobody asked; the file
/// rows already say what happened.
///
/// Summed with `saturating_add` rather than `sum()`, which panics on overflow
/// in a debug build. This runs in `render`, and a repository that manages four
/// billion added lines in one commit should get a wrong number, not a crash.
pub fn diff_totals(files: &[CommitFile]) -> Option<(u32, u32)> {
    let fold = |pick: fn(&CommitFile) -> Option<u32>| {
        files
            .iter()
            .filter_map(pick)
            .fold(0u32, |acc, n| acc.saturating_add(n))
    };
    let (added, removed) = (fold(|f| f.added), fold(|f| f.removed));
    (added > 0 || removed > 0).then_some((added, removed))
}

/// `Ada Lovelace · 2h`. Author, not committer: a rebase rewrites the second
/// one, and "who wrote this" is the question a reader is asking.
pub fn byline(commit: &Commit, now: i64) -> String {
    let when = (commit.author.at.unix > 0).then(|| relative_time(now, commit.author.at.unix));
    match (commit.author.name.trim(), when) {
        ("", Some(when)) => when,
        (name, Some(when)) => format!("{name} · {when}"),
        (name, None) => name.to_string(),
    }
}

/// Seven, which is what git itself prints and what the graph's rows use.
pub fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anterm_core::git::diff::FileStatus;
    use anterm_core::git::log::{OffsetTs, Signature};

    fn commit(name: &str, at: i64) -> Commit {
        Commit {
            oid: "3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a".into(),
            parents: Default::default(),
            author: Signature {
                name: name.into(),
                email: "ada@example.com".into(),
                at: OffsetTs {
                    unix: at,
                    offset_minutes: 0,
                },
            },
            committer: Signature {
                name: "Grace".into(),
                email: "grace@example.com".into(),
                at: OffsetTs {
                    unix: at,
                    offset_minutes: 0,
                },
            },
            summary: "s".into(),
            body: String::new(),
            refs: Vec::new(),
        }
    }

    #[test]
    fn the_byline_drops_the_separator_along_with_the_half_it_joined() {
        let now = 1_786_255_391 + 7200;
        assert_eq!(byline(&commit("Ada", 1_786_255_391), now), "Ada · 2h");
        assert_eq!(
            byline(&commit("", 1_786_255_391), now),
            "2h",
            "an unattributed commit is not `· 2h`"
        );
        assert_eq!(
            byline(&commit("Ada", 0), now),
            "Ada",
            "and a date that would not parse is not `Ada · 56y`"
        );
    }

    #[test]
    fn a_short_oid_is_the_seven_characters_git_itself_prints() {
        assert_eq!(
            short_oid("3f2a1b9c8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f3a"),
            "3f2a1b9"
        );
        assert_eq!(short_oid("abc"), "abc", "a truncated oid is not padded");
        assert_eq!(short_oid(""), "");
    }

    fn file(path: &str, status: FileStatus, counts: Option<(u32, u32)>) -> CommitFile {
        CommitFile {
            path: path.into(),
            orig_path: None,
            status,
            added: counts.map(|c| c.0),
            removed: counts.map(|c| c.1),
            binary: counts.is_none(),
        }
    }

    #[test]
    fn the_summary_adds_up_every_file_git_could_count() {
        let files = [
            file("a.rs", FileStatus::Modified, Some((10, 4))),
            file("b.rs", FileStatus::Added, Some((2, 0))),
            file("c.rs", FileStatus::Deleted, Some((0, 8))),
        ];
        assert_eq!(diff_totals(&files), Some((12, 12)));
    }

    /// The two shapes of "there is nothing to print", which have to come back
    /// as the same answer even though git spells them differently: `-` for a
    /// binary, and a real pair of zeroes for a rename.
    #[test]
    fn a_commit_with_nothing_countable_gets_no_counts_rather_than_zeroes() {
        let binary = [file("logo.png", FileStatus::Modified, None)];
        assert_eq!(
            diff_totals(&binary),
            None,
            "`--numstat` printed `-`, so there is no number to show"
        );

        let renamed = [file("new.rs", FileStatus::Renamed, Some((0, 0)))];
        assert_eq!(
            diff_totals(&renamed),
            None,
            "a pure rename is counted, and what it counts to is nothing"
        );

        assert_eq!(diff_totals(&[]), None, "and neither is an empty list");
    }

    /// A binary alongside real edits must not swallow them, and must not be
    /// counted as a zero that drags the total down either — it simply is not
    /// part of the sum.
    #[test]
    fn an_uncountable_file_drops_out_of_a_sum_that_still_has_something_in_it() {
        let mixed = [
            file("logo.png", FileStatus::Added, None),
            file("main.rs", FileStatus::Modified, Some((3, 1))),
        ];
        assert_eq!(diff_totals(&mixed), Some((3, 1)));
    }

    /// `render` calls this, so an absurd repository has to give a wrong number
    /// rather than take the frame down with it.
    #[test]
    fn a_total_past_what_a_u32_holds_saturates_instead_of_panicking() {
        let files = [
            file("a.rs", FileStatus::Modified, Some((u32::MAX, 1))),
            file("b.rs", FileStatus::Modified, Some((7, u32::MAX))),
        ];
        assert_eq!(diff_totals(&files), Some((u32::MAX, u32::MAX)));
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod detail_gpui_tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use gpui::{Entity, TestAppContext, VisualTestContext, size};
    use gpui_kit::component::Root;

    use super::*;
    use crate::scm::state::{ScmContext, ScmPanelView};
    use anterm_ui_kit::host_ops::HostId;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anterm-detail-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn two_commit_repo(name: &str) -> PathBuf {
        let root = scratch(name);
        git(&root, &["init", "--quiet"]);
        git(&root, &["config", "user.email", "ada@example.com"]);
        git(&root, &["config", "user.name", "Ada"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-qm", "root commit"]);
        std::fs::rename(root.join("a.txt"), root.join("renamed.txt")).unwrap();
        std::fs::write(root.join("with space.txt"), "two\n").unwrap();
        std::fs::write(root.join("中文名.txt"), "three\n").unwrap();
        git(&root, &["add", "-A"]);
        git(
            &root,
            &[
                "commit",
                "-qm",
                "feat(detail): a subject long enough that the graph row could never have shown it",
                "-m",
                "one\ntwo\nthree\nfour\nfive\nsix",
            ],
        );
        git(&root, &["branch", "sidequest"]);
        git(&root, &["tag", "v1.0.0"]);
        root
    }

    fn open_standalone(
        cx: &mut TestAppContext,
        root: &Path,
    ) -> (Entity<ScmPanelView>, VisualTestContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            anterm_ui_kit::config::pin_test_config_dir();
            gpui_kit::init(cx);
            cx.set_global(anterm_ui_kit::config::Config::default());
            anterm_ui_kit::theme::apply_theme(None, cx);
            cx.default_global::<anterm_ui_kit::host_registry::HostRegistry>();
        });
        let slot: Rc<RefCell<Option<Entity<ScmPanelView>>>> = Rc::new(RefCell::new(None));
        let host = anterm_core::host::local::LocalHost::new();
        let root_buf = root.to_path_buf();
        let handle = cx.open_window(size(px(300.), px(600.)), {
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|_| {
                    let mut v = ScmPanelView::new(false);
                    v.sync_context(ScmContext {
                        active_tab: 0,
                        pane_target: Some((host.clone(), root_buf.clone())),
                        diff_focus: None,
                        panel_width: 300.,
                    });
                    v.repo = Some(RepoKey {
                        host: HostId::LOCAL,
                        root: root_buf.clone(),
                    });
                    v.roots.insert(
                        (HostId::LOCAL, root_buf.clone()),
                        (std::time::Instant::now(), Some(root_buf.clone())),
                    );
                    v
                });
                *slot.borrow_mut() = Some(view.clone());
                Root::new(view, window, cx)
            }
        });
        let vcx = VisualTestContext::from_window(handle.into(), cx);
        let view = slot.borrow().clone().expect("scm view");
        (view, vcx)
    }

    fn wait_loaded(view: &Entity<ScmPanelView>, vcx: &mut VisualTestContext) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            vcx.update(|window, cx| window.draw(cx).clear(cx));
            vcx.background_executor.run_until_parked();
            if view.read_with(vcx, |v, _| v.detail.as_ref().is_some_and(|d| d.loaded)) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "commit detail never finished loading"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[gpui::test]
    fn commit_detail_loads_seeded_unseeded_and_clears_on_repo_switch(cx: &mut TestAppContext) {
        let root = two_commit_repo("standalone");
        let head = git(&root, &["rev-parse", "HEAD"]);
        let parent = git(&root, &["rev-parse", "HEAD^"]);
        let (view, mut vcx) = open_standalone(cx, &root);
        let repo = RepoKey {
            host: HostId::LOCAL,
            root: root.clone(),
        };

        // Unseeded load resolves metadata + all three filenames.
        view.update(&mut vcx, |v, cx| {
            v.open_commit_detail(repo.clone(), head.clone(), None, cx)
        });
        wait_loaded(&view, &mut vcx);
        view.read_with(&vcx, |v, _| {
            let detail = v.detail.as_ref().unwrap();
            let commit = detail.commit.as_ref().unwrap();
            assert_eq!(commit.oid, head);
            assert!(commit.summary.starts_with("feat(detail):"));
            let mut names: Vec<String> = detail
                .files
                .as_ref()
                .unwrap()
                .iter()
                .map(|f| f.path.clone())
                .collect();
            names.sort();
            assert_eq!(names, ["renamed.txt", "with space.txt", "中文名.txt"]);
        });

        // Seeded load preserves seed metadata while reading file list.
        let mut seed = anterm_core::git::log::load_commit(
            &*anterm_core::host::local::LocalHost::new(),
            &root,
            &parent,
        )
        .unwrap();
        seed.summary = "seeded summary".into();
        view.update(&mut vcx, |v, cx| {
            v.open_commit_detail(repo.clone(), parent.clone(), Some(seed), cx)
        });
        wait_loaded(&view, &mut vcx);
        view.read_with(&vcx, |v, _| {
            let detail = v.detail.as_ref().unwrap();
            assert_eq!(detail.commit.as_ref().unwrap().summary, "seeded summary");
            let names: Vec<&str> = detail
                .files
                .as_ref()
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect();
            assert_eq!(names, ["a.txt"]);
        });

        // A detail belonging to a different repo clears itself on render.
        view.update_in(&mut vcx, |v, window, cx| {
            let stranger = RepoKey {
                host: HostId::LOCAL,
                root: PathBuf::from("/no/such/anterm/repo"),
            };
            v.open_commit_detail(stranger, head.clone(), None, cx);
            let detail = v.detail.clone().unwrap();
            assert!(v.render_commit_detail(&detail, window, cx).is_none());
            assert!(v.detail.is_none());
        });

        let _ = std::fs::remove_dir_all(&root);
    }
}
