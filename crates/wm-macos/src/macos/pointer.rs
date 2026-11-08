//! Pointer movement and hit testing (spec 5.3).
//!
//! A listen-only event tap records the latest pointer position; it can't block or change
//! events. `next_event` hit-tests only the latest position, and only when it moved at least
//! 1 point, so a burst of mouse events costs one hit test.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopTimer, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
    CGEventType,
};
use wm_core::{Point, PointerTarget};

use super::{Shared, arm, ax, info_ptr, make_timer, run_or_defer, run_or_retry, window_list};
use crate::classify::{pointer_target, topmost_pid_at};
use crate::limits::{Budget, POINTER_HIT_TIME};
use crate::wm_log;

/// Only hit-test again once the pointer has moved this far (points).
const MIN_MOVE: f64 = 1.0;
/// How far up the element tree to look for the window.
const MAX_ANCESTORS: usize = 30;
/// Poll interval if the event tap can't be created.
const POLL_EVERY: Duration = Duration::from_millis(16);

/// Starts listening to pointer movement: an event tap, or a 60 Hz poll if that fails.
pub(crate) fn install(shared: &'static RefCell<Shared>) -> Result<(), String> {
    let info = info_ptr(shared);
    let mask: u64 = [
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDragged,
    ]
    .iter()
    .fold(0, |mask, t| mask | (1u64 << t.0));
    // SAFETY: `on_event` matches CGEventTapCallBack; `info` is the shared-state pointer,
    // valid as long as `MacPlatform` lives.
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::HIDEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            mask,
            Some(on_event),
            info,
        )
    };
    let run_loop = CFRunLoop::main().ok_or("no main run loop")?;
    match tap {
        Some(tap) => {
            let source = CFMachPort::new_run_loop_source(None, Some(&tap), 0)
                .ok_or("could not create the event tap's run-loop source")?;
            // SAFETY: reading an extern static provided by CoreFoundation.
            run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
            shared.borrow_mut().event_tap = Some(tap);
        }
        None => {
            wm_log!("could not create a pointer event tap; polling the pointer instead");
            let timer = make_timer(shared, on_poll_timer)?;
            arm(&timer, POLL_EVERY);
            shared.borrow_mut().poll_timer = Some(timer);
        }
    }
    Ok(())
}

/// Called for every pointer event. Only records the position; never changes the event.
unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        // macOS turns a slow tap off; turn it back on.
        let work = |s: &mut Shared| {
            if let Some(tap) = &s.event_tap {
                CGEvent::tap_enable(tap, true);
            }
        };
        // SAFETY: `info` is the shared-state pointer registered in `install`.
        unsafe { run_or_defer(info, "pointer tap", work) };
        return event.as_ptr();
    }
    // SAFETY: the event is valid for the duration of the callback.
    let location = CGEvent::location(Some(unsafe { event.as_ref() }));
    let p = Point::new(location.x, location.y);
    let work = move |s: &mut Shared| {
        s.pending_pointer = Some(p);
        // Wake `next_event`: the tap firing alone doesn't end AppKit's wait for an event.
        s.wake();
    };
    // SAFETY: as above.
    unsafe { run_or_defer(info, "pointer move", work) };
    event.as_ptr()
}

/// Fallback when there's no event tap: read the pointer position 60 times a second.
unsafe extern "C-unwind" fn on_poll_timer(timer: *mut CFRunLoopTimer, info: *mut c_void) {
    let event = CGEvent::new(None);
    let location = CGEvent::location(event.as_deref());
    let work = |s: &mut Shared| {
        s.pending_pointer = Some(Point::new(location.x, location.y));
        if let Some(timer) = &s.poll_timer {
            arm(timer, POLL_EVERY);
        }
        s.wake();
    };
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "pointer poll", work) };
}

/// If the pointer moved enough since the last hit test, hit-test its latest position.
pub(crate) fn take_pointer_event(s: &mut Shared) -> Option<(Point, PointerTarget)> {
    let p = s.pending_pointer.take()?;
    // A let-chain: both conditions in one `if` (Rust 2024).
    if let Some(last) = s.last_hit
        && (p.x - last.x).abs() < MIN_MOVE
        && (p.y - last.y).abs() < MIN_MOVE
    {
        return None;
    }
    s.last_hit = Some(p);
    Some((p, hit_test(s, p)))
}

/// What's under the pointer, using Accessibility (it skips invisible overlays such as the
/// cursor's own window, which the window server's list doesn't).
///
/// The app under the pointer answers every call, so the walk has a time budget (HARD-5):
/// an app that makes it overrun counts as "not a window" now and for the next 2 s, instead
/// of stalling the WM (and its hotkeys) on every pointer move.
pub(crate) fn hit_test(s: &mut Shared, p: Point) -> PointerTarget {
    let started = Instant::now();
    let budget = Budget::new(started, POINTER_HIT_TIME);
    // The window server says whose window this is without messaging the app, so an app being
    // skipped costs nothing, even a hung one.
    let owner = topmost_pid_at(window_list::on_screen(&mut s.on_screen, started), p);
    if owner.is_some_and(|pid| s.slow_apps.is_cooling(pid, started)) {
        return PointerTarget::Other;
    }
    // SAFETY: creating the system-wide element has no preconditions.
    let system = unsafe { AXUIElement::new_system_wide() };
    let mut raw: *const AXUIElement = std::ptr::null();
    // SAFETY: `raw` is a valid out-pointer for the duration of the call.
    let err =
        unsafe { system.copy_element_at_position(p.x as f32, p.y as f32, NonNull::from(&mut raw)) };
    let Some(raw) = NonNull::new(raw as *mut AXUIElement).filter(|_| err == AXError::Success)
    else {
        // A hung app makes this first call time out (`CannotComplete`) and gives no pid:
        // skip the window server's owner instead.
        if (err == AXError::CannotComplete || budget.spent(Instant::now()))
            && let Some(pid) = owner
        {
            return slow_app(s, pid, started);
        }
        return PointerTarget::Other;
    };
    // SAFETY: AX returned a +1 reference ("Copy rule") that we now own.
    let element = unsafe { CFRetained::from_raw(raw) };
    let Some(pid) = ax::element_pid(&element) else {
        return PointerTarget::Other;
    };
    if s.slow_apps.is_cooling(pid, started) {
        return PointerTarget::Other;
    }

    // Walk up from the hit element, collecting roles, until we reach its window.
    let mut roles = Vec::new();
    let mut window = None;
    let mut current = Some(element.clone());
    for _ in 0..MAX_ANCESTORS {
        if budget.spent(Instant::now()) {
            return slow_app(s, pid, started);
        }
        let Some(el) = current else { break };
        let role = ax::string_attr(&el, "AXRole").unwrap_or_default();
        if role == "AXWindow" {
            window = Some(el);
            break;
        }
        roles.push(role);
        // Two calls per step: check between them too.
        if budget.spent(Instant::now()) {
            return slow_app(s, pid, started);
        }
        current = ax::attr(&el, "AXParent")
            .ok()
            .and_then(|v| v.downcast::<AXUIElement>().ok());
    }
    // Very deep web content: fall back to the element's own AXWindow attribute.
    if window.is_none() {
        if budget.spent(Instant::now()) {
            return slow_app(s, pid, started);
        }
        window = ax::attr(&element, "AXWindow")
            .ok()
            .and_then(|v| v.downcast::<AXUIElement>().ok());
    }
    let tracked = window.and_then(|w| s.windows.find(&w, pid)).and_then(|id| {
        let hidden = s.windows.get(id)?.hidden;
        Some((id, hidden))
    });
    pointer_target(&roles, tracked)
}

/// App `pid` made the hit test overrun its budget: skip it for a while (HARD-5).
fn slow_app(s: &mut Shared, pid: i32, started: Instant) -> PointerTarget {
    wm_log!(
        "pointer: app {pid} took over {} ms to answer; skipping it for 2 s (HARD-5)",
        started.elapsed().as_millis()
    );
    s.slow_apps.mark(pid, Instant::now());
    PointerTarget::Other
}
