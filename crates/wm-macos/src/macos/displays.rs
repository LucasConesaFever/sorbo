//! Monitor snapshots and display-change notifications (spec 5.1 and 5.8).

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Instant;

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFRetained, CFRunLoopTimer, CFUUID};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayChangeSummaryFlags, CGDisplayModelNumber,
    CGDisplayRegisterReconfigurationCallback, CGDisplaySerialNumber, CGDisplayVendorNumber,
    CGMainDisplayID,
};
use objc2_foundation::{NSNumber, NSString};
use wm_core::{Event, MonitorInfo, Rect};

use super::{Shared, arm, disarm, info_ptr, run_or_defer, run_or_retry};
use crate::echo::Settle;
use crate::monitors::{DisplayKeys, monitor_ids, to_top_left};
use crate::wm_log;

/// One connected display, as `wm-macos` sees it.
#[derive(Clone, Debug)]
pub(crate) struct Display {
    /// Full display bounds, top-left origin (includes menu bar and Dock).
    pub bounds: Rect,
    /// What core gets: stable ID, usable frame, main flag.
    pub info: MonitorInfo,
}

// `CGDisplayCreateUUIDFromDisplayID` isn't in the objc2 crates. It's declared in ColorSync.
#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: CGDirectDisplayID) -> *const CFUUID;
}

/// Reads every connected display right now.
pub(crate) fn snapshot(mtm: MainThreadMarker) -> Vec<Display> {
    let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
    let key = NSString::from_str("NSScreenNumber");
    let mut keys = Vec::new();
    let mut rest = Vec::new();
    for screen in NSScreen::screens(mtm).iter() {
        let Some(number) = screen
            .deviceDescription()
            .objectForKey(&key)
            .and_then(|n| n.downcast::<NSNumber>().ok())
        else {
            wm_log!("screen without NSScreenNumber, skipped");
            continue;
        };
        let cg_id = number.unsignedIntValue();
        let b = CGDisplayBounds(cg_id);
        let bounds = Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height);
        let v = screen.visibleFrame();
        let visible = Rect::new(v.origin.x, v.origin.y, v.size.width, v.size.height);
        keys.push(DisplayKeys {
            vendor: CGDisplayVendorNumber(cg_id),
            model: CGDisplayModelNumber(cg_id),
            serial: CGDisplaySerialNumber(cg_id),
            uuid: display_uuid(cg_id),
        });
        rest.push((cg_id, bounds, to_top_left(visible, main_height)));
    }
    // Identity by port, falling back to the UUID (spec 5.1, MON-2).
    monitor_ids(&keys, &super::ports::port_entries())
        .into_iter()
        .zip(rest)
        .map(|(id, (cg_id, bounds, frame))| {
            wm_log!("display {cg_id}: id {} bounds {bounds:?}", id.0);
            Display {
                bounds,
                info: MonitorInfo {
                    id,
                    frame,
                    // The main display is the one at the origin of the global coordinate space.
                    is_main: bounds.x == 0.0 && bounds.y == 0.0,
                },
            }
        })
        .collect()
}

/// The display's UUID as a string, or a fallback built from the CG ID if macOS has none.
fn display_uuid(cg_id: CGDirectDisplayID) -> String {
    // SAFETY: any display ID is accepted; an unknown one returns null.
    let raw = unsafe { CGDisplayCreateUUIDFromDisplayID(cg_id) };
    let Some(raw) = NonNull::new(raw as *mut CFUUID) else {
        return format!("display-{cg_id}");
    };
    // SAFETY: the function follows the Create rule: we own one reference, released on drop.
    let uuid = unsafe { CFRetained::from_raw(raw) };
    CFUUID::new_string(None, Some(&uuid))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("display-{cg_id}"))
}

/// The `MonitorInfo`s core wants, in snapshot order.
pub(crate) fn monitor_infos(displays: &[Display]) -> Vec<MonitorInfo> {
    displays.iter().map(|d| d.info.clone()).collect()
}

/// Registers for display changes. Each change (re)starts the 300 ms settle timer (made in
/// `MacPlatform::new`); when it fires, one `MonitorsChanged` snapshot is queued (spec 5.8).
pub(crate) fn install(shared: &'static RefCell<Shared>) -> Result<(), String> {
    let info = info_ptr(shared);
    // SAFETY: `info` stays valid as long as `MacPlatform` lives, which outlives the WM loop.
    let err = unsafe { CGDisplayRegisterReconfigurationCallback(Some(on_display_change), info) };
    if err.0 != 0 {
        return Err(format!(
            "CGDisplayRegisterReconfigurationCallback failed: {err:?}"
        ));
    }
    Ok(())
}

/// Called by CoreGraphics, possibly several times per plug or unplug.
unsafe extern "C-unwind" fn on_display_change(
    _display: CGDirectDisplayID,
    _flags: CGDisplayChangeSummaryFlags,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer registered in `install`.
    unsafe { run_or_defer(info, "display change", display_changed) };
}

fn display_changed(s: &mut Shared) {
    let now = Instant::now();
    s.last_display_change = Some(now);
    s.debounce.poke(now);
    if let (Some(timer), Some(deadline)) = (&s.display_timer, s.debounce.deadline()) {
        arm(timer, deadline.saturating_duration_since(now));
    }
}

/// Fires once the display changes have been quiet for 300 ms.
pub(crate) unsafe extern "C-unwind" fn on_settle_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "display settle timer", settle) };
}

fn settle(s: &mut Shared) {
    let now = Instant::now();
    let removal_due = s.removal_grace.deadline().is_some_and(|d| now >= d);
    if !s.debounce.take_due(now) && !removal_due {
        return;
    }
    if let Some(timer) = &s.display_timer {
        disarm(timer);
    }
    let fresh = snapshot(s.mtm);
    // A monitor missing for less than 2 s is a flap (screen wake, unlock): wait (MON-6).
    let missing = s
        .displays
        .iter()
        .any(|d| !fresh.iter().any(|f| f.info.id == d.info.id));
    if let Settle::WaitUntil(at) = s.removal_grace.decide(missing, now) {
        wm_log!("a monitor is missing; waiting to see if it comes back (MON-6)");
        if let Some(timer) = &s.display_timer {
            arm(timer, at.saturating_duration_since(now));
        }
        return;
    }
    s.displays = fresh;
    let monitors = monitor_infos(&s.displays);
    s.push(Event::MonitorsChanged(monitors));
    // Reborrow as a plain `&mut Shared` to use two fields at once.
    let s = &mut *s;
    super::windows::repark_hidden(&mut s.windows, &s.displays);
    // Windows macOS moved while the displays settled: look at them now (D53).
    for id in s.held_frames.take() {
        super::observers::check_frame_later(s, id);
    }
    // The main display may have moved or changed size: put the banner back across its top.
    super::banner::refresh(s);
}
