mod tree;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Axis, Bounds, Entity, InteractiveElement as _, MouseButton, MouseMoveEvent, MouseUpEvent,
    Pixels, Window, canvas, div, prelude::*, px,
};
use gpui_kit::component::ActiveTheme as _;

use crate::terminal::view::TerminalView;

#[cfg(test)]
pub use tree::Rect;
pub use tree::{CloseOutcome, Dir, Pane, PaneSlot};
use tree::{MAX_RATIO, MIN_RATIO};

const DIVIDER_THICKNESS: f32 = 5.;

/// How opaque a dragged (lifted) pane's terminal paints, blended toward the
/// window background. The terminal reads it through `TerminalView::dim`; kept
/// here so that field's docs can point at the real numbers instead of
/// duplicating them.
pub(crate) const LIFTED_DIM: f32 = 0.45;
/// How opaque an unfocused pane in a split paints while `dim_inactive_panes`
/// is on.
pub(crate) const INACTIVE_DIM: f32 = 0.55;

/// What a tab wants drawn around its panes this frame.
pub(crate) struct PaneChrome {
    /// Fade every pane but the focused one.
    pub dim_inactive: bool,
    /// Whether a pane can be picked up and put somewhere else. False for a tab
    /// holding one pane: there is nowhere to move it to.
    pub rearrangeable: bool,
    /// The pane whose top edge the pointer is near. The leaves write it as the
    /// pointer crosses their reveal bands and the same frame's siblings read
    /// it, so at most one grip is ever drawn.
    pub hovered: Rc<Cell<Option<gpui::EntityId>>>,
    /// The pane being dragged, drawn faded where it came from.
    pub lifted: Option<gpui::EntityId>,
    pub drag: crate::ui::workspace::pane_drag::PaneDragState,
}

impl Pane<PaneSlot> {
    pub fn focused_leaf(&self, window: &Window, cx: &App) -> Option<PaneSlot> {
        match self {
            Pane::Leaf(v) => v.contains_focused(window, cx).then(|| v.clone()),
            Pane::Split { a, b, .. } => a
                .focused_leaf(window, cx)
                .or_else(|| b.focused_leaf(window, cx)),
            Pane::Empty => None,
        }
    }

    pub fn focused_or_first_slot(&self, window: &Window, cx: &App) -> Option<PaneSlot> {
        self.focused_leaf(window, cx).or_else(|| self.first_leaf())
    }

    pub fn focused_or_first(&self, window: &Window, cx: &App) -> Option<Entity<TerminalView>> {
        self.focused_or_first_slot(window, cx)
            .and_then(|slot| slot.terminal().cloned())
    }

    pub fn terminals(&self) -> Vec<Entity<TerminalView>> {
        self.leaves()
            .iter()
            .filter_map(|slot| slot.terminal().cloned())
            .collect()
    }

    /// The pane focus moves to, `back` naming the pane the last move the other
    /// way started from. A `back` that is no longer a leaf here — closed, or
    /// left behind in another tab — is simply not found; one that is still here
    /// but no longer next to `from` loses to the pane that is.
    pub fn neighbor_in_dir(
        &self,
        dir: Dir,
        back: Option<gpui::EntityId>,
        window: &Window,
        cx: &App,
    ) -> Option<PaneSlot> {
        let focused = self.focused_leaf(window, cx)?;
        let leaves = self.leaves();
        let from = leaves
            .iter()
            .position(|l| l.entity_id() == focused.entity_id())?;
        let back = back.and_then(|id| leaves.iter().position(|l| l.entity_id() == id));
        let target = self.focus_target_in_direction(from, dir, back)?;
        leaves.get(target).cloned()
    }

    pub fn resize_focused_pane(&self, dir: Dir, step: f32, window: &Window, cx: &App) -> bool {
        let Some(focused) = self.focused_leaf(window, cx) else {
            return false;
        };
        self.resize_focused(&|v| v.entity_id() == focused.entity_id(), dir, step)
    }

    pub fn focused_index(&self, window: &Window, cx: &App) -> Option<usize> {
        let focused = self.focused_leaf(window, cx)?;
        self.leaves()
            .iter()
            .position(|l| l.entity_id() == focused.entity_id())
    }

    pub fn split_leaf(
        &mut self,
        target: gpui::EntityId,
        axis: Axis,
        before: bool,
        new: PaneSlot,
    ) -> bool {
        self.split_leaf_where(&|v| v.entity_id() == target, axis, before, new)
    }

    pub fn replace_leaf(&mut self, target: gpui::EntityId, new: PaneSlot) -> bool {
        self.replace_leaf_where(&|v| v.entity_id() == target, new)
    }

    pub fn close_focused(&mut self, window: &Window, cx: &App) -> CloseOutcome {
        self.close_leaf_where(&|v| v.contains_focused(window, cx))
    }

    pub fn close_leaf(&mut self, target: gpui::EntityId) -> CloseOutcome {
        self.close_leaf_where(&|v| v.entity_id() == target)
    }

    pub(crate) fn render(
        &self,
        chrome: &PaneChrome,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::AnyElement {
        match self {
            Pane::Empty => div().into_any_element(),
            Pane::Leaf(v) => {
                let id = v.entity_id();
                let focused = v.contains_focused(window, cx);
                let lifted = chrome.lifted == Some(id);
                let grip = chrome.rearrangeable && chrome.lifted.is_none();
                // The terminal renders itself at this opacity by blending its
                // colours toward the window background (`TerminalView::dim`,
                // whose field docs explain why that beats an element-opacity
                // style). The connecting screen has no stacked content, so it
                // keeps the plain opacity style.
                let dim = if lifted {
                    LIFTED_DIM
                } else if chrome.dim_inactive && !focused {
                    INACTIVE_DIM
                } else {
                    1.0
                };
                div()
                    .size_full()
                    .relative()
                    .overflow_hidden()
                    .when(chrome.rearrangeable, |d| {
                        d.pt(px(crate::ui::workspace::pane_drag::HANDLE_STRIP))
                    })
                    .map(|d| match v {
                        PaneSlot::Ready(t) => {
                            t.update(cx, |v, _cx| v.set_dim(dim));
                            d.child(t.clone())
                        }
                        PaneSlot::Connecting(p) => {
                            d.when(dim < 1., |d| d.opacity(dim)).child(p.clone())
                        }
                    })
                    .when(grip, |d| {
                        d.child(crate::ui::workspace::pane_drag::reveal_band(id, &chrome.hovered))
                            .child(crate::ui::workspace::pane_drag::handle(
                                id,
                                chrome.hovered.get() == Some(id),
                                &chrome.drag,
                                cx,
                            ))
                    })
                    .into_any_element()
            }
            Pane::Split {
                axis,
                a,
                b,
                ratio,
                dragging,
            } => {
                let row = *axis == Axis::Horizontal;
                let r = ratio.get().clamp(MIN_RATIO, MAX_RATIO);

                // At rest the split is a v4 hairline in the divider tone — the
                // line between two panes that already carry their own content,
                // not the stronger rule that outlines a floating surface. Hover
                // and drag keep the accent the sidebar and right-panel edges
                // light up with, so every resize handle answers the same way.
                let idle = cx.theme().sidebar_border;
                let active = cx.theme().drag_border;
                let hairline = crate::ui::chrome::theme::hairline(window);

                let container: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));

                let backing = canvas(
                    {
                        let container = container.clone();
                        move |bounds, _window, _cx| container.set(Some(bounds))
                    },
                    {
                        let container = container.clone();
                        let ratio = ratio.clone();
                        let dragging = dragging.clone();
                        move |_bounds, _state, window, _cx| {
                            window.on_mouse_event({
                                let container = container.clone();
                                let ratio = ratio.clone();
                                let dragging = dragging.clone();
                                move |ev: &MouseMoveEvent, _phase, window, _cx| {
                                    if !dragging.get() {
                                        return;
                                    }
                                    let Some(b) = container.get() else {
                                        return;
                                    };
                                    let span = if row { b.size.width } else { b.size.height };
                                    if span.as_f32() <= 0.0 {
                                        return;
                                    }
                                    let offset = if row {
                                        ev.position.x - b.origin.x
                                    } else {
                                        ev.position.y - b.origin.y
                                    };
                                    let new_ratio = offset / span;
                                    ratio.set(new_ratio.clamp(MIN_RATIO, MAX_RATIO));
                                    window.refresh();
                                }
                            });
                            window.on_mouse_event({
                                let dragging = dragging.clone();
                                move |_ev: &MouseUpEvent, _phase, window, cx| {
                                    if dragging.get() {
                                        dragging.set(false);
                                        if let Some(app) =
                                            crate::ui::workspace::windows::WindowRegistry::app_in(cx, window)
                                        {
                                            app.update(cx, |app, cx| app.save_session(cx));
                                        }
                                        window.refresh();
                                    }
                                }
                            });
                        }
                    },
                )
                .absolute()
                .size_full();

                let line_color = if dragging.get() { active } else { idle };
                // A lit handle goes back to a whole pixel: a hairline of accent
                // is too faint to say "this is what you are holding".
                let line = if dragging.get() { px(1.) } else { hairline };
                // The gutter stays 5px so the split looks the same; the target
                // is the 8px the sidebar and right-panel edges already hand
                // you. The extra 1.5px a side reaches into the pane's own 8px
                // GRID_PAD_X, so it never covers a cell of terminal text.
                let overhang =
                    (crate::ui::files::right_panel::RESIZE_HANDLE_WIDTH - DIVIDER_THICKNESS).max(0.) / 2.;
                let grab = div()
                    .occlude()
                    .absolute()
                    .when(row, |d| {
                        d.top_0()
                            .h_full()
                            .left(px(-overhang))
                            .w(px(DIVIDER_THICKNESS + overhang * 2.))
                            .cursor_col_resize()
                    })
                    .when(!row, |d| {
                        d.left_0()
                            .w_full()
                            .top(px(-overhang))
                            .h(px(DIVIDER_THICKNESS + overhang * 2.))
                            .cursor_row_resize()
                    })
                    .on_mouse_down(MouseButton::Left, {
                        let dragging = dragging.clone();
                        move |_ev, window, _cx| {
                            dragging.set(true);
                            window.refresh();
                        }
                    });
                let divider = div()
                    .group("split-divider")
                    .flex_none()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(row, |d| d.w(px(DIVIDER_THICKNESS)).h_full())
                    .when(!row, |d| d.h(px(DIVIDER_THICKNESS)).w_full())
                    .child(
                        div()
                            .when(row, |d| d.w(line).h_full())
                            .when(!row, |d| d.h(line).w_full())
                            .bg(line_color)
                            .group_hover("split-divider", move |s| match row {
                                true => s.bg(active).w(px(1.)),
                                false => s.bg(active).h(px(1.)),
                            }),
                    )
                    .child(grab);

                div()
                    .size_full()
                    .relative()
                    .flex()
                    .when(row, |d| d.flex_row())
                    .when(!row, |d| d.flex_col())
                    .child(backing)
                    .child(
                        div()
                            .flex_grow(r)
                            .flex_shrink(1.)
                            .flex_basis(px(0.))
                            .min_w_0()
                            .min_h_0()
                            .child(a.render(chrome, window, cx)),
                    )
                    .child(divider)
                    .child(
                        div()
                            .flex_grow(1. - r)
                            .flex_shrink(1.)
                            .flex_basis(px(0.))
                            .min_w_0()
                            .min_h_0()
                            .child(b.render(chrome, window, cx)),
                    )
                    .into_any_element()
            }
        }
    }
}
