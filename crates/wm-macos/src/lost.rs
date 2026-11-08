//! Windows macOS rebuilds during a display change (MON-7, D30). Pure.

//!
//! When a monitor drops out for a moment (screen wake, unlock), macOS can invalidate every
//! window that was on it and show it again later with a new AX element. Reported as
//! "destroyed" and "created", those windows would lose their workspace. Instead, a window
//! destroyed shortly after a display change is kept here, and a window of the same app that
//! shows up later takes its place (same `WindowId`, same placement).

use std::time::{Duration, Instant};

use wm_core::{Rect, WindowId, WindowKind};

/// A window destroyed this soon after a display change may be rebuilt by macOS.
pub const LOSS_WINDOW: Duration = Duration::from_secs(5);
/// How long a lost window waits to be claimed before it's reported destroyed.
pub const KEEP_LOST_FOR: Duration = Duration::from_secs(60);

/// The last placement core asked for, to apply again to the window that replaces it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intent {
    At(Rect),
    Hidden,
}

/// A window that vanished during a display change.
#[derive(Clone, Debug, PartialEq)]
pub struct LostWindow {
    pub id: WindowId,
    pub pid: i32,
    pub kind: WindowKind,
    /// Where it last was (the last frame we set), to tell an app's windows apart.
    pub frame: Option<Rect>,
    pub intent: Option<Intent>,
    pub lost_at: Instant,
}

/// True when a window destroyed at `now` may just be rebuilt by macOS.
pub fn is_display_loss(last_display_change: Option<Instant>, now: Instant) -> bool {
    last_display_change.is_some_and(|at| now.duration_since(at) < LOSS_WINDOW)
}

/// Lost windows waiting for their app to show them again.
#[derive(Debug, Default)]
pub struct LostWindows {
    windows: Vec<LostWindow>,
}

impl LostWindows {
    pub fn remember(&mut self, window: LostWindow) {
        self.windows.push(window);
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// The lost window of app `pid` and the same `kind` nearest to `frame`, if one is still
    /// waiting at `now`. A popup or dialog never takes over a normal window's place.
    pub fn claim(
        &mut self,
        pid: i32,
        kind: WindowKind,
        frame: Rect,
        now: Instant,
    ) -> Option<LostWindow> {
        let distance = |w: &LostWindow| {
            w.frame
                .map_or(f64::MAX, |f| (f.x - frame.x).hypot(f.y - frame.y))
        };
        let index = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| w.pid == pid && w.kind == kind && now < w.lost_at + KEEP_LOST_FOR)
            .min_by(|(_, a), (_, b)| distance(a).total_cmp(&distance(b)))
            .map(|(i, _)| i)?;
        Some(self.windows.remove(index))
    }

    /// Removes and returns the windows nobody claimed in time: they really are gone.
    pub fn take_expired(&mut self, now: Instant) -> Vec<WindowId> {
        self.take_where(|w| now >= w.lost_at + KEEP_LOST_FOR)
    }

    /// Removes and returns app `pid`'s lost windows (it quit, so they won't come back).
    pub fn take_pid(&mut self, pid: i32) -> Vec<WindowId> {
        self.take_where(|w| w.pid == pid)
    }

    /// When the next lost window expires.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.windows.iter().map(|w| w.lost_at + KEEP_LOST_FOR).min()
    }

    fn take_where(&mut self, gone: impl Fn(&LostWindow) -> bool) -> Vec<WindowId> {
        let (taken, kept): (Vec<_>, Vec<_>) = self.windows.drain(..).partition(|w| gone(w));
        self.windows = kept;
        taken.into_iter().map(|w| w.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use wm_core::{Rect, WindowId, WindowKind};

    fn lost(id: u64, pid: i32, x: f64, at: Instant) -> LostWindow {
        LostWindow {
            id: WindowId(id),
            pid,
            kind: WindowKind::Normal,
            frame: Some(Rect::new(x, 35.0, 953.0, 518.0)),
            intent: Some(Intent::Hidden),
            lost_at: at,
        }
    }

    #[test]
    fn mon_7_destroy_soon_after_a_display_change_is_a_loss() {
        let now = Instant::now();
        assert!(is_display_loss(Some(now), now + Duration::from_millis(100)));
        assert!(!is_display_loss(Some(now), now + Duration::from_secs(6)));
        assert!(!is_display_loss(None, now));
    }

    #[test]
    fn mon_7_new_window_of_the_same_app_claims_the_nearest() {
        let now = Instant::now();
        let mut list = LostWindows::default();
        list.remember(lost(2, 10, 1733.0, now));
        list.remember(lost(3, 10, 2690.0, now));
        let later = now + Duration::from_secs(20);
        let claimed = list.claim(
            10,
            WindowKind::Normal,
            Rect::new(2690.0, 35.0, 953.0, 518.0),
            later,
        );
        assert_eq!(claimed.map(|w| w.id), Some(WindowId(3)));
        let claimed = list.claim(
            10,
            WindowKind::Normal,
            Rect::new(1733.0, 35.0, 953.0, 1040.0),
            later,
        );
        assert_eq!(claimed.map(|w| w.id), Some(WindowId(2)));
        assert!(list.is_empty());
    }

    #[test]
    fn mon_7_another_apps_window_claims_nothing() {
        let now = Instant::now();
        let mut list = LostWindows::default();
        list.remember(lost(2, 10, 1733.0, now));
        assert_eq!(
            list.claim(
                11,
                WindowKind::Normal,
                Rect::new(1733.0, 35.0, 953.0, 518.0),
                now
            ),
            None
        );
    }

    #[test]
    fn mon_7_a_popup_doesnt_claim_a_normal_window() {
        let now = Instant::now();
        let mut list = LostWindows::default();
        list.remember(lost(2, 10, 1733.0, now));
        let frame = Rect::new(1733.0, 35.0, 300.0, 200.0);
        assert_eq!(list.claim(10, WindowKind::Popup, frame, now), None);
        assert!(list.claim(10, WindowKind::Normal, frame, now).is_some());
    }

    #[test]
    fn mon_7_unclaimed_windows_expire_after_a_minute() {
        let now = Instant::now();
        let mut list = LostWindows::default();
        list.remember(lost(2, 10, 1733.0, now));
        assert_eq!(list.next_expiry(), Some(now + KEEP_LOST_FOR));
        assert_eq!(list.take_expired(now + Duration::from_secs(30)), vec![]);
        assert_eq!(
            list.claim(
                10,
                WindowKind::Normal,
                Rect::new(0.0, 0.0, 1.0, 1.0),
                now + KEEP_LOST_FOR
            ),
            None
        );
        assert_eq!(list.take_expired(now + KEEP_LOST_FOR), vec![WindowId(2)]);
        assert!(list.is_empty());
    }

    #[test]
    fn mon_7_app_quitting_releases_its_lost_windows() {
        let now = Instant::now();
        let mut list = LostWindows::default();
        list.remember(lost(2, 10, 1733.0, now));
        list.remember(lost(5, 11, 0.0, now));
        assert_eq!(list.take_pid(10), vec![WindowId(2)]);
        assert_eq!(list.take_pid(10), vec![]);
        assert!(!list.is_empty());
    }
}
