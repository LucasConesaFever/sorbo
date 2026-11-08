//! Holding on to the window we asked to focus while its app activates (spec 6, Focus).
//!
//! Measured in acceptance: focusing Brave window 4 (Brave not frontmost) made Brave
//! report its previously active window 3 as focused right after we raised 4. Passing that
//! on made core switch focus (and workspace) to window 3. For a short while after `Focus`,
//! a different window reported by the same app is overridden instead of believed.

use std::time::{Duration, Instant};

use wm_core::{Pid, WindowId};

/// How long after `Focus` the app's own choice of window is overridden.
pub const HOLD_FOCUS_FOR: Duration = Duration::from_millis(500);
/// How many times we re-apply our choice before believing the app.
pub const MAX_REASSERTS: u8 = 2;

/// A `Focus` we sent and are still waiting to see confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingFocus {
    pub id: WindowId,
    pub pid: i32,
    pub until: Instant,
    pub reasserts_left: u8,
}

/// What to do with an app's report of its focused window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusReport {
    /// Pass it on to core.
    Report,
    /// The app picked another of its own windows while activating: focus ours again.
    Reassert,
}

/// Decides what to do when app `pid` reports `focused` as its focused window at `now`.
pub fn decide(
    pending: Option<PendingFocus>,
    pid: i32,
    focused: Option<WindowId>,
    now: Instant,
) -> FocusReport {
    match pending {
        Some(p)
            if p.pid == pid
                && now < p.until
                && p.reasserts_left > 0
                && focused.is_some()
                && focused != Some(p.id) =>
        {
            FocusReport::Reassert
        }
        _ => FocusReport::Report,
    }
}

/// How long an activation that would switch to a hidden workspace waits to see whether the
/// previously active app is quitting (FOC-8, D29). macOS activates the next app as the
/// active one quits, and its "terminated" notice can arrive a few ms after the activation.
pub const QUIT_WAIT: Duration = Duration::from_millis(150);

/// An app activation held back for `QUIT_WAIT`: app `pid` became active and its focused
/// window `id` is on a hidden workspace; `previous` was active before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldActivation {
    pub pid: Pid,
    pub previous: Pid,
    pub id: WindowId,
    pub until: Instant,
}

/// What a held activation turns into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationOutcome {
    /// The previous app quit and macOS picked the next one: stay on this workspace (FOC-8).
    Stay,
    /// The user switched apps (Cmd+Tab, Dock): follow it to its workspace (FOC-7).
    Follow(WindowId),
}

/// An app activation: which app became active, and which was active before. Named fields,
/// so the two pids can't be passed the wrong way round (that would invert FOC-8, HARD-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Activation {
    pub app: Pid,
    pub previous: Pid,
}

impl HeldActivation {
    pub fn new(activation: Activation, id: WindowId, now: Instant) -> HeldActivation {
        HeldActivation {
            pid: activation.app,
            previous: activation.previous,
            id,
            until: now + QUIT_WAIT,
        }
    }

    /// The activation being held.
    pub fn activation(&self) -> Activation {
        Activation {
            app: self.pid,
            previous: self.previous,
        }
    }

    /// App `pid` terminated.
    pub fn on_terminated(&self, pid: Pid) -> Option<ActivationOutcome> {
        (pid == self.previous).then_some(ActivationOutcome::Stay)
    }

    /// The wait timer fired at `now`.
    pub fn on_timeout(&self, now: Instant) -> Option<ActivationOutcome> {
        (now >= self.until).then_some(ActivationOutcome::Follow(self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(now: Instant) -> HeldActivation {
        let activation = Activation {
            app: Pid(20),
            previous: Pid(10),
        };
        HeldActivation::new(activation, WindowId(4), now)
    }

    #[test]
    fn hard_7_held_activation_names_its_pids() {
        let h = held(Instant::now());
        assert_eq!((h.pid, h.previous), (Pid(20), Pid(10)));
        assert_eq!(h.on_terminated(Pid(10)), Some(ActivationOutcome::Stay));
        assert_eq!(h.on_terminated(Pid(20)), None);
    }

    #[test]
    fn foc_8_previous_app_quitting_keeps_the_workspace() {
        let now = Instant::now();
        assert_eq!(
            held(now).on_terminated(Pid(10)),
            Some(ActivationOutcome::Stay)
        );
    }

    #[test]
    fn foc_8_other_apps_quitting_change_nothing() {
        assert_eq!(held(Instant::now()).on_terminated(Pid(30)), None);
    }

    #[test]
    fn foc_8_previous_app_still_running_follows_after_the_wait() {
        let now = Instant::now();
        let h = held(now);
        assert_eq!(h.on_timeout(now + QUIT_WAIT / 2), None);
        assert_eq!(
            h.on_timeout(now + QUIT_WAIT),
            Some(ActivationOutcome::Follow(WindowId(4)))
        );
    }

    fn pending(now: Instant) -> PendingFocus {
        PendingFocus {
            id: WindowId(4),
            pid: 50,
            until: now + HOLD_FOCUS_FOR,
            reasserts_left: MAX_REASSERTS,
        }
    }

    #[test]
    fn foc_requested_window_is_reported() {
        let now = Instant::now();
        assert_eq!(
            decide(Some(pending(now)), 50, Some(WindowId(4)), now),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_app_picking_its_other_window_is_overridden() {
        let now = Instant::now();
        assert_eq!(
            decide(Some(pending(now)), 50, Some(WindowId(3)), now),
            FocusReport::Reassert
        );
    }

    #[test]
    fn foc_late_report_is_believed() {
        let now = Instant::now();
        let later = now + HOLD_FOCUS_FOR + Duration::from_millis(1);
        assert_eq!(
            decide(Some(pending(now)), 50, Some(WindowId(3)), later),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_believed_after_reasserts_run_out() {
        let now = Instant::now();
        let mut p = pending(now);
        p.reasserts_left = 0;
        assert_eq!(
            decide(Some(p), 50, Some(WindowId(3)), now),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_other_apps_are_reported() {
        let now = Instant::now();
        assert_eq!(
            decide(Some(pending(now)), 77, Some(WindowId(9)), now),
            FocusReport::Report
        );
        assert_eq!(
            decide(None, 50, Some(WindowId(3)), now),
            FocusReport::Report
        );
    }
}
