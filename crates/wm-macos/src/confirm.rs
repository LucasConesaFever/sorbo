//! Confirming the frames apps report against the window server (SAFE-3). An app answers
//! Accessibility queries about its own windows and can claim any frame; the window server's
//! list of windows (owner pid and bounds) is something it can't fake. A frame is trusted only
//! when that app has a window there.

use std::time::Duration;

use wm_core::{Rect, UsableRect};

/// Points by which an AX frame and window-server bounds may differ on every edge.
pub const TOLERANCE: f64 = 2.0;
/// How long to wait before checking an unconfirmed frame again: a window can be announced
/// through Accessibility before the window server lists it.
pub const RECHECK_AFTER: Duration = Duration::from_millis(300);
/// How many times an unconfirmed frame is checked again.
pub const RECHECKS: u8 = 3;

/// True if `a` and `b` are within `TOLERANCE` of each other on every edge.
pub fn near(a: &Rect, b: &Rect) -> bool {
    (a.x - b.x).abs() <= TOLERANCE
        && (a.y - b.y).abs() <= TOLERANCE
        && (a.width - b.width).abs() <= TOLERANCE
        && (a.height - b.height).abs() <= TOLERANCE
}

/// True if `server` (that app's window-server bounds) has a window near `ax` that isn't
/// already backing another of the app's tracked windows: `claimed_by_others` of them are
/// known to be there. Each real window backs at most one claim, so an app can't point a
/// second window at the area of one it already has.
pub fn confirmed(ax: Rect, server: &[Rect], claimed_by_others: usize) -> bool {
    server.iter().filter(|b| near(&ax, b)).count() > claimed_by_others
}

/// The frame core may use: `ax` only when the window server confirms it (SAFE-3) and it is
/// usable (SAFE-1). The only place wm-macos turns an app's frame into a `UsableRect` for a
/// managed window, so no other path can hand core an unchecked frame (HARD-7).
pub fn confirmed_frame(ax: Rect, server: &[Rect], claimed_by_others: usize) -> Option<UsableRect> {
    if confirmed(ax, server, claimed_by_others) {
        UsableRect::new(ax)
    } else {
        None
    }
}

/// When to check again after `attempts_done` re-checks; `None` once they're used up.
pub fn next_recheck(attempts_done: u8) -> Option<Duration> {
    (attempts_done < RECHECKS).then_some(RECHECK_AFTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn safe_3_frame_within_2_points_is_confirmed() {
        assert!(confirmed(
            r(100., 100., 400., 300.),
            &[r(101., 98., 401., 302.)],
            0
        ));
    }

    #[test]
    fn safe_3_fake_frame_is_not_confirmed() {
        assert!(!confirmed(
            r(0., 33., 1728., 1084.),
            &[r(800., 500., 10., 10.)],
            0
        ));
    }

    #[test]
    fn safe_3_no_server_window_is_not_confirmed() {
        assert!(!confirmed(r(100., 100., 400., 300.), &[], 0));
    }

    #[test]
    fn safe_3_a_window_backs_only_one_claim() {
        let b = r(5., 5., 990., 790.);
        // The app's real window B is already another tracked window's frame: a second window
        // claiming B's area isn't confirmed (it would borrow B's place for FOC-9).
        assert!(!confirmed(b, &[b], 1));
        // Two real windows there can back two claims.
        assert!(confirmed(b, &[b, b], 1));
        assert!(near(&b, &r(6., 4., 991., 789.)));
        assert!(!near(&b, &r(8., 5., 990., 790.)));
    }

    #[test]
    fn hard_7_only_a_confirmed_usable_frame_becomes_a_usable_rect() {
        let f = r(5., 5., 10., 10.);
        assert_eq!(confirmed_frame(f, &[f], 0).map(UsableRect::rect), Some(f));
        assert_eq!(confirmed_frame(f, &[r(500., 5., 10., 10.)], 0), None);
        assert_eq!(confirmed_frame(f, &[f], 1), None);
        let huge = r(0., 0., f64::INFINITY, 10.);
        assert_eq!(confirmed_frame(huge, &[huge], 0), None);
    }

    #[test]
    fn safe_3_unconfirmed_frame_is_rechecked() {
        assert_eq!(next_recheck(0), Some(Duration::from_millis(300)));
        assert_eq!(next_recheck(2), Some(Duration::from_millis(300)));
        assert_eq!(next_recheck(3), None);
    }
}
