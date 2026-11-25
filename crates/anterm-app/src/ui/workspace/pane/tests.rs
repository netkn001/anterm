use gpui::Axis;

use super::tree::{MAX_RATIO, MIN_RATIO, adjacency};
use super::{CloseOutcome, Dir, Pane, Rect};

type TestPane = Pane<u32>;

fn is(id: u32) -> impl Fn(&u32) -> bool {
    move |v| *v == id
}

fn assert_well_formed(pane: &TestPane) {
    match pane {
        Pane::Leaf(_) => {}
        Pane::Split { a, b, ratio, .. } => {
            let r = ratio.get();
            assert!(
                (MIN_RATIO..=MAX_RATIO).contains(&r),
                "split ratio {r} escaped the legal band"
            );
            assert!(!matches!(**a, Pane::Empty), "split kept an Empty `a` child");
            assert!(!matches!(**b, Pane::Empty), "split kept an Empty `b` child");
            assert_well_formed(a);
            assert_well_formed(b);
        }
        Pane::Empty => panic!("Empty node left in a live tree"),
    }
}

fn split(pane: &mut TestPane, target: u32, axis: Axis, new: u32) {
    assert!(
        pane.split_leaf_where(&is(target), axis, false, new),
        "split target {target} not found"
    );
}

#[test]
fn split_leaf_replaces_target_with_split_keeping_original_first() {
    let mut pane = TestPane::leaf(0);
    assert!(pane.split_leaf_where(&is(0), Axis::Horizontal, false, 1));
    match &pane {
        Pane::Split {
            axis, a, b, ratio, ..
        } => {
            assert!(matches!(axis, Axis::Horizontal));
            assert_eq!(ratio.get(), 0.5);
            assert!(matches!(**a, Pane::Leaf(0)));
            assert!(matches!(**b, Pane::Leaf(1)));
        }
        _ => panic!("split_leaf should replace the leaf with a Split node"),
    }
    assert_well_formed(&pane);
}

#[test]
fn split_leaf_before_puts_the_new_pane_first() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(pane.split_leaf_where(&is(1), Axis::Horizontal, true, 2));
    assert_eq!(pane.leaves(), vec![0, 2, 1]);
    match &pane {
        Pane::Split { a, b, .. } => {
            assert!(matches!(**a, Pane::Leaf(0)), "sibling must not move");
            match &**b {
                Pane::Split { a, b, .. } => {
                    assert!(matches!(**a, Pane::Leaf(2)));
                    assert!(matches!(**b, Pane::Leaf(1)));
                }
                _ => panic!("targeted leaf should have become a nested split"),
            }
        }
        _ => panic!("root should still be the original horizontal split"),
    }
    assert_well_formed(&pane);
}

#[test]
fn split_leaf_splits_only_the_matching_leaf() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Vertical, 2);

    match &pane {
        Pane::Split { axis, a, b, .. } => {
            assert!(matches!(axis, Axis::Horizontal));
            assert!(
                matches!(**a, Pane::Leaf(0)),
                "untargeted leaf must stay a leaf"
            );
            match &**b {
                Pane::Split { axis, a, b, .. } => {
                    assert!(matches!(axis, Axis::Vertical));
                    assert!(matches!(**a, Pane::Leaf(1)));
                    assert!(matches!(**b, Pane::Leaf(2)));
                }
                _ => panic!("targeted leaf should have become a nested split"),
            }
        }
        _ => panic!("root should still be the original horizontal split"),
    }
    assert_well_formed(&pane);
}

#[test]
fn split_leaf_reports_missing_target_without_changing_tree() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(!pane.split_leaf_where(&is(99), Axis::Vertical, false, 2));
    assert_eq!(pane.leaves(), vec![0, 1]);
    assert_well_formed(&pane);
}

#[test]
fn split_node_clamps_restored_ratio_into_legal_band() {
    for (given, expected) in [
        (0.0, MIN_RATIO),
        (-1.0, MIN_RATIO),
        (1.0, MAX_RATIO),
        (7.5, MAX_RATIO),
        (0.3, 0.3),
    ] {
        let node = TestPane::split_node(Axis::Vertical, given, Pane::Leaf(1), Pane::Leaf(2));
        match &node {
            Pane::Split { ratio, .. } => assert_eq!(ratio.get(), expected),
            _ => unreachable!(),
        }
    }
}

#[test]
fn leaves_and_first_leaf_follow_depth_first_a_before_b_order() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Vertical, 2);
    split(&mut pane, 0, Axis::Vertical, 3);
    assert_eq!(pane.leaves(), vec![0, 3, 1, 2]);
    assert_eq!(pane.first_leaf(), Some(0));
}

#[test]
fn leaf_matching_or_first_prefers_the_match_then_falls_back_to_first() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Vertical, 2);
    assert_eq!(pane.leaves(), vec![0, 1, 2]);

    assert_eq!(pane.leaf_matching_or_first(is(2)), Some(2));
    assert_eq!(pane.leaf_matching_or_first(is(1)), Some(1));
    assert_eq!(pane.leaf_matching_or_first(is(99)), Some(0));
    assert_eq!(TestPane::Empty.leaf_matching_or_first(is(0)), None);
}

#[test]
fn a_zoom_is_only_worth_marking_while_it_covers_something() {
    let mut pane = TestPane::leaf(0);
    assert!(
        !pane.zoom_hides_siblings(is(0)),
        "zooming the only pane covers nothing"
    );

    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(pane.zoom_hides_siblings(is(0)));
    assert!(pane.zoom_hides_siblings(is(1)));
    assert!(
        !pane.zoom_hides_siblings(is(99)),
        "a zoom whose pane has exited is not a zoom"
    );
    assert!(!TestPane::Empty.zoom_hides_siblings(is(0)));
}

#[test]
fn closing_the_root_leaf_defers_removal_to_the_caller() {
    let mut pane = TestPane::leaf(7);
    assert!(matches!(
        pane.close_leaf_where(&is(7)),
        CloseOutcome::RemoveSelf
    ));
    assert!(matches!(pane, Pane::Leaf(7)));
}

#[test]
fn closing_first_child_promotes_second_child_to_root() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(matches!(
        pane.close_leaf_where(&is(0)),
        CloseOutcome::Collapsed
    ));
    assert!(matches!(pane, Pane::Leaf(1)));
}

#[test]
fn closing_second_child_promotes_first_child_to_root() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(matches!(
        pane.close_leaf_where(&is(1)),
        CloseOutcome::Collapsed
    ));
    assert!(matches!(pane, Pane::Leaf(0)));
}

#[test]
fn closing_nested_leaf_collapses_only_its_parent_split() {
    let mut pane = TestPane::split_node(
        Axis::Horizontal,
        0.3,
        Pane::Leaf(1),
        Pane::split_node(Axis::Vertical, 0.7, Pane::Leaf(2), Pane::Leaf(3)),
    );
    assert!(matches!(
        pane.close_leaf_where(&is(2)),
        CloseOutcome::Collapsed
    ));
    match &pane {
        Pane::Split {
            axis, a, b, ratio, ..
        } => {
            assert!(matches!(axis, Axis::Horizontal));
            assert_eq!(
                ratio.get(),
                0.3,
                "outer split ratio must survive the collapse"
            );
            assert!(matches!(**a, Pane::Leaf(1)));
            assert!(matches!(**b, Pane::Leaf(3)));
        }
        _ => panic!("outer split must survive an inner collapse"),
    }
    assert_well_formed(&pane);
}

#[test]
fn closing_a_leaf_promotes_entire_sibling_subtree() {
    let mut pane = TestPane::split_node(
        Axis::Horizontal,
        0.5,
        Pane::split_node(Axis::Vertical, 0.7, Pane::Leaf(1), Pane::Leaf(2)),
        Pane::Leaf(3),
    );
    assert!(matches!(
        pane.close_leaf_where(&is(3)),
        CloseOutcome::Collapsed
    ));
    match &pane {
        Pane::Split {
            axis, a, b, ratio, ..
        } => {
            assert!(matches!(axis, Axis::Vertical));
            assert_eq!(ratio.get(), 0.7, "promoted subtree must keep its own ratio");
            assert!(matches!(**a, Pane::Leaf(1)));
            assert!(matches!(**b, Pane::Leaf(2)));
        }
        _ => panic!("sibling subtree should have been promoted to the root"),
    }
    assert_well_formed(&pane);
}

#[test]
fn close_reports_not_found_and_leaves_tree_untouched() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(matches!(
        pane.close_leaf_where(&is(99)),
        CloseOutcome::NotFound
    ));
    assert_eq!(pane.leaves(), vec![0, 1]);
    assert_well_formed(&pane);
}

#[test]
fn close_removes_only_first_match_in_traversal_order() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Vertical, 2);
    assert!(matches!(
        pane.close_leaf_where(&|_| true),
        CloseOutcome::Collapsed
    ));
    assert_eq!(pane.leaves(), vec![1, 2]);
    assert_well_formed(&pane);
}

#[test]
fn deep_split_close_sequence_preserves_invariants_and_leaf_order() {
    enum Op {
        Split(u32, Axis, u32),
        Close(u32),
    }
    use Op::*;
    let script = [
        Split(0, Axis::Horizontal, 1),
        Split(1, Axis::Vertical, 2),
        Split(0, Axis::Vertical, 3),
        Split(2, Axis::Horizontal, 4),
        Split(3, Axis::Horizontal, 5),
        Close(1),
        Close(0),
        Close(4),
        Split(2, Axis::Vertical, 6),
        Close(5),
        Close(3),
        Close(6),
    ];

    let mut pane = TestPane::leaf(0);
    let mut model = vec![0u32];
    for op in script {
        match op {
            Split(target, axis, new) => {
                split(&mut pane, target, axis, new);
                let at = model.iter().position(|&v| v == target).unwrap();
                model.insert(at + 1, new);
            }
            Close(target) => {
                assert!(
                    matches!(pane.close_leaf_where(&is(target)), CloseOutcome::Collapsed),
                    "closing {target} should collapse a split"
                );
                model.retain(|&v| v != target);
            }
        }
        assert_well_formed(&pane);
        assert_eq!(pane.leaves(), model, "tree leaves diverged from the model");
    }
}

#[test]
fn closing_down_to_the_last_pane_hits_remove_self_boundary() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Vertical, 2);
    split(&mut pane, 0, Axis::Vertical, 3);

    while pane.leaves().len() > 1 {
        let target = pane.first_leaf().unwrap();
        assert!(matches!(
            pane.close_leaf_where(&is(target)),
            CloseOutcome::Collapsed
        ));
        assert_well_formed(&pane);
    }

    let last = pane.first_leaf().unwrap();
    assert!(matches!(
        pane.close_leaf_where(&is(last)),
        CloseOutcome::RemoveSelf
    ));
    assert!(
        matches!(pane, Pane::Leaf(_)),
        "last pane is dropped by the caller, not the tree"
    );
}

#[test]
fn empty_placeholder_ignores_all_operations() {
    let mut pane: TestPane = Pane::Empty;
    assert!(pane.leaves().is_empty());
    assert_eq!(pane.first_leaf(), None);
    assert!(!pane.split_leaf_where(&is(0), Axis::Horizontal, false, 1));
    assert!(matches!(
        pane.close_leaf_where(&is(0)),
        CloseOutcome::NotFound
    ));
    assert!(matches!(pane, Pane::Empty));
}

fn rect_of(pane: &TestPane, id: u32) -> Rect {
    pane.leaf_rects()
        .into_iter()
        .find(|(v, _)| *v == id)
        .map(|(_, r)| r)
        .unwrap()
}

fn assert_rect(got: Rect, want: Rect) {
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(
        close(got.x, want.x)
            && close(got.y, want.y)
            && close(got.w, want.w)
            && close(got.h, want.h),
        "rect {got:?} != {want:?}"
    );
}

#[test]
fn leaf_rects_tile_the_unit_square_with_nested_ratios() {
    let pane = TestPane::split_node(
        Axis::Horizontal,
        0.25,
        Pane::Leaf(0),
        TestPane::split_node(Axis::Vertical, 0.6, Pane::Leaf(1), Pane::Leaf(2)),
    );
    assert_rect(
        rect_of(&pane, 0),
        Rect {
            x: 0.0,
            y: 0.0,
            w: 0.25,
            h: 1.0,
        },
    );
    assert_rect(
        rect_of(&pane, 1),
        Rect {
            x: 0.25,
            y: 0.0,
            w: 0.75,
            h: 0.6,
        },
    );
    assert_rect(
        rect_of(&pane, 2),
        Rect {
            x: 0.25,
            y: 0.6,
            w: 0.75,
            h: 0.4,
        },
    );
    assert_eq!(
        pane.leaf_rects()
            .iter()
            .map(|(v, _)| *v)
            .collect::<Vec<_>>(),
        pane.leaves()
    );
}

#[test]
fn neighbor_in_direction_finds_the_adjacent_pane() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    assert_eq!(pane.neighbor_in_direction(idx(0), Dir::Right), Some(idx(1)));
    assert_eq!(pane.neighbor_in_direction(idx(1), Dir::Left), Some(idx(0)));
    assert_eq!(pane.neighbor_in_direction(idx(0), Dir::Up), None);
    assert_eq!(pane.neighbor_in_direction(idx(1), Dir::Right), None);
}

#[test]
fn neighbor_in_direction_prefers_the_largest_overlap() {
    let pane = TestPane::split_node(
        Axis::Horizontal,
        0.5,
        Pane::Leaf(0),
        TestPane::split_node(Axis::Vertical, 0.7, Pane::Leaf(1), Pane::Leaf(2)),
    );
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    assert_eq!(pane.neighbor_in_direction(idx(0), Dir::Right), Some(idx(1)));
}

/// The layout from #738: one full-height pane facing a stack of two.
fn t_junction() -> TestPane {
    TestPane::split_node(
        Axis::Horizontal,
        0.5,
        Pane::Leaf(0),
        TestPane::split_node(Axis::Vertical, 0.5, Pane::Leaf(4), Pane::Leaf(6)),
    )
}

#[test]
fn reversing_a_move_at_a_t_junction_returns_to_where_it_started() {
    let pane = t_junction();
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    assert_eq!(
        pane.focus_target_in_direction(idx(6), Dir::Left, None),
        Some(idx(0))
    );
    // Geometry alone ties on overlap here and hands back the top pane.
    assert_eq!(pane.neighbor_in_direction(idx(0), Dir::Right), Some(idx(4)));
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(idx(6))),
        Some(idx(6))
    );
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(idx(4))),
        Some(idx(4))
    );
}

#[test]
fn a_recorded_pane_the_layout_moved_on_from_falls_back_to_geometry() {
    let pane = t_junction();
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    // Gone entirely: the caller maps a missing pane to no index at all, and
    // an index the tree no longer has must not resolve to whoever took it.
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, None),
        Some(idx(4))
    );
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(99)),
        Some(idx(4))
    );
    // Still there, but no longer reachable that way: splitting the tall
    // pane leaves its top half facing only the top of the stack.
    let mut pane = t_junction();
    split(&mut pane, 0, Axis::Vertical, 1);
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    assert!(!pane.is_adjacent_in_direction(idx(0), idx(6), Dir::Right));
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(idx(6))),
        Some(idx(4))
    );
    // And the direction still has to match: the pane below is never the
    // answer to a move right, however recently focus came from there.
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(idx(1))),
        Some(idx(4))
    );
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(idx(0))),
        Some(idx(4))
    );
}

/// Two steps out and two back is the same walk the reverse of a single move
/// is, and it has to end where it started for the same reason. Replays the
/// bookkeeping `focus_pane_dir` does — each move recorded against the pane
/// it landed on — over the layout that breaks it: three columns whose last
/// one is a stack, so the second move back is the one geometry cannot call.
#[test]
fn a_walk_of_two_steps_comes_back_to_the_pane_it_started_from() {
    // 0 | 1 | (2 over 3).
    let pane = TestPane::split_node(
        Axis::Horizontal,
        1.0 / 3.0,
        Pane::Leaf(0),
        TestPane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(1),
            TestPane::split_node(Axis::Vertical, 0.5, Pane::Leaf(2), Pane::Leaf(3)),
        ),
    );
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    // Geometry alone ties on overlap out of the middle column and answers
    // with the top of the stack, whichever member the walk set off from.
    assert_eq!(pane.neighbor_in_direction(idx(1), Dir::Right), Some(idx(2)));

    let mut origin: std::collections::HashMap<(usize, Dir), usize> =
        std::collections::HashMap::new();
    let mut at = idx(3);
    for dir in [Dir::Left, Dir::Left, Dir::Right, Dir::Right] {
        let back = origin.get(&(at, dir)).copied();
        let to = pane
            .focus_target_in_direction(at, dir, back)
            .expect("a neighbour that way");
        origin.insert((to, dir.opposite()), at);
        at = to;
    }
    assert_eq!(
        at,
        idx(3),
        "the second move back must return to the bottom of the stack too"
    );
}

#[test]
fn a_remembered_pane_further_off_never_steps_over_the_one_between() {
    // Three equal full-height columns, 0 | 1 | 2.
    let pane = TestPane::split_node(
        Axis::Horizontal,
        1.0 / 3.0,
        Pane::Leaf(0),
        TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(1), Pane::Leaf(2)),
    );
    let idx = |id: u32| pane.leaves().iter().position(|v| *v == id).unwrap();
    // Focus reaches 1 by moving left off 2, then leaves for 0 by a click or
    // a cycle — neither of which records anything, so the origin still
    // names 2 when the move back to the right happens from 0.
    assert!(pane.is_adjacent_in_direction(idx(1), idx(2), Dir::Right));
    assert!(!pane.is_adjacent_in_direction(idx(0), idx(2), Dir::Right));
    // 2 does lie to the right of 0 with a full edge in common; only being
    // farther off than 1 disqualifies it.
    assert!(adjacency(rect_of(&pane, 0), rect_of(&pane, 2), Dir::Right).is_some());
    assert_eq!(
        pane.focus_target_in_direction(idx(0), Dir::Right, Some(idx(2))),
        Some(idx(1)),
        "a move right must land on the next column, not skip it"
    );
}

#[test]
fn resize_grows_the_focused_pane_from_either_side() {
    let build = || TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(0), Pane::Leaf(1));
    let ratio = |p: &TestPane| match p {
        Pane::Split { ratio, .. } => ratio.get(),
        _ => unreachable!(),
    };
    let p = build();
    assert!(p.resize_focused(&is(0), Dir::Right, 0.05));
    assert!((ratio(&p) - 0.55).abs() < 1e-6);
    let p = build();
    assert!(p.resize_focused(&is(1), Dir::Right, 0.05));
    assert!((ratio(&p) - 0.45).abs() < 1e-6);
    let p = build();
    assert!(p.resize_focused(&is(0), Dir::Left, 0.05));
    assert!((ratio(&p) - 0.45).abs() < 1e-6);
}

#[test]
fn resize_without_a_matching_axis_is_a_noop() {
    let pane = TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(0), Pane::Leaf(1));
    assert!(!pane.resize_focused(&is(0), Dir::Up, 0.05));
    assert!(!pane.resize_focused(&is(0), Dir::Down, 0.05));
    assert!(!pane.resize_focused(&is(99), Dir::Right, 0.05));
}

#[test]
fn resize_targets_the_nearest_matching_axis_ancestor() {
    let pane = TestPane::split_node(
        Axis::Horizontal,
        0.5,
        Pane::Leaf(0),
        TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(1), Pane::Leaf(2)),
    );
    assert!(pane.resize_focused(&is(1), Dir::Right, 0.05));
    match &pane {
        Pane::Split { ratio, b, .. } => {
            assert!(
                (ratio.get() - 0.5).abs() < 1e-6,
                "outer split must not move"
            );
            match &**b {
                Pane::Split { ratio, .. } => {
                    assert!(
                        (ratio.get() - 0.55).abs() < 1e-6,
                        "inner split should grow 1"
                    );
                }
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

fn grid() -> TestPane {
    // 0 1
    // 2 3
    TestPane::split_node(
        Axis::Vertical,
        0.5,
        TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(0), Pane::Leaf(1)),
        TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(2), Pane::Leaf(3)),
    )
}

fn moved(pane: &mut TestPane, src: u32, dst: u32, dir: Dir) -> bool {
    pane.move_leaf_where(&is(src), &is(dst), dir)
}

#[test]
fn moving_a_pane_splits_the_destination_on_the_named_side() {
    let mut pane = grid();
    assert!(moved(&mut pane, 0, 3, Dir::Down));
    assert_eq!(pane.leaves(), vec![1, 2, 3, 0]);
    match &pane {
        Pane::Split { a, b, .. } => {
            assert!(matches!(**a, Pane::Leaf(1)), "1 was promoted by the lift");
            match &**b {
                Pane::Split { a, b, .. } => {
                    assert!(matches!(**a, Pane::Leaf(2)));
                    match &**b {
                        Pane::Split { axis, a, b, .. } => {
                            assert!(matches!(axis, Axis::Vertical));
                            assert!(matches!(**a, Pane::Leaf(3)));
                            assert!(matches!(**b, Pane::Leaf(0)), "0 landed below 3");
                        }
                        _ => panic!("3 should have become a split"),
                    }
                }
                _ => panic!("the right column should have survived"),
            }
        }
        _ => panic!("the root should still be a split"),
    }
    assert_well_formed(&pane);
}

#[test]
fn moving_a_pane_before_the_destination_puts_it_first() {
    let mut pane = grid();
    assert!(moved(&mut pane, 3, 0, Dir::Left));
    assert_eq!(pane.leaves(), vec![3, 0, 1, 2]);
    assert_well_formed(&pane);
}

/// A row of two over a third pane, so there is always a run to join and a
/// pane outside it to drag in.
fn row_over() -> TestPane {
    TestPane::split_node(
        Axis::Vertical,
        0.5,
        TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(0), Pane::Leaf(1)),
        Pane::Leaf(2),
    )
}

fn widths(pane: &TestPane) -> Vec<f32> {
    pane.leaf_rects()
        .iter()
        .map(|(_, r)| (r.w * 1000.).round() / 1000.)
        .collect()
}

#[test]
fn joining_a_row_takes_an_equal_share_of_it_instead_of_halving_a_neighbour() {
    let mut pane = row_over();
    assert!(moved(&mut pane, 2, 0, Dir::Right));
    assert_eq!(pane.leaves(), vec![0, 2, 1]);
    assert_eq!(
        widths(&pane),
        vec![0.333, 0.333, 0.333],
        "three columns, not one of them quartered"
    );
    assert_well_formed(&pane);
}

#[test]
fn joining_a_row_takes_its_share_off_the_others_in_proportion() {
    let mut pane = TestPane::split_node(
        Axis::Vertical,
        0.5,
        TestPane::split_node(Axis::Horizontal, 0.75, Pane::Leaf(0), Pane::Leaf(1)),
        Pane::Leaf(2),
    );
    assert!(moved(&mut pane, 2, 1, Dir::Right));
    assert_eq!(pane.leaves(), vec![0, 1, 2]);
    assert_eq!(
        widths(&pane),
        vec![0.5, 0.167, 0.333],
        "the newcomer takes a third; the other two stay three to one"
    );
    assert_well_formed(&pane);
}

#[test]
fn a_drop_across_the_run_still_halves_the_pane_it_landed_on() {
    let mut pane = row_over();
    assert!(moved(&mut pane, 2, 0, Dir::Down));
    assert_eq!(pane.leaves(), vec![0, 2, 1]);
    assert_eq!(
        widths(&pane),
        vec![0.5, 0.5, 0.5],
        "0 and 2 share 0's column, which keeps its width"
    );
    match &pane {
        Pane::Split { a, .. } => match &**a {
            Pane::Split { axis, ratio, .. } => {
                assert!(matches!(axis, Axis::Vertical));
                assert_eq!(ratio.get(), 0.5, "there is no row to share out here");
            }
            _ => panic!("0 should have become a column of two"),
        },
        _ => unreachable!(),
    }
    assert_well_formed(&pane);
}

#[test]
fn a_move_that_would_redraw_the_same_layout_is_refused() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(
        !moved(&mut pane, 1, 0, Dir::Right),
        "1 is already right of 0"
    );
    assert!(!moved(&mut pane, 0, 1, Dir::Left), "0 is already left of 1");
    assert_eq!(pane.leaves(), vec![0, 1]);

    assert!(
        moved(&mut pane, 1, 0, Dir::Down),
        "the same neighbours on a new axis is a real move"
    );
    assert_eq!(pane.leaves(), vec![0, 1]);
    assert!(matches!(
        &pane,
        Pane::Split {
            axis: Axis::Vertical,
            ..
        }
    ));
    assert_well_formed(&pane);
}

#[test]
fn a_move_onto_a_missing_or_only_pane_changes_nothing() {
    let mut pane = TestPane::leaf(0);
    assert!(!moved(&mut pane, 0, 0, Dir::Right), "nowhere else to go");
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(!moved(&mut pane, 99, 0, Dir::Right));
    assert!(!moved(&mut pane, 0, 99, Dir::Right));
    assert_eq!(pane.leaves(), vec![0, 1]);
    assert_well_formed(&pane);
}

#[test]
fn moving_to_an_edge_makes_a_band_beside_everything_else() {
    let mut pane = grid();
    assert!(pane.move_leaf_to_edge_where(&is(1), Dir::Right));
    assert_eq!(pane.leaves(), vec![0, 2, 3, 1]);
    match &pane {
        Pane::Split { axis, a, b, .. } => {
            assert!(
                matches!(axis, Axis::Horizontal),
                "the band sits beside the rest, not above it"
            );
            assert!(matches!(**b, Pane::Leaf(1)), "1 is the whole right band");
            assert_eq!(a.leaves(), vec![0, 2, 3]);
        }
        _ => panic!("the root should be the new split"),
    }
    assert_well_formed(&pane);
}

#[test]
fn a_band_takes_one_share_of_the_bands_the_axis_ends_up_with() {
    // What the band ended up taking, read off the split the landing put it
    // in — the same number the drop lands, rather than one carried out of
    // the tree alongside it for the test's benefit.
    let share = |pane: &TestPane, id: u32, dir: Dir| {
        pane.edge_landing(&is(id), dir).map(|landed| match landed {
            Pane::Split { ratio, .. } => {
                let taken = if dir.leads() {
                    ratio.get()
                } else {
                    1. - ratio.get()
                };
                (taken * 1000.).round() / 1000.
            }
            _ => panic!("an edge landing is always a split"),
        })
    };

    // Two columns receive a third column, not a half.
    let mut two = TestPane::leaf(0);
    split(&mut two, 0, Axis::Horizontal, 1);
    split(&mut two, 1, Axis::Horizontal, 2);
    assert_eq!(share(&two, 2, Dir::Right), Some(0.333));

    // A 2×2 reads as two columns even though no one node cuts it in two,
    // and lifting a pane out of it leaves those two columns standing.
    assert_eq!(grid().slices_along(Axis::Horizontal), 2);
    assert_eq!(share(&grid(), 1, Dir::Right), Some(0.333));
    assert_eq!(share(&grid(), 1, Dir::Up), Some(0.333));

    // Two rows have one column between them, so a column is a half.
    let mut rows = TestPane::leaf(0);
    split(&mut rows, 0, Axis::Vertical, 1);
    assert_eq!(share(&rows, 1, Dir::Left), Some(0.5));

    assert_eq!(
        share(&TestPane::leaf(0), 0, Dir::Left),
        None,
        "the only pane has nowhere to go, so there is no share to draw"
    );
}

#[test]
fn the_band_a_move_lands_is_the_share_it_advertised() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Horizontal, 2);
    assert!(pane.move_leaf_to_edge_where(&is(2), Dir::Right));
    match &pane {
        Pane::Split { ratio, b, .. } => {
            assert!(matches!(**b, Pane::Leaf(2)));
            assert!(
                (ratio.get() - 2. / 3.).abs() < 1e-6,
                "the rest keeps two thirds, the new column takes one"
            );
        }
        _ => unreachable!(),
    }

    let mut leading = TestPane::leaf(0);
    split(&mut leading, 0, Axis::Horizontal, 1);
    split(&mut leading, 1, Axis::Horizontal, 2);
    assert!(leading.move_leaf_to_edge_where(&is(2), Dir::Left));
    match &leading {
        Pane::Split { ratio, a, .. } => {
            assert!(matches!(**a, Pane::Leaf(2)));
            assert!((ratio.get() - 1. / 3.).abs() < 1e-6);
        }
        _ => unreachable!(),
    }
}

#[test]
fn an_edge_move_that_changes_nothing_is_refused() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    assert!(!pane.move_leaf_to_edge_where(&is(1), Dir::Right));
    assert!(!pane.move_leaf_to_edge_where(&is(0), Dir::Left));
    assert!(!TestPane::leaf(7).move_leaf_to_edge_where(&is(7), Dir::Up));
    assert!(pane.move_leaf_to_edge_where(&is(1), Dir::Up));
    assert_eq!(pane.leaves(), vec![1, 0]);
    assert_well_formed(&pane);
}

/// A drop zone is read off the rectangles and carried out against the
/// leaves, so the two have to be the same panes in the same order.
#[test]
fn leaf_rects_come_back_in_the_order_the_leaves_do() {
    let pane = TestPane::split_node(
        Axis::Horizontal,
        0.25,
        TestPane::split_node(Axis::Vertical, 0.5, Pane::Leaf(0), Pane::Leaf(1)),
        TestPane::split_node(
            Axis::Horizontal,
            0.5,
            Pane::Leaf(2),
            TestPane::split_node(Axis::Vertical, 0.5, Pane::Leaf(3), Pane::Leaf(4)),
        ),
    );
    let ordered: Vec<u32> = pane.leaf_rects().into_iter().map(|(v, _)| v).collect();
    assert_eq!(ordered, pane.leaves());
}

#[test]
fn swapping_two_leaves_trades_their_places_by_identity() {
    let mut pane = grid();
    assert!(pane.swap_leaves_where(&is(0), &is(3)));
    assert_eq!(pane.leaves(), vec![3, 1, 2, 0]);
    assert!(!pane.swap_leaves_where(&is(0), &is(99)));
    assert_well_formed(&pane);
}

/// The tab arriving from somewhere else: two panes, side by side.
fn newcomer() -> TestPane {
    TestPane::split_node(Axis::Horizontal, 0.5, Pane::Leaf(8), Pane::Leaf(9))
}

#[test]
fn a_grafted_tab_splits_the_pane_it_landed_on() {
    let mut pane = TestPane::leaf(0);
    assert!(pane.graft_beside(newcomer(), &0, Dir::Right).is_ok());
    assert_eq!(pane.leaves(), vec![0, 8, 9]);
    match &pane {
        Pane::Split { axis, a, b, .. } => {
            assert!(matches!(axis, Axis::Horizontal));
            assert!(matches!(**a, Pane::Leaf(0)));
            assert_eq!(b.leaves(), vec![8, 9], "the tab kept its own shape");
        }
        _ => panic!("the leaf should have become a split"),
    }
    assert_eq!(widths(&pane), vec![0.5, 0.25, 0.25]);
    assert_well_formed(&pane);
}

#[test]
fn a_grafted_tab_joins_a_row_as_one_of_its_columns() {
    let mut pane = row_over();
    assert!(pane.graft_beside(newcomer(), &0, Dir::Right).is_ok());
    assert_eq!(pane.leaves(), vec![0, 8, 9, 1, 2]);
    assert_eq!(
        widths(&pane),
        vec![0.333, 0.167, 0.167, 0.333, 1.0],
        "the newcomer is one column of three, split between its own two"
    );
    assert_well_formed(&pane);
}

#[test]
fn a_tab_grafted_at_an_edge_is_a_band_beside_everything() {
    let mut pane = grid();
    assert!(pane.graft_at_edge(newcomer(), Dir::Right).is_ok());
    assert_eq!(pane.leaves(), vec![0, 1, 2, 3, 8, 9]);
    match &pane {
        Pane::Split {
            axis, a, b, ratio, ..
        } => {
            assert!(matches!(axis, Axis::Horizontal));
            assert_eq!(a.leaves(), vec![0, 1, 2, 3]);
            assert_eq!(b.leaves(), vec![8, 9]);
            assert!(
                (ratio.get() - 2. / 3.).abs() < 1e-6,
                "two columns receive a third, not a half"
            );
        }
        _ => panic!("an edge graft is always a split at the root"),
    }
    assert_well_formed(&pane);
}

#[test]
fn a_graft_with_nowhere_to_go_hands_the_panes_back() {
    let mut pane = TestPane::leaf(0);
    let refused = pane
        .graft_beside(newcomer(), &99, Dir::Right)
        .expect_err("there is no pane 99 to land beside");
    assert_eq!(refused.leaves(), vec![8, 9], "the tab came back intact");
    assert_eq!(pane.leaves(), vec![0]);

    let empty = pane
        .graft_beside(Pane::Empty, &0, Dir::Right)
        .expect_err("an empty tab has nothing to graft");
    assert!(matches!(empty, Pane::Empty));
    assert_eq!(pane.leaves(), vec![0]);
    assert_well_formed(&pane);
}

#[test]
fn swap_leaf_indices_trades_payloads_in_place() {
    let mut pane = TestPane::leaf(0);
    split(&mut pane, 0, Axis::Horizontal, 1);
    split(&mut pane, 1, Axis::Vertical, 2);
    split(&mut pane, 0, Axis::Vertical, 3);
    assert_eq!(pane.leaves(), vec![0, 3, 1, 2]);
    assert!(pane.swap_leaf_indices(0, 2));
    assert_eq!(pane.leaves(), vec![1, 3, 0, 2]);
    assert_well_formed(&pane);
    assert!(!pane.swap_leaf_indices(1, 1));
    assert!(!pane.swap_leaf_indices(0, 99));
    assert_eq!(pane.leaves(), vec![1, 3, 0, 2]);
}
