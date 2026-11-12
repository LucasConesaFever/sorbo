//! The order in which a window is given its frame (LAY-3, D48).
//!
//! A window shown from its parking spot usually crosses to another display. Moved first,
//! it keeps its old size for a moment and can hang over that display's edge onto the next
//! one; macOS then won't shrink it to the tile, and it stays too tall. Sized first, it
//! already fits when it arrives. The last resize catches an app that adjusted its size
//! on the way.

/// One Accessibility call of a `SetFrame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStep {
    Size,
    Position,
}

/// The calls a `SetFrame` makes, in order.
pub const SET_FRAME_STEPS: [FrameStep; 3] = [FrameStep::Size, FrameStep::Position, FrameStep::Size];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lay_3_the_size_is_set_before_the_window_moves() {
        assert_eq!(
            SET_FRAME_STEPS,
            [FrameStep::Size, FrameStep::Position, FrameStep::Size]
        );
    }
}
