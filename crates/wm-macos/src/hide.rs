//! Where to park a hidden window, and where to put it back on exit (spec 5.4 and 7).
//!
//! macOS lets a window go off the left or right edge with 1 px visible, but keeps about
//! 32 px on screen at the bottom and refuses the top (measured with `spike_hide`).

use std::time::Duration;

use wm_core::{Point, Rect};

/// Width left visible when a window is parked off the left or right edge.
pub const SIDE_VISIBLE: f64 = 1.0;
/// Height macOS keeps on screen when a window is pushed off the bottom edge.
pub const BOTTOM_VISIBLE: f64 = 32.0;

/// How long to wait before trying again a hide that failed (HARD-3): 0.5 s, then 1, 2 and
/// 4 s, then every 5 s. A hung app answers each Accessibility call only after its 50 ms
/// timeout, so the gaps grow to keep a hung window cheap.
pub fn hide_retry_after(attempts_done: u8) -> Duration {
    match attempts_done {
        0 => Duration::from_millis(500),
        1 => Duration::from_secs(1),
        2 => Duration::from_secs(2),
        3 => Duration::from_secs(4),
        _ => Duration::from_secs(5),
    }
}

/// The size used to pick a hidden window's parking corner (HARD-3): the frame just read if
/// it's usable, otherwise the frame sorbo last set, otherwise the last confirmed one. A
/// busy app may not answer the read, and only the size matters for the corner.
pub fn parking_size(
    read: Option<Rect>,
    last_set: Option<Rect>,
    confirmed: Option<Rect>,
) -> Option<(f64, f64)> {
    [read, last_set, confirmed]
        .into_iter()
        .flatten()
        .find(Rect::is_usable)
        .map(|r| (r.width, r.height))
}

/// The position to request for a hidden window of `width` x `height`.
///
/// `displays` are full display bounds (top-left origin); `own_display` is the index of the
/// display the window was on. Candidates are both bottom corners of every display. A
/// candidate is only allowed if the parked window doesn't sit horizontally over another
/// display (macOS would pull it onto that display). Among allowed candidates the one that
/// overlaps the screens least wins, preferring the window's own display, then the earliest.
pub fn hide_position(width: f64, height: f64, own_display: usize, displays: &[Rect]) -> Point {
    // (allowed, cost, is_own_display, requested position)
    let mut candidates = Vec::new();
    for (i, d) in displays.iter().enumerate() {
        let bottom = d.y + d.height;
        let right = Point::new(d.x + d.width - SIDE_VISIBLE, bottom - 1.0);
        let left = Point::new(d.x - width + SIDE_VISIBLE, bottom - 1.0);
        for request in [right, left] {
            // macOS clamps y so that BOTTOM_VISIBLE points stay on this display.
            let parked = Rect::new(request.x, bottom - BOTTOM_VISIBLE, width, height);
            let cost: f64 = displays.iter().map(|other| overlap(parked, *other)).sum();
            let allowed = displays
                .iter()
                .enumerate()
                .all(|(j, other)| j == i || x_overlap(parked, *other) <= 0.0);
            candidates.push((allowed, cost, i == own_display, request));
        }
    }
    // Prefer allowed candidates; if none is allowed, consider all of them.
    let any_allowed = candidates.iter().any(|c| c.0);
    let mut best: Option<(f64, bool, Point)> = None;
    for (allowed, cost, own, request) in candidates {
        if any_allowed && !allowed {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_cost, best_own, _)) => {
                cost < best_cost || (cost == best_cost && own && !best_own)
            }
        };
        if better {
            best = Some((cost, own, request));
        }
    }
    // With no displays at all there's nowhere better than the origin.
    best.map(|(_, _, p)| p).unwrap_or(Point::new(0.0, 0.0))
}

/// The frame a hidden window gets when the WM exits: centered on `display`, no larger than it.
pub fn restore_frame(width: f64, height: f64, display: Rect) -> Rect {
    let w = width.min(display.width);
    let h = height.min(display.height);
    Rect::new(
        display.x + (display.width - w) / 2.0,
        display.y + (display.height - h) / 2.0,
        w,
        h,
    )
}

/// How much two rectangles overlap horizontally, ignoring y (0 or less if they don't).
fn x_overlap(a: Rect, b: Rect) -> f64 {
    (a.x + a.width).min(b.x + b.width) - a.x.max(b.x)
}

/// Area shared by two rectangles (0 if they don't touch).
fn overlap(a: Rect, b: Rect) -> f64 {
    let w = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let h = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    w.max(0.0) * h.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_3_hide_retry_backoff() {
        let s = |n| hide_retry_after(n).as_millis();
        assert_eq!(
            [s(0), s(1), s(2), s(3), s(4), s(9)],
            [500, 1000, 2000, 4000, 5000, 5000]
        );
    }

    #[test]
    fn hard_3_parking_size_falls_back() {
        let a = Rect::new(0., 0., 300., 200.);
        let b = Rect::new(0., 0., 400., 100.);
        assert_eq!(parking_size(Some(a), Some(b), None), Some((300., 200.)));
        assert_eq!(parking_size(None, Some(b), Some(a)), Some((400., 100.)));
        assert_eq!(
            parking_size(Some(Rect::new(0., 0., f64::NAN, 1.)), None, Some(a)),
            Some((300., 200.))
        );
        assert_eq!(parking_size(None, None, None), None);
    }

    const WIN: (f64, f64) = (586.0, 488.0);

    fn home() -> Vec<Rect> {
        vec![
            Rect::new(0.0, 0.0, 1728.0, 1117.0),
            Rect::new(1728.0, 0.0, 1920.0, 1080.0),
        ]
    }

    #[test]
    fn hide_single_display_uses_own_corner() {
        let d = vec![Rect::new(0.0, 0.0, 1728.0, 1117.0)];
        assert_eq!(
            hide_position(WIN.0, WIN.1, 0, &d),
            Point::new(1727.0, 1116.0)
        );
    }

    #[test]
    fn hide_home_builtin_uses_free_side() {
        // Bottom-right would sit under the external display's x-range: macOS pulls such a
        // window onto the external. Bottom-left is free.
        assert_eq!(
            hide_position(WIN.0, WIN.1, 0, &home()),
            Point::new(-585.0, 1116.0)
        );
    }

    #[test]
    fn hide_never_parks_under_another_displays_x_range() {
        // Measured: a 723x1050 System Settings window parked at (1727, 1116) ended up fully
        // visible on the external display.
        let p = hide_position(723.0, 1050.0, 0, &home());
        assert_eq!(p, Point::new(-722.0, 1116.0));
    }

    #[test]
    fn hide_home_external_avoids_builtin() {
        assert_eq!(
            hide_position(WIN.0, WIN.1, 1, &home()),
            Point::new(3647.0, 1079.0)
        );
    }

    #[test]
    fn hide_office_layout_uses_free_corner() {
        // Built-in (main) below two externals placed side by side above it.
        let d = vec![
            Rect::new(0.0, 0.0, 1728.0, 1117.0),
            Rect::new(-1056.0, -1080.0, 1920.0, 1080.0),
            Rect::new(864.0, -1080.0, 1920.0, 1080.0),
        ];
        assert_eq!(
            hide_position(WIN.0, WIN.1, 1, &d),
            Point::new(-1641.0, -1.0)
        );
    }

    #[test]
    fn hide_middle_of_three_uses_outer_corner() {
        let d = vec![
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            Rect::new(1000.0, 0.0, 1000.0, 800.0),
            Rect::new(2000.0, 0.0, 1000.0, 800.0),
        ];
        assert_eq!(
            hide_position(400.0, 300.0, 1, &d),
            Point::new(-399.0, 799.0)
        );
    }

    #[test]
    fn restore_centers_on_display() {
        assert_eq!(
            restore_frame(586.0, 488.0, Rect::new(0.0, 0.0, 1728.0, 1117.0)),
            Rect::new(571.0, 314.5, 586.0, 488.0)
        );
    }

    #[test]
    fn restore_caps_size_at_display() {
        assert_eq!(
            restore_frame(3000.0, 2000.0, Rect::new(0.0, 0.0, 1728.0, 1117.0)),
            Rect::new(0.0, 0.0, 1728.0, 1117.0)
        );
    }
}
