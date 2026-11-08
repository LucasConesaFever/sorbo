//! Layout of the menu bar's workspace row, shared by drawing and click handling so the two
//! can't disagree (spec section 4). Pure: compiles and tests on any OS.

use wm_core::{WorkspaceNumber, WorkspaceState, WorkspaceStatus};

/// Every box has the same width, sized for two digits, so the row doesn't jump around.
pub const BOX_WIDTH: f64 = 24.0;
pub const BOX_GAP: f64 = 4.0;
/// Space before the first box and after the last one.
pub const MARGIN: f64 = 4.0;
pub const BOX_HEIGHT: f64 = 18.0;
pub const CORNER_RADIUS: f64 = 5.0;

/// Width of a row of `count` boxes, margins included. An empty row takes no space.
pub fn row_width(count: usize) -> f64 {
    if count == 0 {
        return 0.0;
    }
    2.0 * MARGIN + count as f64 * BOX_WIDTH + (count - 1) as f64 * BOX_GAP
}

/// Left edge of the box at `index`.
pub fn box_x(index: usize) -> f64 {
    MARGIN + index as f64 * (BOX_WIDTH + BOX_GAP)
}

/// The workspace a click at `x` should go to: the box under it, unless that one is already
/// focused. Gaps and margins hit nothing (BAR-4).
pub fn hit(row: &[WorkspaceStatus], x: f64) -> Option<WorkspaceNumber> {
    row.iter()
        .enumerate()
        .find(|&(i, _)| x >= box_x(i) && x < box_x(i) + BOX_WIDTH)
        .filter(|(_, ws)| ws.state != WorkspaceState::Focused)
        .map(|(_, ws)| ws.number)
}

/// Diameter of the red "config has errors" dot (CFG-6).
pub const DOT_SIZE: f64 = 8.0;

/// Left edge of the dot after `count` boxes: where another box would start.
pub fn dot_x(count: usize) -> f64 {
    box_x(count)
}

/// Width of the row with the dot after the boxes, when `dot`.
pub fn row_width_with_dot(count: usize, dot: bool) -> f64 {
    if dot {
        dot_x(count) + DOT_SIZE + MARGIN
    } else {
        row_width(count)
    }
}

/// How far the boxes and the dot shift right to make room for the mode label (MODE-4),
/// which is drawn at `MARGIN`, before them. Menu bar items keep their right edge and grow
/// leftward, so a label before the boxes leaves every box where it was on screen.
pub fn row_offset(label_width: Option<f64>) -> f64 {
    label_width.map_or(0.0, |width| width + BOX_GAP)
}

/// Width of the row with the dot (when `dot`) and a label `label_width` wide (when shown).
pub fn row_width_with_label(count: usize, dot: bool, label_width: Option<f64>) -> f64 {
    row_width_with_dot(count, dot) + row_offset(label_width)
}

/// True when a click at `x` is on the dot (with the gap before it and the margin after it,
/// so an 8 pt target is easier to hit). Only meaningful while the dot is shown.
pub fn hit_dot(count: usize, x: f64) -> bool {
    let start = dot_x(count);
    x >= start - BOX_GAP && x < start + DOT_SIZE + MARGIN
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_core::WorkspaceNumber;

    fn ws(n: u32) -> WorkspaceNumber {
        WorkspaceNumber::new(n).unwrap()
    }
    use WorkspaceState::*;

    fn row(v: &[(u32, WorkspaceState)]) -> Vec<WorkspaceStatus> {
        v.iter()
            .map(|&(number, state)| WorkspaceStatus {
                number: ws(number),
                state,
            })
            .collect()
    }

    #[test]
    fn mode_4_boxes_keep_their_distance_from_the_right_edge() {
        // Menu bar items keep their right edge and grow leftward: a box stays put on
        // screen when its distance to the item's right edge doesn't change.
        let label = Some(50.);
        for dot in [false, true] {
            let without = row_width_with_dot(3, dot);
            let with = row_width_with_label(3, dot, label);
            for i in 0..3 {
                assert_eq!(with - (row_offset(label) + box_x(i)), without - box_x(i));
            }
            assert_eq!(with - (row_offset(label) + dot_x(3)), without - dot_x(3));
        }
        assert_eq!(row_offset(None), 0.);
        assert_eq!(
            row_width_with_label(3, true, None),
            row_width_with_dot(3, true)
        );
    }

    #[test]
    fn bar_1_row_width() {
        assert_eq!(row_width(0), 0.0);
        assert_eq!(row_width(1), 32.0);
        assert_eq!(row_width(3), 88.0);
    }

    #[test]
    fn bar_1_box_positions() {
        assert_eq!(box_x(0), 4.0);
        assert_eq!(box_x(2), 60.0);
    }

    #[test]
    fn bar_4_click_on_a_box_returns_its_number() {
        let r = row(&[(1, Visible), (3, Hidden), (5, Focused)]);
        assert_eq!(hit(&r, 5.0), Some(ws(1)));
        assert_eq!(hit(&r, 40.0), Some(ws(3)));
    }

    #[test]
    fn bar_4_click_on_focused_does_nothing() {
        let r = row(&[(1, Visible), (3, Hidden), (5, Focused)]);
        assert_eq!(hit(&r, 65.0), None);
    }

    #[test]
    fn bar_4_click_in_gap_or_margin_does_nothing() {
        let r = row(&[(1, Visible), (3, Hidden), (5, Focused)]);
        assert_eq!(hit(&r, 1.0), None);
        assert_eq!(hit(&r, 29.0), None);
        assert_eq!(hit(&r, 200.0), None);
    }

    #[test]
    fn cfg_6_dot_adds_width_and_is_clickable() {
        assert_eq!(row_width_with_dot(3, false), 88.0);
        // The dot goes where a fourth box would start: 4 + 8 + 4 margin.
        assert_eq!(dot_x(3), 88.0);
        assert_eq!(row_width_with_dot(3, true), 100.0);
        assert_eq!(row_width_with_dot(0, true), 16.0);
        assert!(hit_dot(3, 90.0));
        assert!(hit_dot(3, 85.0));
        assert!(!hit_dot(3, 80.0));
        assert!(!hit_dot(3, 100.0));
    }

    #[test]
    fn bar_4_two_digit_numbers_use_the_same_box() {
        let r = row(&[(10, Hidden), (11, Hidden), (12, Focused)]);
        assert_eq!(hit(&r, 33.0), Some(ws(11)));
        assert_eq!(row_width(3), 88.0);
    }
}
