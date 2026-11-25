use std::collections::HashMap;

use smallvec::SmallVec;

use super::{Edge, GraphRow, Lane, MAX_LANES, Oid};

/// The append-only lane assigner the rows come out of.
///
/// Fed `(sha, parents)` newest-first, it produces exactly one [`GraphRow`] per
/// commit and keeps only the state that has to survive a page boundary: which
/// oid each lane is holding a place for, and which lanes are holding a place
/// for a given oid.
///
/// Append-only is the point. A later page extends the graph without touching a
/// row already on screen, which is only possible because a row says nothing
/// about the rows below it — see [`Edge`].
#[derive(Default)]
pub struct LaneAlloc {
    /// Per lane, the oid that lane is currently waiting for. `None` is free.
    slots: Vec<Option<Oid>>,
    /// The reverse index. A `SmallVec` because one child is the common case
    /// and the second entry is the other side of a merge; more than two
    /// children of one commit is rare enough to spill.
    pending: HashMap<Oid, SmallVec<[Lane; 2]>>,
    truncated: bool,
}

impl LaneAlloc {
    pub fn new() -> LaneAlloc {
        LaneAlloc::default()
    }

    /// Lays out one page of commits, newest first. `--topo-order` is what makes
    /// a single forward pass enough: it guarantees a parent is listed after
    /// every one of its children, so by the time a commit is reached, every
    /// lane that wants it already exists.
    pub fn push(&mut self, page: &[(Oid, SmallVec<[Oid; 2]>)], out: &mut Vec<GraphRow>) {
        out.reserve(page.len());
        for (sha, parents) in page {
            let row = self.row(sha, parents);
            out.push(row);
        }
    }

    /// Lanes still held open past the last row laid out — the page boundary.
    /// The renderer draws these as stubs so the bottom of a page does not read
    /// as a row of root commits.
    pub fn open_lanes(&self) -> Vec<Lane> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.is_some())
            .map(|(lane, _)| lane as Lane)
            .collect()
    }

    /// How many columns are live right now, i.e. one past the rightmost lane in
    /// use. Lanes are never compacted, so this only shrinks when the rightmost
    /// lane itself dies.
    pub fn width(&self) -> Lane {
        self.slots
            .iter()
            .rposition(|slot| slot.is_some())
            .map_or(0, |lane| lane as Lane + 1)
    }

    /// Whether history was wider than [`MAX_LANES`] at some point, so lines
    /// were forced to share the last lane. Sticky: once true it stays true.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    fn row(&mut self, sha: &Oid, parents: &[Oid]) -> GraphRow {
        // Sorted so the lines entering this row are emitted left to right and
        // the node lands on the leftmost of them — mainline hugs the left.
        let mut waiting = self.pending.remove(sha).unwrap_or_default();
        waiting.sort_unstable();
        waiting.dedup();
        let node = match waiting.first() {
            Some(lane) => *lane,
            // Nothing is waiting: no child of this commit is inside the window,
            // so it is a tip and starts a lane of its own.
            None => self.alloc_lane(&[]),
        };

        let mut edges: SmallVec<[Edge; 4]> = SmallVec::new();
        for (lane, slot) in self.slots.iter().enumerate() {
            let lane = lane as Lane;
            if slot.is_some() && !waiting.contains(&lane) {
                edges.push(Edge::Pass { lane, color: lane });
            }
        }
        for &lane in &waiting {
            edges.push(Edge::In {
                from: lane,
                color: lane,
            });
            // Release the lane so a parent can claim it below — but only if it
            // really is this commit's. Past MAX_LANES two commits can share the
            // overflow lane, and clearing it there would strand the other one.
            if self.slots[lane as usize].as_deref() == Some(sha.as_str()) {
                self.slots[lane as usize] = None;
            }
        }

        // Lanes this row just gave up, plus the node's own. A second parent
        // that landed on one of them would draw `lane 2 → node → lane 2`, a V
        // that reads as one line bending rather than two lines meeting.
        let mut avoid = waiting;
        if !avoid.contains(&node) {
            avoid.push(node);
        }

        let mut claimed: SmallVec<[Lane; 4]> = SmallVec::new();
        for (k, parent) in parents.iter().enumerate() {
            let lane = if k == 0 {
                // The first parent inherits the node's lane in place, never
                // migrating. That is what keeps a branch on one lane — and so
                // in one colour — from its tip down to wherever it was merged.
                node
            } else if let Some(lane) = self
                .pending
                .get(parent)
                .and_then(|lanes| lanes.iter().copied().min())
            {
                // Some other child already reserved a lane for this parent.
                // Join it instead of opening a second lane to the same commit:
                // two lanes converging on one dot is what the merge row itself
                // is for, and lanes are the scarce resource.
                lane
            } else {
                self.alloc_lane(&avoid)
            };
            if claimed.contains(&lane) {
                continue;
            }
            claimed.push(lane);
            if self.slots[lane as usize].as_deref() != Some(parent.as_str()) {
                self.slots[lane as usize] = Some(parent.clone());
                self.pending.entry(parent.clone()).or_default().push(lane);
            }
            edges.push(Edge::Out {
                to: lane,
                color: lane,
            });
        }
        // No parents: a root. `slots[node]` was released above and nothing
        // claimed it, so the lane simply ends here.

        // Stable: two edges of one rank (a merge's several `Out`s) keep their
        // insertion order — first parent first — which the golden tests pin.
        edges.sort_by_key(Edge::paint_rank);
        GraphRow {
            node,
            // Colour is the lane number, fixed when the lane is created and
            // never reassigned. Not a per-branch counter (what the Git Graph
            // extension does): with a palette of N, branch 0 and branch N come
            // out the same colour, and in a 3-column panel those two are very
            // likely adjacent. Keying on the lane makes neighbouring columns
            // maximally distinct by construction, and the "one branch, one
            // colour" property falls out of the first parent inheriting in
            // place.
            color: node,
            parents: parents.len().min(u8::MAX as usize) as u8,
            edges,
        }
    }

    /// The lowest free lane, avoiding the given ones if that is possible
    /// without widening the graph past [`MAX_LANES`].
    fn alloc_lane(&mut self, avoid: &[Lane]) -> Lane {
        let free = self
            .slots
            .iter()
            .enumerate()
            .find(|(lane, slot)| slot.is_none() && !avoid.contains(&(*lane as Lane)))
            .map(|(lane, _)| lane as Lane);
        if let Some(lane) = free {
            return lane;
        }
        if self.slots.len() < MAX_LANES as usize {
            self.slots.push(None);
            return (self.slots.len() - 1) as Lane;
        }
        // At the cap the avoidance is dropped rather than honoured: a lane is
        // expensive in a 216px panel and a V-shaped kink is only ugly.
        if let Some(lane) = self.slots.iter().position(Option::is_none) {
            return lane as Lane;
        }
        // Genuinely out of lanes. Everything past here shares the last one,
        // which the caller reports so the renderer can say the graph is
        // incomplete rather than quietly drawing a lie.
        self.truncated = true;
        MAX_LANES - 1
    }
}

pub(super) fn row_span(row: &GraphRow) -> Lane {
    let mut span = row.node + 1;
    for edge in &row.edges {
        let lane = match *edge {
            Edge::Pass { lane, .. } => lane,
            Edge::In { from, .. } => from,
            Edge::Out { to, .. } => to,
        };
        span = span.max(lane + 1);
    }
    span
}
