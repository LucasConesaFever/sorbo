//! Telling our own moves apart from the user's, and waiting for display changes to settle
//! (spec 5.8).

use std::time::{Duration, Instant};

use wm_core::{Rect, WindowId};

/// A frame this close (on every edge) to the one we last set is our own move echoing back.
pub const ECHO_TOLERANCE: f64 = 2.0;
/// How long a window must stay still before its move/resize is reported.
pub const FRAME_SETTLE: Duration = Duration::from_millis(300);
/// How long display changes must stay quiet before one `MonitorsChanged` is sent.
pub const DISPLAY_SETTLE: Duration = Duration::from_millis(300);

/// True if a settled window `frame` is just the echo of our own `SetFrame` (`last_set` =
/// the frame we set).
pub fn is_echo(last_set: Option<Rect>, frame: Rect) -> bool {
    let Some(set) = last_set else {
        return false;
    };
    let close = |a: f64, b: f64| (a - b).abs() <= ECHO_TOLERANCE;
    close(set.x, frame.x)
        && close(set.y, frame.y)
        && close(set.x + set.width, frame.x + frame.width)
        && close(set.y + set.height, frame.y + frame.height)
}

/// Windows that moved or resized recently, each with the time it settles: `FRAME_SETTLE`
/// after its last notification. A drag only gets reported once the window stops.
#[derive(Debug, Default)]
pub struct SettleQueue {
    pending: Vec<(WindowId, Instant)>,
}

impl SettleQueue {
    pub fn new() -> SettleQueue {
        SettleQueue {
            pending: Vec::new(),
        }
    }

    /// `id` moved at `now`: (re)start its wait.
    pub fn poke(&mut self, id: WindowId, now: Instant) {
        let due = now + FRAME_SETTLE;
        match self.pending.iter_mut().find(|(w, _)| *w == id) {
            Some(entry) => entry.1 = due,
            None => self.pending.push((id, due)),
        }
    }

    /// The windows that have settled by `now`, removed from the queue.
    pub fn take_due(&mut self, now: Instant) -> Vec<WindowId> {
        let due: Vec<WindowId> = self
            .pending
            .iter()
            .filter(|(_, at)| *at <= now)
            .map(|(id, _)| *id)
            .collect();
        self.pending.retain(|(_, at)| *at > now);
        due
    }

    /// When the next window settles, if any is waiting.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|(_, at)| *at).min()
    }

    /// Forgets a window (it was closed or hidden).
    pub fn remove(&mut self, id: WindowId) {
        self.pending.retain(|(w, _)| *w != id);
    }
}

/// Collapses a burst of display-change callbacks into one, `DISPLAY_SETTLE` after the last.
#[derive(Debug, Default)]
pub struct Debounce {
    deadline: Option<Instant>,
}

impl Debounce {
    pub fn new() -> Debounce {
        Debounce { deadline: None }
    }

    /// A change happened at `now`: (re)start the wait.
    pub fn poke(&mut self, now: Instant) {
        self.deadline = Some(now + DISPLAY_SETTLE);
    }

    /// When the pending change becomes due, if one is pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// True exactly once, the first time it's asked at or after the deadline.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(d) if now >= d => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }
}

/// How long a monitor may be missing before core hears it's gone (MON-6, D30). macOS
/// reports the external monitor gone for about a second when the screen wakes or unlocks.
pub const REMOVAL_GRACE: Duration = Duration::from_secs(2);

/// What to do with a settled display snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// Send it to core.
    Report,
    /// A monitor is missing: look again at this time.
    WaitUntil(Instant),
}

/// Holds back snapshots that lose a monitor until it has been missing for `REMOVAL_GRACE`.
#[derive(Debug, Default)]
pub struct RemovalGrace {
    deadline: Option<Instant>,
}

impl RemovalGrace {
    pub fn new() -> RemovalGrace {
        RemovalGrace::default()
    }

    /// A snapshot settled at `now`; `monitor_missing` when it lacks a monitor core knows.
    pub fn decide(&mut self, monitor_missing: bool, now: Instant) -> Settle {
        if !monitor_missing {
            self.deadline = None;
            return Settle::Report;
        }
        match self.deadline {
            None => {
                let deadline = now + REMOVAL_GRACE;
                self.deadline = Some(deadline);
                Settle::WaitUntil(deadline)
            }
            Some(deadline) if now < deadline => Settle::WaitUntil(deadline),
            Some(_) => {
                self.deadline = None;
                Settle::Report
            }
        }
    }

    /// When a held-back removal becomes due, if one is.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mon_6_missing_monitor_waits_two_seconds() {
        let now = Instant::now();
        let mut grace = RemovalGrace::new();
        assert_eq!(
            grace.decide(true, now),
            Settle::WaitUntil(now + REMOVAL_GRACE)
        );
        let half = now + REMOVAL_GRACE / 2;
        assert_eq!(
            grace.decide(true, half),
            Settle::WaitUntil(now + REMOVAL_GRACE)
        );
        assert_eq!(grace.decide(true, now + REMOVAL_GRACE), Settle::Report);
        // Settled: a later removal waits again from scratch.
        let later = now + Duration::from_secs(10);
        assert_eq!(
            grace.decide(true, later),
            Settle::WaitUntil(later + REMOVAL_GRACE)
        );
    }

    #[test]
    fn mon_6_monitor_back_within_the_grace_reports_at_once() {
        let now = Instant::now();
        let mut grace = RemovalGrace::new();
        grace.decide(true, now);
        assert_eq!(
            grace.decide(false, now + Duration::from_millis(1300)),
            Settle::Report
        );
        assert_eq!(grace.deadline(), None);
    }

    #[test]
    fn mon_6_additions_report_at_once() {
        assert_eq!(
            RemovalGrace::new().decide(false, Instant::now()),
            Settle::Report
        );
    }

    #[test]
    fn echo_within_tolerance_is_dropped() {
        let set = Rect::new(5.0, 38.0, 800.0, 600.0);
        assert!(is_echo(Some(set), Rect::new(6.5, 38.0, 799.0, 600.0)));
    }

    #[test]
    fn echo_real_user_move_is_kept() {
        let set = Rect::new(5.0, 38.0, 800.0, 600.0);
        assert!(!is_echo(Some(set), Rect::new(300.0, 300.0, 400.0, 400.0)));
    }

    #[test]
    fn echo_never_set_is_kept() {
        assert!(!is_echo(None, Rect::new(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn drag_is_reported_only_after_settling() {
        // A drag sends a burst of moves: nothing is due until 300 ms after the last one.
        let t0 = Instant::now();
        let w = WindowId(7);
        let mut q = SettleQueue::new();
        q.poke(w, t0);
        q.poke(w, t0 + Duration::from_millis(200));
        assert_eq!(q.take_due(t0 + Duration::from_millis(450)), vec![]);
        assert_eq!(q.next_deadline(), Some(t0 + Duration::from_millis(500)));
        assert_eq!(q.take_due(t0 + Duration::from_millis(500)), vec![w]);
        assert_eq!(q.take_due(t0 + Duration::from_millis(900)), vec![]);
        assert_eq!(q.next_deadline(), None);
    }

    #[test]
    fn settle_tracks_windows_separately() {
        let t0 = Instant::now();
        let mut q = SettleQueue::new();
        q.poke(WindowId(1), t0);
        q.poke(WindowId(2), t0 + Duration::from_millis(100));
        assert_eq!(
            q.take_due(t0 + Duration::from_millis(300)),
            vec![WindowId(1)]
        );
        assert_eq!(
            q.take_due(t0 + Duration::from_millis(400)),
            vec![WindowId(2)]
        );
    }

    #[test]
    fn mon_debounce_fires_once_after_settle() {
        let t0 = Instant::now();
        let mut d = Debounce::new();
        d.poke(t0);
        assert!(!d.take_due(t0 + Duration::from_millis(299)));
        assert!(d.take_due(t0 + Duration::from_millis(300)));
        assert!(!d.take_due(t0 + Duration::from_millis(400)));
    }

    #[test]
    fn mon_debounce_restarts_on_new_change() {
        let t0 = Instant::now();
        let mut d = Debounce::new();
        d.poke(t0);
        d.poke(t0 + Duration::from_millis(200));
        assert!(!d.take_due(t0 + Duration::from_millis(450)));
        assert!(d.take_due(t0 + Duration::from_millis(500)));
    }
}
