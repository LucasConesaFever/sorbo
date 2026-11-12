//! Holding on to the window we asked to focus while its app activates (spec 6, Focus).
//!
//! Measured in acceptance: focusing Brave window 4 (Brave not frontmost) made Brave
//! report its previously active window 3 as focused right after we raised 4. Passing that
//! on made core switch focus (and workspace) to window 3. For a short while after `Focus`,
//! a different window reported by the same app is overridden instead of believed.
//!
//! The app confirming our window doesn't end that: kitty confirmed it and then, 25 ms later,
//! focused its window on a hidden workspace, which made FOC-7 switch workspaces (D46). After
//! the confirmation only a hidden window is overridden: the user can't have clicked one.
//!
//! Closing a window does the same without any `Focus` from us: kitty focused another of its
//! windows on screen, then its window on a hidden workspace. For a second after a close, a
//! hidden window isn't believed (FOC-11, D51).

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
    /// The app has reported our window as focused at least once.
    pub confirmed: bool,
}

/// What to do with an app's report of its focused window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusReport {
    /// Pass it on to core.
    Report,
    /// The app picked another of its own windows while activating: focus ours again.
    Reassert,
}

/// Decides what to do when app `pid` reports `focused` as its focused window at `now`;
/// `focused_hidden` says whether that window is parked on a hidden workspace.
pub fn decide(
    pending: Option<PendingFocus>,
    pid: i32,
    focused: Option<WindowId>,
    focused_hidden: bool,
    now: Instant,
) -> FocusReport {
    match pending {
        Some(p)
            if p.pid == pid
                && now < p.until
                && p.reasserts_left > 0
                && focused.is_some()
                && focused != Some(p.id)
                && (!p.confirmed || focused_hidden) =>
        {
            FocusReport::Reassert
        }
        _ => FocusReport::Report,
    }
}

/// How long after a window closes focus on a hidden window is refused (FOC-11). kitty
/// moved focus to its window on a hidden workspace 270 ms after one of its windows closed.
pub const HOLD_AFTER_CLOSE: Duration = Duration::from_secs(1);

/// What to do with a focus report shortly after a window closed (FOC-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterClose {
    /// Pass it on to core.
    Report,
    /// An app picked its window on a hidden workspace: focus this shown window instead.
    Refocus(WindowId),
    /// An app picked its window on a hidden workspace and nothing on screen took over:
    /// keep focus on the workspace the closed window was on.
    Ignore,
}

/// Decides what to do when `focused` is reported at `now`, a window having closed before
/// `close_until`. `focused_hidden` says whether it's parked on a hidden workspace;
/// `successor` is the shown window that had focus after the close, if any.
pub fn after_close(
    close_until: Option<Instant>,
    focused: WindowId,
    focused_hidden: bool,
    successor: Option<WindowId>,
    now: Instant,
) -> AfterClose {
    if !focused_hidden || close_until.is_none_or(|until| now >= until) {
        return AfterClose::Report;
    }
    match successor.filter(|s| *s != focused) {
        Some(s) => AfterClose::Refocus(s),
        None => AfterClose::Ignore,
    }
}

/// The pending focus after app `pid`'s report of `focused` was passed on to core: our window
/// marks it confirmed and it stays until it runs out; another window of that app ends it.
pub fn after_report(
    pending: Option<PendingFocus>,
    pid: i32,
    focused: Option<WindowId>,
) -> Option<PendingFocus> {
    match pending {
        Some(p) if p.pid == pid && focused == Some(p.id) => Some(PendingFocus {
            confirmed: true,
            ..p
        }),
        Some(p) if p.pid == pid => None,
        other => other,
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
            confirmed: false,
        }
    }

    #[test]
    fn foc_requested_window_is_reported() {
        let now = Instant::now();
        assert_eq!(
            decide(Some(pending(now)), 50, Some(WindowId(4)), false, now),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_app_picking_its_other_window_is_overridden() {
        let now = Instant::now();
        assert_eq!(
            decide(Some(pending(now)), 50, Some(WindowId(3)), false, now),
            FocusReport::Reassert
        );
    }

    #[test]
    fn foc_late_report_is_believed() {
        let now = Instant::now();
        let later = now + HOLD_FOCUS_FOR + Duration::from_millis(1);
        assert_eq!(
            decide(Some(pending(now)), 50, Some(WindowId(3)), false, later),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_believed_after_reasserts_run_out() {
        let now = Instant::now();
        let mut p = pending(now);
        p.reasserts_left = 0;
        assert_eq!(
            decide(Some(p), 50, Some(WindowId(3)), false, now),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_other_apps_are_reported() {
        let now = Instant::now();
        assert_eq!(
            decide(Some(pending(now)), 77, Some(WindowId(9)), false, now),
            FocusReport::Report
        );
        assert_eq!(
            decide(None, 50, Some(WindowId(3)), false, now),
            FocusReport::Report
        );
    }

    #[test]
    fn foc_1_the_guard_outlives_the_confirmation() {
        // Found in daily use: kitty confirmed the window sorbo focused, then 25 ms later
        // focused its window on a hidden workspace, and FOC-7 switched to it.
        let now = Instant::now();
        let p = after_report(Some(pending(now)), 50, Some(WindowId(4)));
        assert_eq!(p.map(|p| p.confirmed), Some(true));
        let soon = now + Duration::from_millis(25);
        assert_eq!(
            decide(p, 50, Some(WindowId(5)), true, soon),
            FocusReport::Reassert
        );
    }

    #[test]
    fn foc_1_after_the_confirmation_a_shown_window_is_believed() {
        // A click on another of the app's windows on screen is the user's choice.
        let now = Instant::now();
        let p = after_report(Some(pending(now)), 50, Some(WindowId(4)));
        let soon = now + Duration::from_millis(25);
        assert_eq!(
            decide(p, 50, Some(WindowId(13)), false, soon),
            FocusReport::Report
        );
        assert_eq!(after_report(p, 50, Some(WindowId(13))), None);
    }

    #[test]
    fn foc_1_another_apps_report_leaves_the_guard() {
        let now = Instant::now();
        let p = Some(pending(now));
        assert_eq!(after_report(p, 77, Some(WindowId(9))), p);
    }

    #[test]
    fn foc_11_after_a_close_the_app_cant_pull_focus_to_a_hidden_workspace() {
        let now = Instant::now();
        let until = Some(now + HOLD_AFTER_CLOSE);
        let later = now + Duration::from_millis(270);
        assert_eq!(
            after_close(until, WindowId(4), true, Some(WindowId(6)), later),
            AfterClose::Refocus(WindowId(6))
        );
    }

    #[test]
    fn foc_11_an_emptied_workspace_keeps_focus() {
        let now = Instant::now();
        let until = Some(now + HOLD_AFTER_CLOSE);
        assert_eq!(
            after_close(until, WindowId(4), true, None, now),
            AfterClose::Ignore
        );
    }

    #[test]
    fn foc_11_a_shown_window_is_believed() {
        let now = Instant::now();
        let until = Some(now + HOLD_AFTER_CLOSE);
        assert_eq!(
            after_close(until, WindowId(6), false, Some(WindowId(5)), now),
            AfterClose::Report
        );
    }

    #[test]
    fn foc_11_later_a_hidden_window_switches_workspace_again() {
        let now = Instant::now();
        let until = Some(now + HOLD_AFTER_CLOSE);
        assert_eq!(
            after_close(until, WindowId(4), true, None, now + HOLD_AFTER_CLOSE),
            AfterClose::Report
        );
        assert_eq!(
            after_close(None, WindowId(4), true, None, now),
            AfterClose::Report
        );
    }
}
