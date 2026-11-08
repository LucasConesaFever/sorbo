//! The window server's list of windows: owner pid and bounds (SAFE-3). Unlike Accessibility,
//! an app can't fake this, and reading it needs no permission beyond what sorbo has (window
//! names would need Screen Recording; sorbo never reads them). Spike:
//! `examples/spike_window_list.rs`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFString, CFType, CGRect};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID,
};
use wm_core::Rect;

/// A list fetched this recently is reused, so tracking many windows at once (startup, an app
/// unhiding) asks the window server once, not once per window.
const REUSE_FOR: Duration = Duration::from_millis(50);

/// How long the pointer's view of the screen is reused. The pointer moves far more often than
/// windows do; a slightly stale list only means a slow app is skipped a moment late.
const ON_SCREEN_REUSE_FOR: Duration = Duration::from_millis(250);

/// On-screen layer-0 windows as of `.0`: (owner pid, bounds), front to back (HARD-5).
pub(crate) type OnScreen = Option<(Instant, Vec<(i32, Rect)>)>;

/// The on-screen app windows, front to back, from `cache` if it's fresh.
pub(crate) fn on_screen(cache: &mut OnScreen, now: Instant) -> &[(i32, Rect)] {
    let fresh = cache
        .as_ref()
        .is_some_and(|(at, _)| now.saturating_duration_since(*at) < ON_SCREEN_REUSE_FOR);
    if !fresh {
        *cache = Some((now, on_screen_layer_0()));
    }
    cache
        .as_ref()
        .map(|(_, w)| w.as_slice())
        .unwrap_or_default()
}

/// Every on-screen window at layer 0 (apps' ordinary windows), front to back.
fn on_screen_layer_0() -> Vec<(i32, Rect)> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: the window list is documented as an array of CFDictionary.
    let list: &CFArray<CFDictionary> = unsafe { list.cast_unchecked() };
    let mut out = Vec::new();
    for dict in list.iter() {
        // SAFETY: window info dictionaries have CFString keys and CF object values.
        let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
        // SAFETY: valid static CFStrings provided by CoreGraphics.
        let (layer, pid) = unsafe {
            (
                number(dict, kCGWindowLayer),
                number(dict, kCGWindowOwnerPID),
            )
        };
        if layer != Some(0) {
            continue;
        }
        if let (Some(Ok(pid)), Some(b)) = (pid.map(i32::try_from), bounds(dict)) {
            out.push((
                pid,
                Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height),
            ));
        }
    }
    out
}

/// The window-server list as of `.0`, by owner pid.
pub(crate) type Cached = Option<(Instant, HashMap<i32, Vec<Rect>>)>;

/// The bounds of every window `pid` owns, on screen or not (windows
/// sorbo parks off screen are still listed), from `cache` if it's fresh.
pub(crate) fn bounds_for(cache: &mut Cached, pid: i32, now: Instant) -> Vec<Rect> {
    let fresh = cache
        .as_ref()
        .is_some_and(|(at, _)| now.saturating_duration_since(*at) < REUSE_FOR);
    if !fresh {
        *cache = Some((now, bounds_by_pid()));
    }
    cache
        .as_ref()
        .and_then(|(_, by_pid)| by_pid.get(&pid).cloned())
        .unwrap_or_default()
}

/// Every window's bounds, by owner pid.
fn bounds_by_pid() -> HashMap<i32, Vec<Rect>> {
    let mut by_pid: HashMap<i32, Vec<Rect>> = HashMap::new();
    let options = CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return by_pid;
    };
    // SAFETY: the window list is documented as an array of CFDictionary.
    let list: &CFArray<CFDictionary> = unsafe { list.cast_unchecked() };
    for dict in list.iter() {
        // SAFETY: window info dictionaries have CFString keys and CF object values.
        let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
        // Every layer: floating panels (Colors, inspectors) sit above layer 0, and bounds
        // can't be faked at any layer. Each real window backs only one claim (`confirmed`).
        // SAFETY: valid static CFString provided by CoreGraphics.
        let Some(pid) = number(dict, unsafe { kCGWindowOwnerPID }) else {
            continue;
        };
        if let (Ok(pid), Some(b)) = (i32::try_from(pid), bounds(dict)) {
            by_pid.entry(pid).or_default().push(Rect::new(
                b.origin.x,
                b.origin.y,
                b.size.width,
                b.size.height,
            ));
        }
    }
    by_pid
}

fn bounds(dict: &CFDictionary<CFString, CFType>) -> Option<CGRect> {
    // SAFETY: valid static CFString provided by CoreGraphics.
    let bounds = dict.get(unsafe { kCGWindowBounds })?;
    let bounds = bounds.downcast::<CFDictionary>().ok()?;
    let mut r = CGRect::default();
    // SAFETY: `bounds` is the dictionary form of a CGRect; `r` is a valid out-pointer.
    let ok = unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut r) };
    ok.then_some(r)
}

fn number(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}
