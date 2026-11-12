//! Focusing windows, warping the pointer, and reporting focus changes (spec 5.7 and 6).

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_application_services::AXError;
use objc2_application_services::AXUIElement;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{CGAssociateMouseAndMouseCursorPosition, CGWarpMouseCursorPosition};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use wm_core::{Event, Pid, Point, WindowId, WindowKind};

use objc2_core_foundation::CFRunLoopTimer;

use super::{Shared, arm, ax, observers, run_or_retry};
use crate::focus_guard::{
    Activation, ActivationOutcome, AfterClose, FocusReport, HOLD_FOCUS_FOR, HeldActivation,
    MAX_REASSERTS, PendingFocus, QUIT_WAIT, after_close, after_report, decide,
};
use crate::wm_log;

/// `Focus(id)`: activate the app first, then make the window main and raise it. Activating
/// first matters: on activation an app may restore its own last window (spike: Brave).
pub(crate) fn focus(s: &mut Shared, id: WindowId) {
    let Some(w) = s.windows.get(id) else {
        wm_log!("Focus for unknown {id:?}");
        return;
    };
    let app = ax::app_element(w.pid);
    let frontmost = ax::set_bool(&app, "AXFrontmost", true);
    ax::set_bool(&w.element, "AXMain", true);
    let raised = ax::perform(&w.element, "AXRaise");
    if frontmost != AXError::Success {
        // Fallback (spec 6). We check the AX error rather than asking NSWorkspace who's in
        // front: NSWorkspace's answer only refreshes between run-loop turns.
        wm_log!("Focus {id:?}: AXFrontmost failed ({frontmost:?}), using activateWithOptions");
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(w.pid) {
            app.activateWithOptions(NSApplicationActivationOptions::empty());
        }
    }
    if raised != AXError::Success {
        wm_log!("Focus {id:?}: AXRaise failed: {raised:?}");
    }
    s.last_focus = Some(id);
    // The app may still pick another of its own windows while it activates (focus_guard).
    s.pending_focus = Some(PendingFocus {
        id,
        pid: w.pid,
        until: Instant::now() + HOLD_FOCUS_FOR,
        reasserts_left: MAX_REASSERTS,
        confirmed: false,
    });
}

/// Makes the pending focus target the app's main window again and raises it.
fn reassert(s: &mut Shared) {
    let Some(p) = s.pending_focus.as_mut() else {
        return;
    };
    p.reasserts_left = p.reasserts_left.saturating_sub(1);
    let id = p.id;
    let Some(w) = s.windows.get(id) else { return };
    wm_log!("app picked another window while activating; focusing {id:?} again");
    ax::set_bool(&w.element, "AXMain", true);
    ax::perform(&w.element, "AXRaise");
}

/// `WarpPointer(p)`: move the pointer without a mouse event.
pub(crate) fn warp(p: Point) {
    let err = CGWarpMouseCursorPosition(CGPoint::new(p.x, p.y));
    if err.0 != 0 {
        wm_log!("WarpPointer failed: {err:?}");
    }
    // Without this the pointer ignores the mouse for a moment after a warp.
    CGAssociateMouseAndMouseCursorPosition(true);
}

/// The app `pid` became active or changed its focused window: tell core which window has
/// focus, or that focus left the windows it manages.
/// App `pid` became active; `previous` was active before it. Like `report_focus`, except
/// when the app's focused window is on a hidden workspace: then we first wait a moment to
/// see whether `previous` is quitting, because then macOS picked `pid` by itself and the WM
/// stays where it is (FOC-8, D29) instead of following (FOC-7).
pub(crate) fn report_activation(s: &mut Shared, pid: i32, previous: Option<i32>) {
    // A newer activation replaces a held one.
    s.held_activation = None;
    if let Some(previous) = previous.filter(|&p| p != pid)
        && s.pending_focus.is_none_or(|p| p.pid != pid)
        && let Some(id) = focused_window_id(s, pid)
        && s.windows.get(id).is_some_and(|w| w.hidden)
    {
        let activation = Activation {
            app: Pid(pid),
            previous: Pid(previous),
        };
        if app_gone(previous) {
            stay(s, activation);
            return;
        }
        s.held_activation = Some(HeldActivation::new(activation, id, Instant::now()));
        if let Some(timer) = &s.activation_timer {
            arm(timer, QUIT_WAIT);
        }
        return;
    }
    report_focus(s, pid);
}

/// App `pid` terminated: a held activation it caused is macOS's own choice.
pub(crate) fn app_quit(s: &mut Shared, pid: i32) {
    if let Some(held) = s.held_activation
        && held.on_terminated(Pid(pid)) == Some(ActivationOutcome::Stay)
    {
        s.held_activation = None;
        stay(s, held.activation());
    }
}

/// The wait is over and the previous app is still running: the user switched apps.
pub(crate) unsafe extern "C-unwind" fn on_activation_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "activation timer", activation_wait_over) };
}

fn activation_wait_over(s: &mut Shared) {
    let Some(held) = s.held_activation else {
        return;
    };
    match held.on_timeout(Instant::now()) {
        Some(_) => {
            s.held_activation = None;
            if app_gone(held.previous.0) {
                stay(s, held.activation());
            } else {
                report_focus(s, held.pid.0);
            }
        }
        // Fired a little early: check again shortly.
        None => {
            if let Some(timer) = &s.activation_timer {
                arm(timer, Duration::from_millis(10));
            }
        }
    }
}

fn stay(s: &mut Shared, activation: Activation) {
    let Activation {
        app: Pid(pid),
        previous: Pid(previous),
    } = activation;
    wm_log!(
        "focus: pid {pid} activated by macOS because pid {previous} quit; staying on this \
         workspace (FOC-8)"
    );
    s.push(Event::FocusLeft);
}

/// True when no running app has this pid any more.
fn app_gone(pid: i32) -> bool {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .is_none_or(|app| app.isTerminated())
}

/// The managed window app `pid` says is focused, if any.
fn focused_window_id(s: &Shared, pid: i32) -> Option<WindowId> {
    let window = ax::attr(&ax::app_element(pid), "AXFocusedWindow")
        .ok()?
        .downcast::<AXUIElement>()
        .ok()?;
    s.windows.find(&window, pid)
}

pub(crate) fn report_focus(s: &mut Shared, pid: i32) {
    let app = ax::app_element(pid);
    let Some(window) = ax::attr(&app, "AXFocusedWindow")
        .ok()
        .and_then(|v| v.downcast::<AXUIElement>().ok())
    else {
        s.push(Event::FocusLeft);
        return;
    };
    if let Some(mut id) = s.windows.find(&window, pid) {
        // A "popup" the user just focused may be a real window classified while its app was
        // busy: classify it again now, so the click focuses it (HARD-2).
        if s.windows
            .get(id)
            .is_some_and(|w| w.kind == WindowKind::Popup)
            && let Some(new_id) = observers::reclassify(s, id)
        {
            id = new_id;
        }
        let hidden = s.windows.get(id).is_some_and(|w| w.hidden);
        let successor = s
            .last_focus
            .filter(|f| s.windows.get(*f).is_some_and(|w| !w.hidden));
        match after_close(s.close_until, id, hidden, successor, Instant::now()) {
            AfterClose::Report => {}
            AfterClose::Refocus(shown) => {
                wm_log!(
                    "{id:?} on a hidden workspace took focus after a close; focusing {shown:?} again (FOC-11)"
                );
                focus(s, shown);
                return;
            }
            AfterClose::Ignore => {
                wm_log!(
                    "{id:?} on a hidden workspace took focus after a close; staying on this workspace (FOC-11)"
                );
                return;
            }
        }
        match decide(s.pending_focus, pid, Some(id), hidden, Instant::now()) {
            FocusReport::Reassert => reassert(s),
            FocusReport::Report => {
                s.pending_focus = after_report(s.pending_focus, pid, Some(id));
                s.last_focus = Some(id);
                s.push(Event::WindowFocused(id));
            }
        }
        return;
    }
    // macOS can announce focus on a new window before announcing the window itself. Track
    // it now so core hears "created" before "focused" instead of a spurious FocusLeft.
    match observers::track_window(s, pid, window) {
        Some(observers::Tracked::New(info)) => {
            let id = info.id;
            s.push(Event::WindowCreated(info));
            s.push(Event::WindowFocused(id));
        }
        Some(observers::Tracked::Rebound(id)) => s.push(Event::WindowFocused(id)),
        None => s.push(Event::FocusLeft),
    }
}
