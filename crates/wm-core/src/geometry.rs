//! Points and rectangles in global macOS points: origin top-left of the main display, y down.

/// Two positions closer than this (on both axes) count as the same position.
pub const POSITION_TOLERANCE: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    /// True if both coordinates are within `POSITION_TOLERANCE` of `other`.
    pub fn near(&self, other: Point) -> bool {
        (self.x - other.x).abs() <= POSITION_TOLERANCE
            && (self.y - other.y).abs() <= POSITION_TOLERANCE
    }
}

/// Direction of a split. `Horizontal` means the two children sit side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Half-open: the left and top edges are inside, the right and bottom edges are not.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.width && p.y >= self.y && p.y < self.y + self.height
    }

    /// Moves every edge inward by `by`.
    pub fn shrink(&self, by: f64) -> Rect {
        Rect::new(
            self.x + by,
            self.y + by,
            self.width - 2.0 * by,
            self.height - 2.0 * by,
        )
    }

    /// Splits into (first, second). `ratio` is the share given to `first`.
    pub fn split(&self, axis: Axis, ratio: f64) -> (Rect, Rect) {
        match axis {
            Axis::Horizontal => {
                let first_width = self.width * ratio;
                (
                    Rect::new(self.x, self.y, first_width, self.height),
                    Rect::new(
                        self.x + first_width,
                        self.y,
                        self.width - first_width,
                        self.height,
                    ),
                )
            }
            Axis::Vertical => {
                let first_height = self.height * ratio;
                (
                    Rect::new(self.x, self.y, self.width, first_height),
                    Rect::new(
                        self.x,
                        self.y + first_height,
                        self.width,
                        self.height - first_height,
                    ),
                )
            }
        }
    }

    pub fn translate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// Moves (never resizes) the rect so it lies inside `bounds` where it fits.
    /// If it is larger than `bounds` on an axis, it is aligned to the top/left edge.
    pub fn clamp_inside(&self, bounds: &Rect) -> Rect {
        let max_x = bounds.x + bounds.width - self.width;
        let max_y = bounds.y + bounds.height - self.height;
        let x = self.x.min(max_x).max(bounds.x);
        let y = self.y.min(max_y).max(bounds.y);
        Rect::new(x, y, self.width, self.height)
    }

    /// A frame core can work with (SAFE-1): all four numbers finite, a positive size. Apps
    /// report their own frames, and an infinite or NaN one would make `contains` true
    /// everywhere or `near` false forever.
    pub fn is_usable(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.
            && self.height > 0.
    }

    /// This rect with its size limited to `bounds`' size; the origin is unchanged (SAFE-2).
    pub fn capped_to(&self, bounds: &Rect) -> Rect {
        Rect::new(
            self.x,
            self.y,
            self.width.min(bounds.width),
            self.height.min(bounds.height),
        )
    }

    /// True if every field is within `POSITION_TOLERANCE` of `other`'s. macOS rounds window
    /// frames to whole points, so a reported frame this close counts as the same frame.
    pub fn near(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() <= POSITION_TOLERANCE
            && (self.y - other.y).abs() <= POSITION_TOLERANCE
            && (self.width - other.width).abs() <= POSITION_TOLERANCE
            && (self.height - other.height).abs() <= POSITION_TOLERANCE
    }

    pub fn is_wider_than_tall(&self) -> bool {
        self.width > self.height
    }
}

/// A frame known to be usable (SAFE-1). The only way to make one is `UsableRect::new`,
/// which checks, so code that takes a `UsableRect` can't forget the check (HARD-7). Apps
/// report their own frames; wm-macos also only makes one from a frame the window server
/// confirmed (SAFE-3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsableRect(Rect);

impl UsableRect {
    /// `Some` only if `rect` is usable.
    pub fn new(rect: Rect) -> Option<UsableRect> {
        rect.is_usable().then_some(UsableRect(rect))
    }

    pub fn rect(self) -> Rect {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_1_usable_frames() {
        assert!(Rect::new(0., 0., 10., 10.).is_usable());
        assert!(!Rect::new(0., 0., f64::INFINITY, 10.).is_usable());
        assert!(!Rect::new(f64::NAN, 0., 10., 10.).is_usable());
        assert!(!Rect::new(0., 0., 0., 10.).is_usable());
        assert!(!Rect::new(0., 0., 10., -1.).is_usable());
    }

    #[test]
    fn hard_7_usable_rect_only_from_usable_frames() {
        let ok = Rect::new(0., 0., 10., 10.);
        assert_eq!(UsableRect::new(ok).map(UsableRect::rect), Some(ok));
        assert_eq!(UsableRect::new(Rect::new(0., 0., f64::INFINITY, 10.)), None);
        assert_eq!(UsableRect::new(Rect::new(f64::NAN, 0., 10., 10.)), None);
        assert_eq!(UsableRect::new(Rect::new(0., 0., 0., 10.)), None);
    }

    #[test]
    fn rect_center() {
        assert_eq!(Rect::new(0., 0., 100., 50.).center(), Point::new(50., 25.));
    }

    #[test]
    fn rect_split_horizontal_halves() {
        assert_eq!(
            Rect::new(3., 3., 994., 794.).split(Axis::Horizontal, 0.5),
            (
                Rect::new(3., 3., 497., 794.),
                Rect::new(500., 3., 497., 794.)
            )
        );
    }

    #[test]
    fn rect_split_vertical_by_ratio() {
        assert_eq!(
            Rect::new(0., 0., 100., 200.).split(Axis::Vertical, 0.25),
            (Rect::new(0., 0., 100., 50.), Rect::new(0., 50., 100., 150.))
        );
    }

    #[test]
    fn rect_shrink() {
        assert_eq!(
            Rect::new(0., 0., 100., 100.).shrink(2.),
            Rect::new(2., 2., 96., 96.)
        );
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0., 0., 100., 50.);
        assert!(r.contains(Point::new(0., 0.)));
        assert!(!r.contains(Point::new(100., 10.)));
    }

    #[test]
    fn point_near_uses_tolerance() {
        assert!(Point::new(10., 10.).near(Point::new(10.9, 9.2)));
        assert!(!Point::new(10., 10.).near(Point::new(11.5, 10.)));
    }

    #[test]
    fn rect_clamp_inside_moves_without_resizing() {
        assert_eq!(
            Rect::new(1900., 100., 300., 200.).clamp_inside(&Rect::new(1000., 0., 1000., 800.)),
            Rect::new(1700., 100., 300., 200.)
        );
    }
}
