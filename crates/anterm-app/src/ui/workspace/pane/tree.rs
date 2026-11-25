use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, Axis, Entity, Window};

use crate::terminal::view::TerminalView;
use crate::ui::workspace::pending_pane::PendingPane;

pub(super) const MIN_RATIO: f32 = 0.1;
pub(super) const MAX_RATIO: f32 = 0.9;

#[derive(Clone)]
pub enum PaneSlot {
    Ready(Entity<TerminalView>),
    #[allow(dead_code)]
    Connecting(Entity<PendingPane>),
}

/// Two slots are the same pane when they hold the same view. The payload is a
/// handle, so identity is all there is to compare — and it is what the layout
/// operations below need in order to tell "this changed nothing" from a move.
impl PartialEq for PaneSlot {
    fn eq(&self, other: &Self) -> bool {
        self.entity_id() == other.entity_id()
    }
}

impl PaneSlot {
    pub fn entity_id(&self) -> gpui::EntityId {
        match self {
            PaneSlot::Ready(v) => v.entity_id(),
            PaneSlot::Connecting(v) => v.entity_id(),
        }
    }

    pub fn terminal(&self) -> Option<&Entity<TerminalView>> {
        match self {
            PaneSlot::Ready(v) => Some(v),
            PaneSlot::Connecting(_) => None,
        }
    }

    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        match self {
            PaneSlot::Ready(v) => v.read(cx).focus_handle.contains_focused(window, cx),
            PaneSlot::Connecting(v) => v.read(cx).focus_handle.contains_focused(window, cx),
        }
    }

    pub fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        match self {
            PaneSlot::Ready(v) => v.read(cx).focus_handle.clone(),
            PaneSlot::Connecting(v) => v.read(cx).focus_handle.clone(),
        }
    }
}

/// Deliberately not `Clone`: the two copies a tree can be asked for differ in
/// whether they share their splits' sizes, and that is not a difference to
/// leave to whichever one `.clone()` happens to mean. See
/// [`Pane::shallow_clone`] and [`Pane::deep_clone`].
pub enum Pane<L = PaneSlot> {
    Leaf(L),
    Split {
        axis: Axis,
        a: Box<Pane<L>>,
        b: Box<Pane<L>>,
        ratio: Rc<Cell<f32>>,
        dragging: Rc<Cell<bool>>,
    },
    Empty,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn axis(self) -> Axis {
        match self {
            Dir::Left | Dir::Right => Axis::Horizontal,
            Dir::Up | Dir::Down => Axis::Vertical,
        }
    }

    /// Whether a pane placed on this side lands in the `a` child of the split
    /// that holds it — the side the layout draws first.
    pub fn leads(self) -> bool {
        matches!(self, Dir::Left | Dir::Up)
    }

    fn grows(self) -> bool {
        matches!(self, Dir::Right | Dir::Down)
    }

    /// The direction that undoes a move this way.
    pub fn opposite(self) -> Dir {
        match self {
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn overlap_1d(a0: f32, alen: f32, b0: f32, blen: f32) -> f32 {
    ((a0 + alen).min(b0 + blen) - a0.max(b0)).max(0.0)
}

/// Slack for the arithmetic behind `leaf_rects`: ratios multiply out down the
/// tree, so edges that meet exactly in the layout can differ in the last bits.
const ADJACENCY_EPS: f32 = 1e-4;

/// How far `c` lies from `f` in `dir` and how much edge the two share, or
/// `None` when `c` is not on that side of `f` or only touches it at a corner.
pub(super) fn adjacency(f: Rect, c: Rect, dir: Dir) -> Option<(f32, f32)> {
    let (dist, overlap) = match dir {
        Dir::Left => (f.x - (c.x + c.w), overlap_1d(f.y, f.h, c.y, c.h)),
        Dir::Right => (c.x - (f.x + f.w), overlap_1d(f.y, f.h, c.y, c.h)),
        Dir::Up => (f.y - (c.y + c.h), overlap_1d(f.x, f.w, c.x, c.w)),
        Dir::Down => (c.y - (f.y + f.h), overlap_1d(f.x, f.w, c.x, c.w)),
    };
    (dist >= -ADJACENCY_EPS && overlap > ADJACENCY_EPS).then_some((dist, overlap))
}

pub enum CloseOutcome {
    NotFound,
    Collapsed,
    RemoveSelf,
}

impl<L: Clone> Pane<L> {
    pub fn leaf(view: L) -> Self {
        Pane::Leaf(view)
    }

    pub fn split_node(axis: Axis, ratio: f32, a: Pane<L>, b: Pane<L>) -> Self {
        Pane::Split {
            axis,
            a: Box::new(a),
            b: Box::new(b),
            ratio: Rc::new(Cell::new(ratio.clamp(MIN_RATIO, MAX_RATIO))),
            dragging: Rc::new(Cell::new(false)),
        }
    }

    pub fn collect_leaves<'a>(&'a self, out: &mut Vec<L>) {
        match self {
            Pane::Leaf(v) => out.push(v.clone()),
            Pane::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
            Pane::Empty => {}
        }
    }

    pub fn leaves(&self) -> Vec<L> {
        let mut v = Vec::new();
        self.collect_leaves(&mut v);
        v
    }

    pub fn first_leaf(&self) -> Option<L> {
        match self {
            Pane::Leaf(v) => Some(v.clone()),
            Pane::Split { a, b, .. } => a.first_leaf().or_else(|| b.first_leaf()),
            Pane::Empty => None,
        }
    }

    /// Whether zooming the leaf `pred` names would actually hide anything.
    ///
    /// The zoom that gets marked in the chrome is the one a reader cannot see
    /// for themselves: a zoom naming a pane that has since exited names no
    /// leaf here, and a zoom over the last pane standing covers nothing. Both
    /// look exactly like an unzoomed single pane, so neither earns a badge.
    pub fn zoom_hides_siblings(&self, pred: impl Fn(&L) -> bool) -> bool {
        let leaves = self.leaves();
        leaves.len() > 1 && leaves.iter().any(pred)
    }

    pub fn leaf_matching_or_first(&self, pred: impl Fn(&L) -> bool) -> Option<L> {
        self.leaves()
            .into_iter()
            .find(|l| pred(l))
            .or_else(|| self.first_leaf())
    }

    pub(super) fn split_leaf_where(
        &mut self,
        is_target: &impl Fn(&L) -> bool,
        axis: Axis,
        before: bool,
        new: L,
    ) -> bool {
        match self {
            Pane::Leaf(v) => {
                if is_target(v) {
                    let old = Pane::Leaf(v.clone());
                    let new = Pane::Leaf(new);
                    let (a, b) = if before { (new, old) } else { (old, new) };
                    *self = Pane::split_node(axis, 0.5, a, b);
                    true
                } else {
                    false
                }
            }
            Pane::Split { a, b, .. } => {
                a.split_leaf_where(is_target, axis, before, new.clone())
                    || b.split_leaf_where(is_target, axis, before, new)
            }
            Pane::Empty => false,
        }
    }

    /// Puts a whole subtree where the leaf `is_target` names stands.
    ///
    /// The subtree arrives in an `Option` the walk takes it out of: a tree of
    /// live panes cannot be cloned for every branch that might hold the target,
    /// and a caller whose target was not there gets it back to put somewhere
    /// else.
    fn replace_leaf_with(
        &mut self,
        is_target: &impl Fn(&L) -> bool,
        new: &mut Option<Pane<L>>,
    ) -> bool {
        match self {
            Pane::Leaf(v) => {
                if !is_target(v) {
                    return false;
                }
                let Some(new) = new.take() else {
                    return false;
                };
                *self = new;
                true
            }
            Pane::Split { a, b, .. } => {
                a.replace_leaf_with(is_target, new) || b.replace_leaf_with(is_target, new)
            }
            Pane::Empty => false,
        }
    }

    pub(super) fn replace_leaf_where(&mut self, is_target: &impl Fn(&L) -> bool, new: L) -> bool {
        match self {
            Pane::Leaf(v) => {
                if is_target(v) {
                    *v = new;
                    true
                } else {
                    false
                }
            }
            Pane::Split { a, b, .. } => {
                a.replace_leaf_where(is_target, new.clone()) || b.replace_leaf_where(is_target, new)
            }
            Pane::Empty => false,
        }
    }

    pub(super) fn close_leaf_where(&mut self, is_target: &impl Fn(&L) -> bool) -> CloseOutcome {
        match self {
            Pane::Leaf(v) => {
                if is_target(v) {
                    CloseOutcome::RemoveSelf
                } else {
                    CloseOutcome::NotFound
                }
            }
            Pane::Split { .. } => {
                let a_outcome = if let Pane::Split { a, .. } = self {
                    a.close_leaf_where(is_target)
                } else {
                    unreachable!()
                };
                match a_outcome {
                    CloseOutcome::RemoveSelf => {
                        if let Pane::Split { b, .. } = std::mem::replace(self, Pane::Empty) {
                            *self = *b;
                        }
                        return CloseOutcome::Collapsed;
                    }
                    CloseOutcome::Collapsed => return CloseOutcome::Collapsed,
                    CloseOutcome::NotFound => {}
                }

                let b_outcome = if let Pane::Split { b, .. } = self {
                    b.close_leaf_where(is_target)
                } else {
                    unreachable!()
                };
                match b_outcome {
                    CloseOutcome::RemoveSelf => {
                        if let Pane::Split { a, .. } = std::mem::replace(self, Pane::Empty) {
                            *self = *a;
                        }
                        CloseOutcome::Collapsed
                    }
                    other => other,
                }
            }
            Pane::Empty => CloseOutcome::NotFound,
        }
    }

    /// Lifts a leaf out of the tree, collapsing the split that held it.
    ///
    /// Answers `None` for the last pane in the tab: there is nowhere to put it
    /// back that is not where it already is, and a tree with no leaves is not a
    /// state this type is allowed to be in.
    fn take_leaf_where(&mut self, is_target: &impl Fn(&L) -> bool) -> Option<L> {
        let taken = self.leaves().into_iter().find(|l| is_target(l))?;
        match self.close_leaf_where(is_target) {
            CloseOutcome::Collapsed => Some(taken),
            CloseOutcome::RemoveSelf | CloseOutcome::NotFound => None,
        }
    }

    /// A copy that shares its splits' sizes with the original.
    ///
    /// What the layout edits below want: they build the next shape on one of
    /// these and install it only once it is known to be a real change, and a
    /// split that survives the edit must keep the size the user dragged it to.
    fn shallow_clone(&self) -> Self {
        match self {
            Pane::Leaf(v) => Pane::Leaf(v.clone()),
            Pane::Empty => Pane::Empty,
            Pane::Split {
                axis,
                a,
                b,
                ratio,
                dragging,
            } => Pane::Split {
                axis: *axis,
                a: Box::new(a.shallow_clone()),
                b: Box::new(b.shallow_clone()),
                ratio: ratio.clone(),
                dragging: dragging.clone(),
            },
        }
    }

    /// A copy with sizes of its own, safe to try a rearrangement out on.
    ///
    /// The shared-size copy above is what an edit about to be installed wants,
    /// but not what a hover wants: resizing the tried-out tree would resize the
    /// one still on screen.
    pub fn deep_clone(&self) -> Self {
        match self {
            Pane::Leaf(v) => Pane::Leaf(v.clone()),
            Pane::Empty => Pane::Empty,
            Pane::Split {
                axis, ratio, a, b, ..
            } => Pane::split_node(*axis, ratio.get(), a.deep_clone(), b.deep_clone()),
        }
    }

    fn holds(&self, pred: &impl Fn(&L) -> bool) -> bool {
        self.leaves().iter().any(pred)
    }

    /// The node heading the run of `axis` splits that lays out the leaf `pred`
    /// names — the row it is a cell of, or the column.
    ///
    /// `None` when nothing along the way splits on that axis: the leaf is not
    /// part of a run in that direction, so there is no row to share out.
    fn run_head_mut(&mut self, axis: Axis, pred: &impl Fn(&L) -> bool) -> Option<&mut Pane<L>> {
        if !self.holds(pred) {
            return None;
        }
        if matches!(self, Pane::Split { axis: a, .. } if *a == axis) {
            return Some(self);
        }
        match self {
            Pane::Split { a, b, .. } => {
                if a.holds(pred) {
                    a.run_head_mut(axis, pred)
                } else {
                    b.run_head_mut(axis, pred)
                }
            }
            _ => None,
        }
    }

    /// What each piece of this run takes of it, in the order they are laid out.
    fn run_shares(&self, axis: Axis, of: f32, out: &mut Vec<f32>) {
        match self {
            Pane::Split {
                axis: split,
                ratio,
                a,
                b,
                ..
            } if *split == axis => {
                let r = ratio.get().clamp(MIN_RATIO, MAX_RATIO);
                a.run_shares(axis, of * r, out);
                b.run_shares(axis, of * (1. - r), out);
            }
            _ => out.push(of),
        }
    }

    /// Hands the run back its shares, in that same order, answering the total
    /// it took so each split above can be set from the two sides it holds.
    fn set_run_shares(&mut self, axis: Axis, shares: &mut impl Iterator<Item = f32>) -> f32 {
        match self {
            Pane::Split {
                axis: split,
                ratio,
                a,
                b,
                ..
            } if *split == axis => {
                let left = a.set_run_shares(axis, shares);
                let right = b.set_run_shares(axis, shares);
                let total = left + right;
                if total > 0. {
                    ratio.set((left / total).clamp(MIN_RATIO, MAX_RATIO));
                }
                total
            }
            _ => shares.next().unwrap_or(0.),
        }
    }

    /// Which piece of the run is the leaf `pred` names, when it is a piece of
    /// the run in its own right rather than something nested inside one.
    fn run_index(&self, axis: Axis, pred: &impl Fn(&L) -> bool, at: &mut usize) -> Option<usize> {
        match self {
            Pane::Split {
                axis: split, a, b, ..
            } if *split == axis => a
                .run_index(axis, pred, at)
                .or_else(|| b.run_index(axis, pred, at)),
            Pane::Leaf(v) if pred(v) => Some(*at),
            _ => {
                *at += 1;
                None
            }
        }
    }

    /// Shares a newly inserted pane into the run it joined, giving it an even
    /// piece and taking that piece off the others in proportion.
    ///
    /// This is the difference between dropping a pane against its neighbour and
    /// carving up the neighbour: three equal columns rather than one of them
    /// quartered. A pane that landed inside a run's piece instead of beside it
    /// keeps the half of its target it was given — there is no row it joined.
    fn share_out_run(&mut self, axis: Axis, is_new: &impl Fn(&L) -> bool, leads: bool) {
        let Some(head) = self.run_head_mut(axis, is_new) else {
            return;
        };
        let mut shares = Vec::new();
        head.run_shares(axis, 1., &mut shares);
        let Some(new) = head.run_index(axis, is_new, &mut 0) else {
            return;
        };
        let n = shares.len();
        // The newcomer went in beside the pane it split, on the side the drop
        // named, and the two of them are holding that pane's old piece between
        // them. Everyone else keeps what they had, less the newcomer's share.
        let split = if leads { new + 1 } else { new.wrapping_sub(1) };
        let (Some(&mine), Some(&theirs)) = (shares.get(new), shares.get(split)) else {
            return;
        };
        let keep = (n - 1) as f32 / n as f32;
        let mut want: Vec<f32> = shares.iter().map(|s| s * keep).collect();
        want[new] = 1. / n as f32;
        want[split] = (mine + theirs) * keep;
        head.set_run_shares(axis, &mut want.into_iter());
    }

    /// Whether two trees put the same panes in the same places. Ratios are not
    /// part of it: what this answers is "did the drag change anything".
    fn same_layout(&self, other: &Self) -> bool
    where
        L: PartialEq,
    {
        match (self, other) {
            (Pane::Leaf(a), Pane::Leaf(b)) => a == b,
            (
                Pane::Split {
                    axis: ax,
                    a: aa,
                    b: ab,
                    ..
                },
                Pane::Split {
                    axis: bx,
                    a: ba,
                    b: bb,
                    ..
                },
            ) => ax == bx && aa.same_layout(ba) && ab.same_layout(bb),
            (Pane::Empty, Pane::Empty) => true,
            _ => false,
        }
    }

    /// Moves one leaf to `dir` of another, splitting the destination.
    ///
    /// The source is lifted out first, so the destination is the tree as it
    /// stands *after* the collapse — dropping a pane next to its own sibling
    /// therefore lands where the eye expects rather than nesting a split that
    /// is about to disappear. A move that would redraw the same layout is
    /// refused, so an idle drag does not churn the session file.
    pub(super) fn move_leaf_where(
        &mut self,
        is_src: &impl Fn(&L) -> bool,
        is_dst: &impl Fn(&L) -> bool,
        dir: Dir,
    ) -> bool
    where
        L: PartialEq,
    {
        let mut next = self.shallow_clone();
        let Some(moved) = next.take_leaf_where(is_src) else {
            return false;
        };
        if !next.split_leaf_where(is_dst, dir.axis(), dir.leads(), moved) {
            return false;
        }
        if next.same_layout(self) {
            return false;
        }
        // Only once the move is going to happen: sharing the run out writes
        // ratios that this tree's splits hold in common with the one on screen.
        next.share_out_run(dir.axis(), is_src, dir.leads());
        *self = next;
        true
    }

    /// How many bands this layout already presents along `axis` — the columns
    /// you would count across it, or the rows down it.
    ///
    /// Splits on that axis add their sides up; splits across it stack, so the
    /// count is the widest of the two rather than their sum. A 2×2 therefore
    /// answers two columns even though no single node cuts it into two.
    pub(super) fn slices_along(&self, axis: Axis) -> usize {
        match self {
            Pane::Leaf(_) => 1,
            Pane::Empty => 0,
            Pane::Split {
                axis: split, a, b, ..
            } => {
                let (l, r) = (a.slices_along(axis), b.slices_along(axis));
                if *split == axis { l + r } else { l.max(r) }
            }
        }
    }

    /// Lifts a leaf out and works out the share of the tab it should take back
    /// as a band along `dir`: one more band than the layout already has, each
    /// of them the same width. A tab already cut into two columns therefore
    /// receives a third column, not a half.
    pub(super) fn edge_landing(&self, is_src: &impl Fn(&L) -> bool, dir: Dir) -> Option<Pane<L>> {
        let mut rest = self.shallow_clone();
        let moved = rest.take_leaf_where(is_src)?;
        let slices = rest.slices_along(dir.axis()).max(1);
        let share = 1. / (slices + 1) as f32;
        let (a, b) = if dir.leads() {
            (Pane::Leaf(moved), rest)
        } else {
            (rest, Pane::Leaf(moved))
        };
        let ratio = if dir.leads() { share } else { 1. - share };
        Some(Pane::split_node(dir.axis(), ratio, a, b))
    }

    /// Moves one leaf against an outer edge of the whole tab, as a full-width
    /// or full-height band beside everything that is left.
    pub(super) fn move_leaf_to_edge_where(
        &mut self,
        is_src: &impl Fn(&L) -> bool,
        dir: Dir,
    ) -> bool
    where
        L: PartialEq,
    {
        let Some(next) = self.edge_landing(is_src, dir) else {
            return false;
        };
        if next.same_layout(self) {
            return false;
        }
        *self = next;
        true
    }

    /// Drops `src` on the `dir` side of `dst`, splitting `dst` to make room —
    /// or, when that side faces a neighbour in the same row or column, joining
    /// them as an equal instead.
    pub fn move_leaf_beside(&mut self, src: &L, dst: &L, dir: Dir) -> bool
    where
        L: PartialEq,
    {
        src != dst && self.move_leaf_where(&|v| v == src, &|v| v == dst, dir)
    }

    /// Drops `src` against the `dir` edge of the tab, beside every other pane.
    pub fn move_leaf_to_edge(&mut self, src: &L, dir: Dir) -> bool
    where
        L: PartialEq,
    {
        self.move_leaf_to_edge_where(&|v| v == src, dir)
    }

    /// Lifts a leaf out of this tree, for whoever is taking it somewhere this
    /// tree cannot reach — a tab of its own, say.
    ///
    /// `None` when the tree holds nothing else: the last pane in a tab has
    /// nowhere to go that is not where it already is.
    pub fn take_leaf(&mut self, target: &L) -> Option<L>
    where
        L: PartialEq,
    {
        self.take_leaf_where(&|v| v == target)
    }

    /// Grafts a whole subtree in on the `dir` side of `dst`.
    ///
    /// The same reading as [`Self::move_leaf_beside`], with a tab's worth of
    /// panes arriving instead of one of this tab's own: the newcomer joins the
    /// run it faces as an equal where there is one, and splits `dst` where
    /// there is not. Whatever shape it brought with it it keeps.
    ///
    /// A graft with nowhere to go hands the subtree back rather than dropping
    /// it: what is being passed around is the live panes themselves, and a
    /// refusal that swallowed them would take a tab's worth of terminals down.
    ///
    /// Carried out as a one-pane move and then a swap — one of the newcomer's
    /// own panes goes in first and is traded for the whole tab once the row it
    /// joined has shared itself out. Grafting the tab straight in would have it
    /// dissolve into that row wherever the two split the same way: three panes
    /// arriving into a row of two would come out as five columns sharing a
    /// fifth each, rather than as one column of three beside the other two.
    pub fn graft_beside(&mut self, sub: Pane<L>, dst: &L, dir: Dir) -> Result<(), Pane<L>>
    where
        L: PartialEq,
    {
        let Some(anchor) = sub.first_leaf() else {
            return Err(sub);
        };
        let mut next = self.shallow_clone();
        if !next.split_leaf_where(&|v| v == dst, dir.axis(), dir.leads(), anchor.clone()) {
            return Err(sub);
        }
        next.share_out_run(dir.axis(), &|v| *v == anchor, dir.leads());
        let mut held = Some(sub);
        if !next.replace_leaf_with(&|v| *v == anchor, &mut held) {
            return Err(held.expect("the anchor this graft just planted is still here"));
        }
        *self = next;
        Ok(())
    }

    /// Grafts a whole subtree against an outer edge of the tab, beside
    /// everything already here — one more band along `dir`, sized like the
    /// bands it joins. Refused the same way as [`Self::graft_beside`].
    pub fn graft_at_edge(&mut self, sub: Pane<L>, dir: Dir) -> Result<(), Pane<L>> {
        if sub.leaves().is_empty() || matches!(self, Pane::Empty) {
            return Err(sub);
        }
        let slices = self.slices_along(dir.axis()).max(1);
        let share = 1. / (slices + 1) as f32;
        let rest = self.shallow_clone();
        let (a, b) = if dir.leads() {
            (sub, rest)
        } else {
            (rest, sub)
        };
        let ratio = if dir.leads() { share } else { 1. - share };
        *self = Pane::split_node(dir.axis(), ratio, a, b);
        Ok(())
    }

    /// Trades two panes' places, each keeping the other's size.
    pub fn swap_leaves(&mut self, a: &L, b: &L) -> bool
    where
        L: PartialEq,
    {
        a != b && self.swap_leaves_where(&|v| v == a, &|v| v == b)
    }

    pub(super) fn swap_leaves_where(
        &mut self,
        is_a: &impl Fn(&L) -> bool,
        is_b: &impl Fn(&L) -> bool,
    ) -> bool {
        let leaves = self.leaves();
        let Some(i) = leaves.iter().position(is_a) else {
            return false;
        };
        let Some(j) = leaves.iter().position(is_b) else {
            return false;
        };
        self.swap_leaf_indices(i, j)
    }

    fn collect_leaves_mut<'a>(&'a mut self, out: &mut Vec<&'a mut L>) {
        match self {
            Pane::Leaf(v) => out.push(v),
            Pane::Split { a, b, .. } => {
                a.collect_leaves_mut(out);
                b.collect_leaves_mut(out);
            }
            Pane::Empty => {}
        }
    }

    pub fn swap_leaf_indices(&mut self, i: usize, j: usize) -> bool {
        if i == j {
            return false;
        }
        let mut refs: Vec<&mut L> = Vec::new();
        self.collect_leaves_mut(&mut refs);
        let (lo, hi) = (i.min(j), i.max(j));
        if hi >= refs.len() {
            return false;
        }
        let (left, right) = refs.split_at_mut(hi);
        std::mem::swap(&mut *left[lo], &mut *right[0]);
        true
    }

    pub fn leaf_rects(&self) -> Vec<(L, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            &mut out,
        );
        out
    }

    fn collect_rects(&self, area: Rect, out: &mut Vec<(L, Rect)>) {
        match self {
            Pane::Leaf(v) => out.push((v.clone(), area)),
            Pane::Split {
                axis, a, b, ratio, ..
            } => {
                let r = ratio.get().clamp(MIN_RATIO, MAX_RATIO);
                match axis {
                    Axis::Horizontal => {
                        let aw = area.w * r;
                        a.collect_rects(Rect { w: aw, ..area }, out);
                        b.collect_rects(
                            Rect {
                                x: area.x + aw,
                                w: area.w - aw,
                                ..area
                            },
                            out,
                        );
                    }
                    Axis::Vertical => {
                        let ah = area.h * r;
                        a.collect_rects(Rect { h: ah, ..area }, out);
                        b.collect_rects(
                            Rect {
                                y: area.y + ah,
                                h: area.h - ah,
                                ..area
                            },
                            out,
                        );
                    }
                }
            }
            Pane::Empty => {}
        }
    }

    pub fn neighbor_in_direction(&self, from: usize, dir: Dir) -> Option<usize> {
        let rects = self.leaf_rects();
        Self::ranked_neighbor(&rects, from, dir).map(|(i, _)| i)
    }

    /// The pane a move in `dir` lands on and how far off it sits: nearest wins,
    /// and the widest shared edge breaks a tie.
    fn ranked_neighbor(rects: &[(L, Rect)], from: usize, dir: Dir) -> Option<(usize, f32)> {
        let f = rects.get(from)?.1;
        const EPS: f32 = ADJACENCY_EPS;
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, (_, c)) in rects.iter().enumerate() {
            if i == from {
                continue;
            }
            let Some((dist, overlap)) = adjacency(f, *c, dir) else {
                continue;
            };
            let better = match best {
                None => true,
                Some((_, bd, bo)) => dist < bd - EPS || (dist <= bd + EPS && overlap > bo + EPS),
            };
            if better {
                best = Some((i, dist, overlap));
            }
        }
        best.map(|(i, dist, _)| (i, dist))
    }

    /// Whether a move in `dir` could land on `to` without stepping over
    /// anything: `to` shares an edge with that side of `from`, and nothing in
    /// that direction sits nearer.
    ///
    /// Lying on the right side is not enough on its own. In a row of three
    /// columns the far one also sits to the right of the first with a full edge
    /// in common, and treating that as adjacent would skip the column between
    /// them.
    pub fn is_adjacent_in_direction(&self, from: usize, to: usize, dir: Dir) -> bool {
        if from == to {
            return false;
        }
        let rects = self.leaf_rects();
        let (Some((_, f)), Some((_, c))) = (rects.get(from), rects.get(to)) else {
            return false;
        };
        let Some((dist, _)) = adjacency(*f, *c, dir) else {
            return false;
        };
        Self::ranked_neighbor(&rects, from, dir)
            .is_some_and(|(_, nearest)| dist <= nearest + ADJACENCY_EPS)
    }

    /// The pane a move in `dir` lands on, preferring `back` — where the last
    /// move the other way started — as long as it is still adjacent.
    ///
    /// Among the panes actually next to `from`, geometry can only rank by
    /// shared edge, so at a T-junction (one tall pane facing a stack) the
    /// reverse move lands on the same member of the stack whichever one you
    /// left, and going back and forth drifts (#738). Preferring where you came
    /// from settles that tie the only way the user can mean it.
    ///
    /// It settles a tie and nothing more: `back` still has to be one of the
    /// nearest panes that way, so a move can never step over the pane in
    /// between, however out of date the caller's memory is.
    pub fn focus_target_in_direction(
        &self,
        from: usize,
        dir: Dir,
        back: Option<usize>,
    ) -> Option<usize> {
        back.filter(|&back| self.is_adjacent_in_direction(from, back, dir))
            .or_else(|| self.neighbor_in_direction(from, dir))
    }

    pub fn resize_focused(&self, is_focused: &impl Fn(&L) -> bool, dir: Dir, step: f32) -> bool {
        let mut path: Vec<(&Pane<L>, bool)> = Vec::new();
        if !self.focus_path(is_focused, &mut path) {
            return false;
        }
        let target_axis = dir.axis();
        for (node, went_a) in path.iter().rev() {
            if let Pane::Split { axis, ratio, .. } = node {
                if *axis == target_axis {
                    let delta = if *went_a == dir.grows() { step } else { -step };
                    let r = (ratio.get() + delta).clamp(MIN_RATIO, MAX_RATIO);
                    ratio.set(r);
                    return true;
                }
            }
        }
        false
    }

    fn focus_path<'a>(
        &'a self,
        is_focused: &impl Fn(&L) -> bool,
        path: &mut Vec<(&'a Pane<L>, bool)>,
    ) -> bool {
        match self {
            Pane::Leaf(v) => is_focused(v),
            Pane::Split { a, b, .. } => {
                path.push((self, true));
                if a.focus_path(is_focused, path) {
                    return true;
                }
                path.pop();
                path.push((self, false));
                if b.focus_path(is_focused, path) {
                    return true;
                }
                path.pop();
                false
            }
            Pane::Empty => false,
        }
    }
}
