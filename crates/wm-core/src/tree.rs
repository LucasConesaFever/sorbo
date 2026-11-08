//! The dwindle layout tree (LAY-1, LAY-2): a binary tree whose leaves are windows.
//!
//! A tree is an `Option<Node>`: `None` means the workspace has no tiled windows.

use crate::{Axis, Direction, Rect, ResizeStep, WindowId};

/// How much one resize keypress moves a split.
pub const RESIZE_STEP: f64 = 0.05;
pub const MIN_RATIO: f64 = 0.1;
pub const MAX_RATIO: f64 = 0.9;
/// Space around tiled windows, in points (CFG-1, CFG-4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gaps {
    /// Between a window and the monitor edge.
    pub outer: f64,
    /// Between two adjacent windows.
    pub inner: f64,
}

impl Gaps {
    pub const DEFAULT: Gaps = Gaps {
        outer: 5.0,
        inner: 4.0,
    };
}

impl Default for Gaps {
    fn default() -> Gaps {
        Gaps::DEFAULT
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Window(WindowId),
    /// `Box` gives the recursive type a known size: a `Split` holds two more `Node`s.
    Split(Box<Split>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    /// Fixed when the split is created (Hyprland `preserve_split`); only ToggleSplit changes it.
    pub axis: Axis,
    /// Share of the space given to `first`, in [MIN_RATIO, MAX_RATIO].
    pub ratio: f64,
    /// Left or top.
    pub first: Node,
    /// Right or bottom.
    pub second: Node,
}

impl Node {
    pub fn contains(&self, w: WindowId) -> bool {
        match self {
            Node::Window(id) => *id == w,
            Node::Split(s) => s.first.contains(w) || s.second.contains(w),
        }
    }

    /// All windows, first subtree before second.
    pub fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.collect_windows(&mut out);
        out
    }

    fn collect_windows(&self, out: &mut Vec<WindowId>) {
        match self {
            Node::Window(id) => out.push(*id),
            Node::Split(s) => {
                s.first.collect_windows(out);
                s.second.collect_windows(out);
            }
        }
    }

    pub fn first_window(&self) -> WindowId {
        match self {
            Node::Window(id) => *id,
            Node::Split(s) => s.first.first_window(),
        }
    }

    /// The leaf node holding `w`, if any.
    fn leaf_mut(&mut self, w: WindowId) -> Option<&mut Node> {
        if *self == Node::Window(w) {
            return Some(self);
        }
        match self {
            Node::Window(_) => None,
            Node::Split(s) => {
                if s.first.contains(w) {
                    s.first.leaf_mut(w)
                } else {
                    s.second.leaf_mut(w)
                }
            }
        }
    }

    /// The split whose direct child is the leaf `w`, and whether `w` is its `first` child.
    fn parent_of_mut(&mut self, w: WindowId) -> Option<(&mut Split, bool)> {
        let Node::Split(s) = self else {
            return None;
        };
        if s.first == Node::Window(w) {
            return Some((&mut **s, true));
        }
        if s.second == Node::Window(w) {
            return Some((&mut **s, false));
        }
        if s.first.contains(w) {
            s.first.parent_of_mut(w)
        } else {
            s.second.parent_of_mut(w)
        }
    }

    /// True if a split with `axis` lies between this node and the leaf `w` (this node
    /// included).
    fn has_split_on_axis(&self, w: WindowId, axis: Axis) -> bool {
        match self {
            Node::Window(_) => false,
            Node::Split(s) => {
                self.contains(w)
                    && (s.axis == axis
                        || s.first.has_split_on_axis(w, axis)
                        || s.second.has_split_on_axis(w, axis))
            }
        }
    }

    /// The split with `axis` closest above the leaf `w` (MODE-2).
    fn nearest_split_on_axis_mut(&mut self, w: WindowId, axis: Axis) -> Option<&mut Split> {
        let Node::Split(s) = self else {
            return None;
        };
        // Decide with shared borrows first, then take one mutable borrow per branch: a
        // mutable borrow of a child held across the `&mut **s` below wouldn't compile.
        let in_first = s.first.contains(w);
        if !in_first && !s.second.contains(w) {
            return None;
        }
        let deeper = if in_first {
            s.first.has_split_on_axis(w, axis)
        } else {
            s.second.has_split_on_axis(w, axis)
        };
        match (deeper, in_first) {
            (true, true) => s.first.nearest_split_on_axis_mut(w, axis),
            (true, false) => s.second.nearest_split_on_axis_mut(w, axis),
            (false, _) => (s.axis == axis).then_some(&mut **s),
        }
    }

    /// Replaces the split that directly holds leaf `w` with `w`'s sibling subtree.
    /// Returns the first window of that sibling. `w` must be in this subtree and not be `self`.
    fn remove_leaf(&mut self, w: WindowId) -> Option<WindowId> {
        let Node::Split(s) = self else {
            return None;
        };
        let keep_first = if s.second == Node::Window(w) {
            true
        } else if s.first == Node::Window(w) {
            false
        } else if s.first.contains(w) {
            return s.first.remove_leaf(w);
        } else {
            return s.second.remove_leaf(w);
        };

        // Take ownership of the split by swapping a throwaway leaf into its place,
        // then put the kept sibling where the split was.
        let Node::Split(split) = std::mem::replace(self, Node::Window(w)) else {
            unreachable!("self was a split a moment ago");
        };
        let split = *split; // move the Split out of its Box
        let sibling = if keep_first {
            split.first
        } else {
            split.second
        };
        let first = sibling.first_window();
        *self = sibling;
        Some(first)
    }

    fn for_each_window_mut(&mut self, f: &mut impl FnMut(&mut WindowId)) {
        match self {
            Node::Window(id) => f(id),
            Node::Split(s) => {
                s.first.for_each_window_mut(f);
                s.second.for_each_window_mut(f);
            }
        }
    }
}

/// Inserts `new` next to `target` (LAY-1). Falls back to the first window when `target`
/// is `None` or not in the tree. The split axis follows the target's longer side.
pub fn insert(tree: &mut Option<Node>, target: Option<WindowId>, new: WindowId, area: Rect) {
    let Some(root) = tree.as_mut() else {
        *tree = Some(Node::Window(new));
        return;
    };
    let target = match target {
        Some(t) if root.contains(t) => t,
        _ => root.first_window(),
    };
    // The default gaps are close enough to pick the target's longer side.
    let target_rect = layout(tree, area, Gaps::DEFAULT)
        .into_iter()
        .find(|(id, _)| *id == target)
        .map(|(_, rect)| rect)
        .unwrap_or(area);
    let Some(root) = tree.as_mut() else {
        return;
    };
    let axis = if target_rect.is_wider_than_tall() {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    if let Some(leaf) = root.leaf_mut(target) {
        *leaf = Node::Split(Box::new(Split {
            axis,
            ratio: 0.5,
            first: Node::Window(target),
            second: Node::Window(new),
        }));
    }
}

/// Removes `w`; its sibling takes its space (LAY-2). Returns the first window of the
/// subtree that took the space, or `None` if the tree is now empty or `w` wasn't found.
pub fn remove(tree: &mut Option<Node>, w: WindowId) -> Option<WindowId> {
    let root = tree.as_mut()?;
    if *root == Node::Window(w) {
        *tree = None;
        return None;
    }
    if !root.contains(w) {
        return None;
    }
    root.remove_leaf(w)
}

/// Exchanges the positions of `a` and `b`. Does nothing unless both are in the tree.
pub fn swap(tree: &mut Option<Node>, a: WindowId, b: WindowId) {
    let Some(root) = tree.as_mut() else {
        return;
    };
    if !root.contains(a) || !root.contains(b) {
        return;
    }
    root.for_each_window_mut(&mut |id| {
        if *id == a {
            *id = b;
        } else if *id == b {
            *id = a;
        }
    });
}

/// Flips the axis of `w`'s parent split. False if `w` has no parent.
pub fn toggle_split(tree: &mut Option<Node>, w: WindowId) -> bool {
    match tree.as_mut().and_then(|root| root.parent_of_mut(w)) {
        Some((split, _)) => {
            split.axis = match split.axis {
                Axis::Horizontal => Axis::Vertical,
                Axis::Vertical => Axis::Horizontal,
            };
            true
        }
        None => false,
    }
}

/// Grows or shrinks `w`'s side of its parent split. False if `w` has no parent.
pub fn resize(tree: &mut Option<Node>, w: WindowId, step: ResizeStep) -> bool {
    match tree.as_mut().and_then(|root| root.parent_of_mut(w)) {
        Some((split, is_first)) => {
            let mut delta = match step {
                ResizeStep::Grow => RESIZE_STEP,
                ResizeStep::Shrink => -RESIZE_STEP,
            };
            if !is_first {
                delta = -delta;
            }
            split.ratio = (split.ratio + delta).clamp(MIN_RATIO, MAX_RATIO);
            true
        }
        None => false,
    }
}

/// MODE-2: moves the divider closest to `w` on `direction`'s axis one step toward
/// `direction` (Left/Right: side-by-side splits; Up/Down: stacked splits). False if no split
/// on that axis holds `w`.
pub fn resize_toward(tree: &mut Option<Node>, w: WindowId, direction: Direction) -> bool {
    let (axis, delta) = match direction {
        Direction::Left => (Axis::Horizontal, -RESIZE_STEP),
        Direction::Right => (Axis::Horizontal, RESIZE_STEP),
        Direction::Up => (Axis::Vertical, -RESIZE_STEP),
        Direction::Down => (Axis::Vertical, RESIZE_STEP),
    };
    match tree
        .as_mut()
        .and_then(|root| root.nearest_split_on_axis_mut(w, axis))
    {
        Some(split) => {
            split.ratio = (split.ratio + delta).clamp(MIN_RATIO, MAX_RATIO);
            true
        }
        None => false,
    }
}

/// Window rectangles for a tree filling `area`, with gaps, in `windows()` order.
///
/// The tree splits the area inside the outer gap. Each window then gives up half the inner
/// gap on the sides where it faces another window, so windows end up `outer` from the edge
/// and `inner` from each other. Gaps never take more than half a cell, so a window in a
/// cell smaller than the gaps still has a positive size and stays inside the outer gap.
pub fn layout(tree: &Option<Node>, area: Rect, gaps: Gaps) -> Vec<(WindowId, Rect)> {
    let mut out = Vec::new();
    if let Some(root) = tree {
        let usable = area.shrink(gaps.outer);
        layout_node(root, usable, usable, gaps.inner / 2.0, &mut out);
    }
    out
}

fn layout_node(
    node: &Node,
    cell: Rect,
    usable: Rect,
    half_inner: f64,
    out: &mut Vec<(WindowId, Rect)>,
) {
    match node {
        Node::Window(id) => out.push((*id, window_in_cell(cell, usable, half_inner))),
        Node::Split(s) => {
            let (a, b) = cell.split(s.axis, s.ratio);
            layout_node(&s.first, a, usable, half_inner, out);
            layout_node(&s.second, b, usable, half_inner, out);
        }
    }
}

/// `cell` minus `half_inner` on each side that faces another cell (not the edge of `usable`).
fn window_in_cell(cell: Rect, usable: Rect, half_inner: f64) -> Rect {
    // Cells come from splitting `usable`, so an outer side matches its edge up to rounding.
    const EDGE: f64 = 0.5;
    let inset = |faces_another: bool| if faces_another { half_inner } else { 0.0 };
    let (left, right) = at_most_half(
        inset(cell.x > usable.x + EDGE),
        inset(cell.x + cell.width < usable.x + usable.width - EDGE),
        cell.width,
    );
    let (top, bottom) = at_most_half(
        inset(cell.y > usable.y + EDGE),
        inset(cell.y + cell.height < usable.y + usable.height - EDGE),
        cell.height,
    );
    Rect::new(
        cell.x + left,
        cell.y + top,
        cell.width - left - right,
        cell.height - top - bottom,
    )
}

/// Scales two insets down so together they take at most half of `size`.
fn at_most_half(a: f64, b: f64, size: f64) -> (f64, f64) {
    let limit = size / 2.0;
    if a + b > limit {
        let scale = limit / (a + b);
        (a * scale, b * scale)
    } else {
        (a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(n: u64) -> WindowId {
        WindowId(n)
    }

    fn area() -> Rect {
        Rect::new(0., 0., 1000., 800.)
    }

    /// [1,2,3]: 1 left, 2 top-right, 3 bottom-right
    fn three() -> Option<Node> {
        let mut t = None;
        insert(&mut t, None, w(1), area());
        insert(&mut t, Some(w(1)), w(2), area());
        insert(&mut t, Some(w(2)), w(3), area());
        t
    }

    fn two() -> Option<Node> {
        let mut t = None;
        insert(&mut t, None, w(1), area());
        insert(&mut t, Some(w(1)), w(2), area());
        t
    }

    fn root_split(t: &Option<Node>) -> &Split {
        match t {
            Some(Node::Split(s)) => s,
            other => panic!("expected split, got {other:?}"),
        }
    }

    #[test]
    fn lay_1_first_window_becomes_root() {
        let mut t = None;
        insert(&mut t, None, w(1), area());
        assert_eq!(t, Some(Node::Window(w(1))));
    }

    #[test]
    fn lay_1_second_window_splits_along_longer_side() {
        assert_eq!(
            two(),
            Some(Node::Split(Box::new(Split {
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Node::Window(w(1)),
                second: Node::Window(w(2)),
            })))
        );
    }

    #[test]
    fn lay_1_third_window_splits_tall_target_vertically() {
        let t = three();
        match &root_split(&t).second {
            Node::Split(s) => assert_eq!(s.axis, Axis::Vertical),
            n => panic!("{n:?}"),
        }
        assert_eq!(t.as_ref().unwrap().windows(), vec![w(1), w(2), w(3)]);
    }

    #[test]
    fn lay_1_missing_target_uses_first_window() {
        let mut t = None;
        insert(&mut t, None, w(1), area());
        insert(&mut t, Some(w(99)), w(2), area());
        assert_eq!(t.as_ref().unwrap().windows(), vec![w(1), w(2)]);
    }

    #[test]
    fn lay_2_remove_gives_space_to_sibling() {
        let mut t = three();
        assert_eq!(remove(&mut t, w(2)), Some(w(3)));
        let root = root_split(&t);
        assert_eq!(
            (root.axis, &root.first, &root.second),
            (Axis::Horizontal, &Node::Window(w(1)), &Node::Window(w(3)))
        );
    }

    #[test]
    fn lay_2_remove_last_window_empties_tree() {
        let mut t = Some(Node::Window(w(1)));
        assert_eq!(remove(&mut t, w(1)), None);
        assert_eq!(t, None);
    }

    #[test]
    fn remove_unknown_window_leaves_tree_unchanged() {
        let mut t = three();
        assert_eq!(remove(&mut t, w(9)), None);
        assert_eq!(t, three());
    }

    #[test]
    fn swap_exchanges_leaves() {
        let mut t = three();
        swap(&mut t, w(1), w(3));
        assert_eq!(t.as_ref().unwrap().windows(), vec![w(3), w(2), w(1)]);
    }

    #[test]
    fn key_10_toggle_split_flips_parent_axis() {
        let mut t = three();
        assert!(toggle_split(&mut t, w(1)));
        assert_eq!(root_split(&t).axis, Axis::Vertical);
        let mut single = Some(Node::Window(w(1)));
        assert!(!toggle_split(&mut single, w(1)));
    }

    #[test]
    fn key_9_grow_depends_on_side() {
        let mut t = two();
        assert!(resize(&mut t, w(1), ResizeStep::Grow));
        assert!((root_split(&t).ratio - 0.55).abs() < 1e-9);
        assert!(resize(&mut t, w(2), ResizeStep::Grow));
        assert!((root_split(&t).ratio - 0.50).abs() < 1e-9);
    }

    #[test]
    fn key_9_ratio_is_clamped() {
        let mut t = two();
        for _ in 0..20 {
            resize(&mut t, w(1), ResizeStep::Grow);
        }
        assert!((root_split(&t).ratio - MAX_RATIO).abs() < 1e-9);
    }

    fn right_split(t: &Option<Node>) -> &Split {
        match &root_split(t).second {
            Node::Split(s) => s,
            other => panic!("expected split, got {other:?}"),
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn mode_2_right_moves_the_side_by_side_divider_right() {
        let mut t = three();
        assert!(resize_toward(&mut t, w(1), Direction::Right));
        assert!(approx(root_split(&t).ratio, 0.55));
        // The right side's window: the divider still moves right (that window shrinks).
        let mut t = three();
        assert!(resize_toward(&mut t, w(3), Direction::Right));
        assert!(approx(root_split(&t).ratio, 0.55));
    }

    #[test]
    fn mode_2_up_moves_the_nearest_stacked_divider() {
        let mut t = three();
        assert!(resize_toward(&mut t, w(3), Direction::Up));
        assert!(approx(right_split(&t).ratio, 0.45));
        assert!(approx(root_split(&t).ratio, 0.5));
    }

    #[test]
    fn mode_2_no_divider_on_that_axis_does_nothing() {
        let mut t = three();
        let before = t.clone();
        assert!(!resize_toward(&mut t, w(1), Direction::Up));
        assert_eq!(t, before);
        let mut single = Some(Node::Window(w(1)));
        assert!(!resize_toward(&mut single, w(1), Direction::Left));
    }

    #[test]
    fn mode_2_ratio_stays_within_limits() {
        let mut t = three();
        for _ in 0..20 {
            resize_toward(&mut t, w(1), Direction::Right);
        }
        assert!(approx(root_split(&t).ratio, MAX_RATIO));
    }

    #[test]
    fn lay_1_layout_empty_tree() {
        assert_eq!(layout(&None, area(), Gaps::DEFAULT), vec![]);
    }

    #[test]
    fn lay_1_layout_single_window_has_outer_gap() {
        assert_eq!(
            layout(&Some(Node::Window(w(1))), area(), Gaps::DEFAULT),
            vec![(w(1), Rect::new(5., 5., 990., 790.))]
        );
    }

    #[test]
    fn lay_1_layout_three_windows_dwindle() {
        assert_eq!(
            layout(&three(), area(), Gaps::DEFAULT),
            vec![
                (w(1), Rect::new(5., 5., 493., 790.)),
                (w(2), Rect::new(502., 5., 493., 393.)),
                (w(3), Rect::new(502., 402., 493., 393.)),
            ]
        );
    }

    #[test]
    fn lay_1_large_gaps_keep_windows_positive_and_inside_the_outer_gap() {
        // Eight windows dwindled into a small area: the deepest cells are narrower than the
        // inner gap.
        let small = Rect::new(0., 0., 600., 400.);
        let mut t = None;
        insert(&mut t, None, w(1), small);
        for n in 2..=8 {
            insert(&mut t, Some(w(n - 1)), w(n), small);
        }
        let gaps = Gaps {
            outer: 0.,
            inner: 100.,
        };
        for (id, rect) in layout(&t, small, gaps) {
            assert!(rect.width > 0. && rect.height > 0., "{id:?} got {rect:?}");
            assert!(
                rect.x >= 0.
                    && rect.y >= 0.
                    && rect.x + rect.width <= 600.
                    && rect.y + rect.height <= 400.,
                "{id:?} leaves the area: {rect:?}"
            );
        }
    }

    #[test]
    fn lay_1_layout_respects_area_origin() {
        assert_eq!(
            layout(
                &Some(Node::Window(w(1))),
                Rect::new(1000., 0., 1000., 800.),
                Gaps::DEFAULT
            ),
            vec![(w(1), Rect::new(1005., 5., 990., 790.))]
        );
    }
}
