//! Fixed limits on the work one app can make sorbo do (SAFE-4, SAFE-5, HARD-5). Apps control how many
//! windows and menu items they report; without limits a hostile app can freeze the WM, which
//! launchd doesn't restart.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Windows tracked per app; the rest are ignored (SAFE-4).
pub const MAX_WINDOWS_PER_APP: usize = 100;

/// Menu bar items looked at when searching for ⌘N (SAFE-5).
pub const MENU_BAR_ITEMS: usize = 40;
/// Items looked at in each menu (SAFE-5).
pub const MENU_ITEMS: usize = 100;
/// The whole ⌘N search gives up after this long (SAFE-5).
pub const MENU_WALK_TIME: Duration = Duration::from_millis(250);

/// Accessibility time one pointer hit test may take (HARD-5). Each AX call may wait up to
/// its 50 ms timeout, and the walk up the element tree makes up to ~30 of them.
pub const POINTER_HIT_TIME: Duration = Duration::from_millis(30);
/// How long an app that overran a hit test is skipped (HARD-5).
pub const SLOW_APP_COOLDOWN: Duration = Duration::from_secs(2);

/// Whether one more window of an app that already has `tracked_for_app` tracked may be
/// tracked.
pub fn may_track(tracked_for_app: usize) -> bool {
    tracked_for_app < MAX_WINDOWS_PER_APP
}

/// A time budget for one walk over an app's elements (its menus, or the tree under the
/// pointer).
pub struct Budget {
    deadline: Instant,
}

impl Budget {
    /// A budget of `limit` from `now`.
    pub fn new(now: Instant, limit: Duration) -> Budget {
        Budget {
            deadline: now + limit,
        }
    }

    /// A budget of `MENU_WALK_TIME` from `now`.
    pub fn starting(now: Instant) -> Budget {
        Budget::new(now, MENU_WALK_TIME)
    }

    /// Whether the budget is used up at `now`.
    pub fn spent(&self, now: Instant) -> bool {
        now >= self.deadline
    }
}

/// Apps that recently made a pointer hit test overrun its budget, and until when they're
/// skipped (HARD-5).
#[derive(Default)]
pub struct SlowApps {
    until: HashMap<i32, Instant>,
}

impl SlowApps {
    /// App `pid` overran at `now`: skip it for `SLOW_APP_COOLDOWN`.
    pub fn mark(&mut self, pid: i32, now: Instant) {
        self.until.insert(pid, now + SLOW_APP_COOLDOWN);
    }

    /// Whether app `pid` is still being skipped at `now`. Expired entries are kept until the
    /// app is marked again or forgotten; there's one per app at most.
    pub fn is_cooling(&self, pid: i32, now: Instant) -> bool {
        self.until.get(&pid).is_some_and(|until| now < *until)
    }

    /// App `pid` quit.
    pub fn forget(&mut self, pid: i32) {
        self.until.remove(&pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_5_budget_has_its_own_limit() {
        let start = Instant::now();
        let budget = Budget::new(start, POINTER_HIT_TIME);
        assert_eq!(POINTER_HIT_TIME, Duration::from_millis(30));
        assert!(!budget.spent(start + Duration::from_millis(29)));
        assert!(budget.spent(start + Duration::from_millis(30)));
    }

    #[test]
    fn hard_5_slow_app_is_skipped_for_2_s() {
        let t0 = Instant::now();
        let mut slow = SlowApps::default();
        assert!(!slow.is_cooling(7, t0));
        slow.mark(7, t0);
        assert!(slow.is_cooling(7, t0 + Duration::from_millis(1999)));
        assert!(!slow.is_cooling(7, t0 + SLOW_APP_COOLDOWN));
        assert!(!slow.is_cooling(8, t0));
    }

    #[test]
    fn safe_4_tracks_up_to_100_windows_per_app() {
        assert!(may_track(0));
        assert!(may_track(99));
        assert!(!may_track(100));
    }

    #[test]
    fn safe_5_menu_walk_budget_is_250_ms() {
        let start = Instant::now();
        let budget = Budget::starting(start);
        assert!(!budget.spent(start + Duration::from_millis(249)));
        assert!(budget.spent(start + Duration::from_millis(250)));
    }
}
