//! The slot the code panel and the diff overlay are drawn in.
//!
//! Both surfaces used to be full-workspace overlays — `absolute`, `inset_0`,
//! `occlude` — so opening a file to read it hid the agent that told you to read
//! it, and reviewing one turned into a toggle loop. They now dock as a column
//! beside the terminal instead, on the same pattern the right panel has always
//! used: a flex sibling with a drag handle and a persisted share of the width.
//!
//! A sibling column, rather than a narrower overlay, is the whole point: the
//! terminal element's laid-out bounds are what drive `set_grid_size`, so a
//! column takes width *away* from the grid and the PTY reflows to what is left.
//! An overlay painted over half the workspace would leave the grid full width
//! with half of it under a card.
//!
//! The overlay is not gone — [`DocumentLayout::Fill`] is exactly the old paint,
//! one command away, and a window too narrow to seat both a terminal and a
//! document falls back to it for that frame without touching what the user
//! saved.

use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use gpui_kit::component::{ActiveTheme as _, InteractiveElementExt as _, v_flex};
use std::cell::Cell as StdCell;
use std::rc::Rc;

use anterm_ui_kit::config::{
    Config, DOCUMENT_RATIO_MAX, DOCUMENT_RATIO_MIN, DOCUMENT_RATIO_STOPS, DocumentLayout,
};
use anterm_ui_kit::window_chrome::TERMINAL_MIN_W;
use crate::CodeEditorWorkspace;
use anterm_ui_kit::panel_tokens::RESIZE_HANDLE_WIDTH;

pub use anterm_ui_kit::window_chrome::{
    DOCUMENT_MIN_W, DocumentChrome, HEADER_CELL_H, HEADER_CELL_MAX_W, HEADER_CELL_RADIUS,
    HEADER_CELL_TEXT, OverlayTop, document_column_px, document_layout_switch_control,
    document_terminal_button,
};
#[cfg(test)]
use anterm_ui_kit::window_chrome::layout_switch_cell;

impl CodeEditorWorkspace {
    /// A filled document's lead: the terminal it covers, by name, and a way
    /// back that keeps the file or diff open beside it. Both surfaces use this
    /// button so returning to the terminal never depends on which document is
    /// in front. A docked header already has the terminal beside it.
    pub fn render_document_terminal_button(
        &self,
        chrome: DocumentChrome,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if chrome != DocumentChrome::Fill {
            return None;
        }
        let name = self.active_tab_label().to_string();
        document_terminal_button(
            chrome,
            &name,
            cx.listener(|this, _, _, cx| {
                this.set_document_layout(DocumentLayout::Dock, cx);
            }),
            window,
            cx,
        )
    }

    /// The layout the active tab's document was asked for, which is not always
    /// the one it gets — see [`CodeEditorWorkspace::document_dock_px`].
    ///
    /// A tab that has never been told follows the config, which holds the last
    /// explicit choice anyone made and is therefore what a fresh tab starts
    /// from. Telling one tab never moves another.
    pub fn document_layout(&self, cx: &gpui::App) -> DocumentLayout {
        self.active_tab_doc()
            .and_then(|t| t.document_layout)
            .unwrap_or_else(|| cx.global::<Config>().document_layout)
    }

    /// Which of the two surfaces the docked column shows.
    ///
    /// `overlay_top` orders a *pair* of overlays in fill mode, where both are
    /// painted and the front one wins on paint order. A column has one child,
    /// so the ordering has to become a choice: the surface on top, unless it is
    /// closed, in which case there is no front and the survivor is it.
    pub fn document_front(&self) -> Option<OverlayTop> {
        let tab = self.active_tab_doc()?;
        let code = tab.code.as_ref().is_some_and(|c| c.visible);
        let diff = tab.diff_overlay.is_some();
        match (tab.overlay_top, code, diff) {
            (_, false, false) => None,
            (OverlayTop::Code, true, _) | (OverlayTop::Diff, true, false) => Some(OverlayTop::Code),
            (OverlayTop::Diff, _, true) | (OverlayTop::Code, false, true) => Some(OverlayTop::Diff),
        }
    }

    /// What the document column has reserved, from a side panel's point of
    /// view.
    ///
    /// Read from the user's intent and from whether a surface is open — never
    /// from the *effective* layout, which is derived from the widths this feeds
    /// and would close the loop on itself.
    pub fn document_floor(&self, cx: &gpui::App) -> f32 {
        if self.document_layout(cx) == DocumentLayout::Dock && self.document_front().is_some() {
            DOCUMENT_MIN_W
        } else {
            0.
        }
    }

    /// The width the terminal and a docked document share: the window less
    /// whichever side panels are open, at the widths they are actually drawn
    /// at rather than at their floors. A sidebar someone dragged wider is width
    /// the terminal no longer has.
    pub fn document_body_px(&self, window: &Window, cx: &gpui::App) -> f32 {
        let viewport = window.viewport_size().width.as_f32();
        let sidebar = self.sidebar_px(window, cx);
        let panel = self.right_panel_px(window, cx);
        viewport - sidebar - panel
    }

    /// How wide the document column is drawn this frame, or `None` when the
    /// surface is closed, the user chose fill, or the window is too narrow to
    /// seat both.
    pub fn document_dock_px(&self, window: &Window, cx: &gpui::App) -> Option<f32> {
        if self.document_layout(cx) != DocumentLayout::Dock || self.document_front().is_none() {
            return None;
        }
        document_column_px(
            self.document_body_px(window, cx),
            self.editor.document_ratio.get(),
        )
    }

    /// Fill ↔ dock, for the active tab.
    pub fn toggle_document_fill(&mut self, cx: &mut Context<Self>) {
        let next = match self.document_layout(cx) {
            DocumentLayout::Dock => DocumentLayout::Fill,
            DocumentLayout::Fill => DocumentLayout::Dock,
        };
        self.set_document_layout(next, cx);
    }

    /// Snap the column to a named share of the terminal column. Docks first if
    /// the surface is filling the window: asking for a third of the width is
    /// asking for a column.
    pub fn set_document_ratio(&mut self, ratio: f32, cx: &mut Context<Self>) {
        self.editor.document_ratio.set(ratio);
        self.update_config(cx, |cfg| cfg.document_ratio = ratio);
        self.set_document_layout(DocumentLayout::Dock, cx);
    }

    /// Third → half → two thirds → third, the double-click on the divider.
    pub fn cycle_document_ratio(&mut self, cx: &mut Context<Self>) {
        let current = self.editor.document_ratio.get();
        let nearest = next_ratio_stop(current);
        self.set_document_ratio(nearest, cx);
    }

    /// Point the active tab at a layout, and only that tab.
    pub fn set_document_layout(&mut self, layout: DocumentLayout, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_tab_doc_mut() {
            tab.document_layout = Some(layout);
        }
        cx.notify();
    }

    /// Dock / Fill, as a two-cell icon switch: a raised cell on a sunken track.
    pub fn document_layout_switch(
        &self,
        chrome: DocumentChrome,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = cx.entity().downgrade();
        document_layout_switch_control(
            chrome,
            move |layout, _, cx| {
                let _ = app.update(cx, |this, cx| this.set_document_layout(layout, cx));
            },
            cx,
        )
    }

    pub fn toggle_diff_overlay(
        &mut self,
        host: anterm_ui_kit::host_ops::HostId,
        cwd: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_diff_overlay_at(host, cwd, None, window, cx)
    }

    pub fn toggle_diff_overlay_at(
        &mut self,
        host: anterm_ui_kit::host_ops::HostId,
        cwd: std::path::PathBuf,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_diff_overlay(
            host,
            cwd,
            anterm_ui_kit::git::diff::DiffSource::Head,
            focus,
            window,
            cx,
        )
    }

    pub fn open_diff_overlay(
        &mut self,
        host: anterm_ui_kit::host_ops::HostId,
        cwd: std::path::PathBuf,
        source: anterm_ui_kit::git::diff::DiffSource,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use anterm_files_ui::diff_overlay::{DiffOverlayEvent, DiffOverlayView};
        use gpui::Focusable as _;

        let Some(active_tab) = self.active_tab_id() else {
            return;
        };
        let was_front = self
            .tab_doc(active_tab)
            .is_some_and(|t| t.overlay_top == OverlayTop::Diff)
            || !self.code_panel_visible();
        self.tab_doc_mut(active_tab).overlay_top = OverlayTop::Diff;
        if let Some(existing) = self
            .tab_doc(active_tab)
            .and_then(|t| t.diff_overlay.clone())
            .filter(|o| o.read(cx).matches_target(host, &cwd, &source))
        {
            if existing.read(cx).focus_path() == focus.as_deref() && was_front {
                self.close_diff_overlay(window, cx);
                return;
            }
            existing.update(cx, |o, cx| o.set_focus(focus, window, cx));
            cx.notify();
            return;
        }
        self.remember_active_pane(window, cx);
        let overlay = cx.new(|cx| DiffOverlayView::new(host, cwd, source, focus, cx));
        cx.subscribe_in(
            &overlay,
            window,
            |this, _, ev: &DiffOverlayEvent, window, cx| match ev {
                DiffOverlayEvent::Close => this.close_diff_overlay(window, cx),
                DiffOverlayEvent::OpenFile { path } => this.open_file_in_editor(path, window, cx),
                DiffOverlayEvent::SetDocumentLayout(layout) => {
                    this.set_document_layout(*layout, cx)
                }
                DiffOverlayEvent::SetDiffViewMode(mode) => {
                    let mode = *mode;
                    this.update_config(cx, |cfg| cfg.diff_view = mode);
                }
            },
        )
        .detach();
        cx.observe(&overlay, |_, _, cx| cx.notify()).detach();
        let handle = overlay.focus_handle(cx);
        self.tab_doc_mut(active_tab).diff_overlay = Some(overlay);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub fn close_diff_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let taken = self
            .active_tab_doc_mut()
            .and_then(|t| t.diff_overlay.take());
        if taken.is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub fn maybe_refresh_diff_overlay(&mut self, cx: &mut Context<Self>) {
        if let Some(overlay) = self.active_diff_overlay() {
            overlay.update(cx, |o, cx| o.maybe_refresh(cx));
        }
    }

    fn diff_header_context(
        &self,
        chrome: DocumentChrome,
        _window: &Window,
        cx: &gpui::App,
    ) -> anterm_files_ui::diff_overlay::DiffHeaderContext {
        let terminal_label = if chrome == DocumentChrome::Fill {
            self.active_tab_label().to_string()
        } else {
            String::new()
        };
        anterm_files_ui::diff_overlay::DiffHeaderContext {
            chrome,
            left_panel_open: self.left_panel_open(cx),
            terminal_label,
        }
    }

    pub fn render_diff_overlay(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.active_diff_overlay()?;
        let header_ctx = self.diff_header_context(chrome, window, cx);
        overlay.update(cx, |o, _| o.set_header_context(header_ctx));
        Some(overlay.into_any_element())
    }

    pub fn render_diff_header_only(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.active_diff_overlay()?;
        let header_ctx = self.diff_header_context(chrome, window, cx);
        Some(overlay.update(cx, |o, cx| o.render_header(&header_ctx, window, cx)))
    }

    /// The column itself: the surface `overlay_top` selects, sized, bordered,
    /// and given the divider on its left edge.
    /// The header on its own, for the strip above the column — see
    /// [`DocumentChrome::DockHoisted`]. Returns `None` for the layouts that
    /// keep their header inside the surface.
    pub fn render_document_header(
        &mut self,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if chrome.renders_own_header() {
            return None;
        }
        match self.document_front()? {
            OverlayTop::Code => Some(self.render_editor_header_only(chrome, window, cx)),
            OverlayTop::Diff => self.render_diff_header_only(chrome, window, cx),
        }
    }

    pub fn render_document_column(
        &mut self,
        width: f32,
        chrome: DocumentChrome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let body = self.document_body_px(window, cx);
        let surface = match self.document_front()? {
            OverlayTop::Code => self.render_code_overlay(chrome, window, cx),
            OverlayTop::Diff => self.render_diff_overlay(chrome, window, cx),
        }?;
        let (backing, handle) = self.document_resize(body, cx);
        Some(
            v_flex()
                .id("document-column")
                .relative()
                .flex_none()
                .w(px(width))
                .h_full()
                .bg(anterm_ui_kit::theme::workspace_surface_color(cx))
                .border_l(px(DOCUMENT_RULE))
                .border_color(cx.theme().sidebar_border)
                .child(backing)
                // The clip belongs to the content, not to the column: the
                // divider hangs half a handle past the left edge, the way the
                // panels' do, and clipping the column would have taken that
                // half — and the grab with it — away.
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w(px(document_content_px(width)))
                        .overflow_hidden()
                        .child(surface),
                )
                .child(handle)
                .into_any_element(),
        )
    }

    /// The divider, on the same contract as the right panel's: the cell moves
    /// on every mouse move, the config is written once, on mouse up.
    ///
    /// `body` is read here, while there is still a `cx` to read it from — the
    /// drag handler only ever sees a `Window`, and a limit that disagreed with
    /// the one the layout applies would spring the column back from wherever it
    /// was dropped.
    fn document_resize(&self, body: f32, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        use gpui::{Bounds, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, canvas};

        let container: Rc<StdCell<Option<Bounds<Pixels>>>> = Rc::new(StdCell::new(None));
        let backing = canvas(
            {
                let container = container.clone();
                move |bounds, _window, _cx| container.set(Some(bounds))
            },
            {
                let container = container.clone();
                let ratio_cell = self.editor.document_ratio.clone();
                let dragging = self.editor.document_dragging.clone();
                move |_bounds, _state, window, _cx| {
                    window.on_mouse_event({
                        let container = container.clone();
                        let ratio_cell = ratio_cell.clone();
                        let dragging = dragging.clone();
                        move |ev: &MouseMoveEvent, _phase, window, _cx| {
                            if !dragging.get() || body <= 0. {
                                return;
                            }
                            let Some(b) = container.get() else {
                                return;
                            };
                            let right = b.origin.x + b.size.width;
                            let raw = (right - ev.position.x).as_f32();
                            ratio_cell.set(dragged_ratio(body, raw));
                            window.refresh();
                        }
                    });
                    window.on_mouse_event({
                        let ratio_cell = ratio_cell.clone();
                        let dragging = dragging.clone();
                        move |_ev: &MouseUpEvent, _phase, window, cx| {
                            if !dragging.get() {
                                return;
                            }
                            dragging.set(false);
                            let r = ratio_cell.get();
                            let cfg = cx.global_mut::<Config>();
                            if cfg.document_ratio != r {
                                cfg.document_ratio = r;
                                cfg.save();
                            }
                            window.refresh();
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element();

        let active = self.editor.document_dragging.get();
        let handle = div()
            .id("document-resize")
            .group("document-resize")
            .occlude()
            .absolute()
            .top_0()
            .left(px(-(RESIZE_HANDLE_WIDTH / 2.)))
            .w(px(RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_col_resize()
            .child(
                div()
                    .w(px(1.))
                    .h_full()
                    .when(active, |d| d.bg(cx.theme().drag_border))
                    .group_hover("document-resize", |s| s.bg(cx.theme().drag_border)),
            )
            .on_mouse_down(MouseButton::Left, {
                let dragging = self.editor.document_dragging.clone();
                move |_ev, window, _cx| {
                    dragging.set(true);
                    window.refresh();
                }
            })
            // A double-click lands a mouse-down first, which arms the drag; the
            // mouse-up disarms it without having moved, so the cycle below is
            // the only thing that ends up happening.
            .on_double_click(cx.listener(|this, _, _window, cx| {
                this.editor.document_dragging.set(false);
                this.cycle_document_ratio(cx);
            }))
            .into_any_element();

        (backing, handle)
    }
}

/// The rule on the docked column's left edge, in points.
///
/// Named rather than written twice: the content box below is the column less
/// this, and the two have to move together.
pub const DOCUMENT_RULE: f32 = 1.;

/// The width the docked column's content box is drawn at.
///
/// The column is sized in points here, and its content has to be sized the
/// same way. It used to be `w_full()` — a share of whatever box the flex
/// ancestor chain resolved to — and on a fractional display scale (a 1.75x
/// Wayland surface, GNOME's `scale-monitor-framebuffer`) that share came out
/// against the wrong box entirely: a 404-point column handed its file a
/// 123-point content box, so the editor laid out at a fifth of the space it
/// was given and soft-wrapped every few characters (#1150). A point count has
/// no ancestor to be resolved against.
///
/// [`DOCUMENT_RULE`] is taken off the content because the column draws it as a
/// border: sizing the content at the full width instead would push it a point
/// past the column, and the clip would eat that off the right.
pub fn document_content_px(column: f32) -> f32 {
    (column - DOCUMENT_RULE).max(0.)
}

/// The ratio a divider dropped `raw` points from the right edge of a `body`
/// wide area settles on.
///
/// Two clamps, and both are load-bearing. The pixel one keeps the terminal and
/// the document each above their floor, and is what binds on a narrow window.
/// The ratio one is the band the *file* keeps — `Config::sanitize` holds
/// `document_ratio` to it, so a drag that wrote outside it would be moved on
/// the next launch, and a column dropped against the edge of a wide window
/// would reopen hundreds of points from where it was left.
pub fn dragged_ratio(body: f32, raw: f32) -> f32 {
    let w = raw.clamp(DOCUMENT_MIN_W, (body - TERMINAL_MIN_W).max(DOCUMENT_MIN_W));
    (w / body).clamp(DOCUMENT_RATIO_MIN, DOCUMENT_RATIO_MAX)
}

/// The width the divider's double-click moves to from `current`: the one after
/// whichever named stop `current` is nearest, wrapping round.
pub fn next_ratio_stop(current: f32) -> f32 {
    let nearest = DOCUMENT_RATIO_STOPS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (*a - current).abs().total_cmp(&(*b - current).abs()))
        .map(|(i, _)| i)
        .unwrap_or(1);
    DOCUMENT_RATIO_STOPS[(nearest + 1) % DOCUMENT_RATIO_STOPS.len()]
}

#[cfg(test)]
mod tests {
    use super::{
        DOCUMENT_MIN_W, DocumentChrome, document_column_px, dragged_ratio, layout_switch_cell,
        next_ratio_stop,
    };
    use anterm_ui_kit::config::{
        DOCUMENT_RATIO_HALF, DOCUMENT_RATIO_MAX, DOCUMENT_RATIO_MIN, DOCUMENT_RATIO_THIRD,
        DOCUMENT_RATIO_TWO_THIRDS, DocumentLayout,
    };
    use anterm_ui_kit::window_chrome::TERMINAL_MIN_W;

    /// Both document headers raise the cell that matches what is on screen, so
    /// the narrow-window fallback — where a tab asked to dock is drawn filled —
    /// cannot leave the switch pointing at a box the surface is not in.
    #[test]
    fn the_layout_switch_raises_the_box_the_document_is_actually_in() {
        assert_eq!(
            layout_switch_cell(DocumentChrome::Fill),
            DocumentLayout::Fill,
            "the overlay is the whole workspace"
        );
        for column in [DocumentChrome::Dock, DocumentChrome::DockHoisted] {
            assert_eq!(
                layout_switch_cell(column),
                DocumentLayout::Dock,
                "a column is a column, hoisted header or not"
            );
        }
    }

    /// Nothing the divider can write is something the next launch moves.
    /// `Config::sanitize` clamps `document_ratio` into a band, so a drag that
    /// left the band was saved and then quietly relocated: on a 2560-point body
    /// the narrowest the divider went was a ratio of 0.11, which came back as
    /// 0.2 — a 232-point jump the user never asked for. The drag clamps to the
    /// file's band as well as to the two floors now, and this is what says so.
    #[test]
    fn a_dragged_width_is_one_the_config_can_keep() {
        for body in [640., 900., 1440., 2560., 5120.] {
            for raw in [-500., 0., 1., DOCUMENT_MIN_W, body / 2., body, body + 900.] {
                let r = dragged_ratio(body, raw);
                assert!(
                    (DOCUMENT_RATIO_MIN..=DOCUMENT_RATIO_MAX).contains(&r),
                    "body {body} dropped at {raw} wrote {r}, which the file would not keep"
                );
                // And the width that ratio draws still holds the invariant the
                // whole budget exists for.
                let drawn = document_column_px(body, r).expect("a body that seats both");
                assert!(body - drawn >= TERMINAL_MIN_W - f32::EPSILON);
                assert!(drawn >= DOCUMENT_MIN_W - f32::EPSILON);
            }
        }
    }

    /// #1150: the content box is the column's own width, less the rule on its
    /// left edge — never a share of a box resolved further up the flex chain.
    /// A fractional display scale made that share resolve against the wrong
    /// ancestor, and a 404-point column gave its file 123 points, so a docked
    /// document soft-wrapped every few characters.
    #[test]
    fn the_docked_content_is_the_columns_own_width_less_its_rule() {
        use super::{DOCUMENT_RULE, document_content_px};
        for column in [DOCUMENT_MIN_W, 320., 404.54688, 542., 1920.] {
            let content = document_content_px(column);
            assert_eq!(content, column - DOCUMENT_RULE, "column {column}");
            assert!(
                content > 0.,
                "a column of {column} still has to have room for a file"
            );
        }
        // A column narrower than its own rule is not a layout; it must not
        // answer with a negative width for taffy to reject.
        assert_eq!(document_content_px(0.), 0.);
        assert_eq!(document_content_px(0.5), 0.);
    }

    /// Third, half, two thirds, round again — and a dragged width joins at
    /// whichever stop it looks nearest to rather than always restarting.
    #[test]
    fn the_divider_cycles_the_named_widths() {
        assert_eq!(next_ratio_stop(DOCUMENT_RATIO_THIRD), DOCUMENT_RATIO_HALF);
        assert_eq!(
            next_ratio_stop(DOCUMENT_RATIO_HALF),
            DOCUMENT_RATIO_TWO_THIRDS
        );
        assert_eq!(
            next_ratio_stop(DOCUMENT_RATIO_TWO_THIRDS),
            DOCUMENT_RATIO_THIRD
        );
        // Dragged to just under half: nearest stop is half, so the cycle goes
        // on to two thirds rather than back to a third.
        assert_eq!(next_ratio_stop(0.47), DOCUMENT_RATIO_TWO_THIRDS);
        assert_eq!(next_ratio_stop(0.8), DOCUMENT_RATIO_THIRD);
    }

    /// The default is half of what the terminal and the document share, and
    /// what they share is the window less the panels — not the window.
    #[test]
    fn half_of_the_body_is_half_of_the_body() {
        let body = 1440. - 220. - 260.;
        assert_eq!(document_column_px(body, 0.5), Some(body / 2.));
    }

    /// Two thirds is allowed past the half-window cap the side panels obey.
    /// Only the terminal's floor binds it.
    #[test]
    fn two_thirds_is_two_thirds_until_the_terminal_floor_says_otherwise() {
        let wide = 1200.;
        assert_eq!(document_column_px(wide, 2. / 3.), Some(wide * 2. / 3.));

        // 800 * 2/3 is 533, which would leave the terminal 267 — under its
        // floor — so the column stops where the terminal starts.
        let tight = 800.;
        assert_eq!(
            document_column_px(tight, 2. / 3.),
            Some(tight - TERMINAL_MIN_W)
        );
    }

    /// A column narrower than it can be read at is not a column. The ratio
    /// floors out rather than shrinking with the window.
    #[test]
    fn a_thin_share_still_gets_the_documents_floor() {
        let body = 700.;
        assert_eq!(document_column_px(body, 0.2), Some(DOCUMENT_MIN_W));
    }

    /// Below the width where both fit there is no docked layout to draw, and
    /// the caller falls back to the overlay for the frame. The threshold is
    /// exact so that widening by a point re-docks.
    #[test]
    fn a_window_too_narrow_for_both_has_no_docked_width() {
        let floor = TERMINAL_MIN_W + DOCUMENT_MIN_W;
        assert_eq!(document_column_px(floor - 1., 0.5), None);
        assert_eq!(document_column_px(floor, 0.5), Some(DOCUMENT_MIN_W));
        assert_eq!(document_column_px(f32::NAN, 0.5), None);
    }

    /// Whatever the terminal is left, it is never less than its floor. That is
    /// the invariant the whole budget exists for.
    #[test]
    fn the_terminal_keeps_its_floor_at_every_share() {
        for body in [640., 700., 900., 1200., 2400.] {
            for ratio in [0.2, 1. / 3., 0.5, 2. / 3., 0.8] {
                let Some(doc) = document_column_px(body, ratio) else {
                    continue;
                };
                assert!(
                    body - doc >= TERMINAL_MIN_W - f32::EPSILON,
                    "body {body} ratio {ratio} left the terminal {}",
                    body - doc
                );
                assert!(doc >= DOCUMENT_MIN_W - f32::EPSILON);
            }
        }
    }
}

#[cfg(test)]
mod gpui_tests {
    use super::*;
    use anterm_ui_kit::config::{DOCUMENT_RATIO_TWO_THIRDS, DocumentLayout};
    use crate::CodeEditorWorkspace;
    use anterm_core::machine::TabId;
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};

    fn standalone_workspace(
        cx: &mut TestAppContext,
        w: f32,
        tab_count: usize,
    ) -> (Entity<CodeEditorWorkspace>, Vec<TabId>, VisualTestContext) {
        let (ws, tab_ids, vcx) =
            crate::workspace::test_support::harness_with_tabs(cx, tab_count);
        vcx.simulate_resize(size(px(w), px(900.)));
        vcx.run_until_parked();
        (ws, tab_ids, vcx)
    }

    fn set_active_tab(
        ws: &Entity<CodeEditorWorkspace>,
        tab: TabId,
        vcx: &mut VisualTestContext,
    ) {
        ws.update_in(vcx, |ws, _, cx| {
            ws.ctx.active_tab = Some(tab);
            if !ws.ctx.live_tabs.contains(&tab) {
                ws.ctx.live_tabs.push(tab);
            }
            cx.notify();
        });
    }

    fn dock_px(ws: &Entity<CodeEditorWorkspace>, vcx: &mut VisualTestContext) -> Option<f32> {
        ws.update_in(vcx, |ws, window, cx| ws.document_dock_px(window, cx))
    }

    fn config_layout(vcx: &mut VisualTestContext) -> DocumentLayout {
        vcx.update(|_, cx| cx.global::<Config>().document_layout)
    }

    fn tab_layout(ws: &Entity<CodeEditorWorkspace>, vcx: &mut VisualTestContext) -> DocumentLayout {
        ws.update_in(vcx, |ws, _, cx| ws.document_layout(cx))
    }

    #[gpui::test]
    fn one_tabs_fill_leaves_the_other_docked(cx: &mut TestAppContext) {
        let (ws, tabs, mut vcx) = standalone_workspace(cx, 1440., 2);

        for &tab in &tabs {
            set_active_tab(&ws, tab, &mut vcx);
            ws.update_in(&mut vcx, |ws, window, cx| ws.toggle_code_panel(window, cx));
        }
        vcx.run_until_parked();

        set_active_tab(&ws, tabs[1], &mut vcx);
        ws.update_in(&mut vcx, |ws, _, cx| ws.toggle_document_fill(cx));
        vcx.run_until_parked();
        assert_eq!(dock_px(&ws, &mut vcx), None, "the tab that asked, fills");

        set_active_tab(&ws, tabs[0], &mut vcx);
        assert!(
            dock_px(&ws, &mut vcx).is_some(),
            "the tab that did not ask stays docked"
        );
        assert_eq!(config_layout(&mut vcx), DocumentLayout::Dock);

        let fresh = TabId::new();
        set_active_tab(&ws, fresh, &mut vcx);
        ws.update_in(&mut vcx, |ws, window, cx| ws.toggle_code_panel(window, cx));
        assert!(
            dock_px(&ws, &mut vcx).is_some(),
            "a fresh tab starts from config layout, not from tab 1"
        );
    }

    #[gpui::test]
    fn closing_the_surface_gives_the_width_back(cx: &mut TestAppContext) {
        let (ws, _, mut vcx) = standalone_workspace(cx, 1440., 1);
        ws.update_in(&mut vcx, |ws, window, cx| ws.toggle_code_panel(window, cx));
        vcx.run_until_parked();
        assert!(dock_px(&ws, &mut vcx).is_some());
        assert_eq!(
            ws.update_in(&mut vcx, |ws, _, cx| ws.document_floor(cx)),
            DOCUMENT_MIN_W
        );

        ws.update_in(&mut vcx, |ws, window, cx| ws.toggle_code_panel(window, cx));
        vcx.run_until_parked();
        assert_eq!(ws.update_in(&mut vcx, |ws, _, _| ws.document_front()), None);
        assert_eq!(dock_px(&ws, &mut vcx), None);
        assert_eq!(
            ws.update_in(&mut vcx, |ws, _, cx| ws.document_floor(cx)),
            0.
        );
    }

    #[gpui::test]
    fn a_narrow_window_falls_back_without_saving_it(cx: &mut TestAppContext) {
        let (ws, _, mut vcx) = standalone_workspace(cx, 560., 1);
        ws.update_in(&mut vcx, |ws, window, cx| ws.toggle_code_panel(window, cx));
        vcx.run_until_parked();

        assert_eq!(dock_px(&ws, &mut vcx), None, "no room for both");
        assert_eq!(config_layout(&mut vcx), DocumentLayout::Dock);

        vcx.simulate_resize(size(px(1440.), px(900.)));
        vcx.run_until_parked();
        assert!(
            dock_px(&ws, &mut vcx).is_some(),
            "widening re-docks on the next frame"
        );
        assert_eq!(config_layout(&mut vcx), DocumentLayout::Dock);
    }

    #[gpui::test]
    fn asking_to_fill_is_kept_and_a_named_width_docks_again(cx: &mut TestAppContext) {
        let (ws, _, mut vcx) = standalone_workspace(cx, 560., 1);
        ws.update_in(&mut vcx, |ws, window, cx| {
            ws.toggle_code_panel(window, cx);
            ws.toggle_document_fill(cx);
        });
        vcx.run_until_parked();
        assert_eq!(tab_layout(&ws, &mut vcx), DocumentLayout::Fill);

        vcx.simulate_resize(size(px(1440.), px(900.)));
        vcx.run_until_parked();
        assert_eq!(
            dock_px(&ws, &mut vcx),
            None,
            "a wide window does not overrule a chosen fill"
        );

        ws.update_in(&mut vcx, |ws, _, cx| {
            ws.set_document_ratio(DOCUMENT_RATIO_TWO_THIRDS, cx)
        });
        vcx.run_until_parked();
        assert_eq!(tab_layout(&ws, &mut vcx), DocumentLayout::Dock);
        let body = ws.update_in(&mut vcx, |ws, window, cx| ws.document_body_px(window, cx));
        let docked = dock_px(&ws, &mut vcx).expect("docked again");
        let want = (body * 2. / 3.).min(body - TERMINAL_MIN_W);
        assert!((docked - want).abs() < 0.5, "{docked} of {body}");
        assert_eq!(
            vcx.update(|_, cx| cx.global::<Config>().document_ratio),
            DOCUMENT_RATIO_TWO_THIRDS
        );

        ws.update_in(&mut vcx, |ws, _, cx| {
            ws.set_document_layout(DocumentLayout::Fill, cx)
        });
        vcx.run_until_parked();
        assert_eq!(tab_layout(&ws, &mut vcx), DocumentLayout::Fill);
        assert_eq!(dock_px(&ws, &mut vcx), None);
        assert_eq!(
            vcx.update(|_, cx| cx.global::<Config>().document_ratio),
            DOCUMENT_RATIO_TWO_THIRDS
        );
    }
}
