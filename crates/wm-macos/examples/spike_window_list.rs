//! Spike: does the window server confirm the frames apps report through Accessibility,
//! without asking for any new permission? (SAFE-3, decision D38.)
//!
//! For every running app with windows it prints each AX window's frame (`AXPosition` +
//! `AXSize`) and whether the window server lists a layer-0 window of the same pid whose
//! bounds are within 2 points on every edge (`MATCH`), plus the window server's own list for
//! that pid. It also says whether `kCGWindowName` is present, the one field documented to
//! need Screen Recording permission.
//!
//! Usage (Accessibility must be granted to the terminal; Screen Recording must NOT be):
//!
//!     cargo run -p wm-macos --example spike_window_list

#[cfg(target_os = "macos")]
fn main() {
    spike::main();
}

// Examples are built by `cargo test` too, so this file must compile on Linux.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod spike {
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use objc2_app_kit::NSWorkspace;
    use objc2_application_services::{
        AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType,
    };
    use objc2_core_foundation::{
        CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
    };
    use objc2_core_graphics::{
        CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
        kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowOwnerPID,
    };

    const TOLERANCE: f64 = 2.0;

    pub fn main() {
        // SAFETY: no arguments, no preconditions.
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!("Not trusted for Accessibility. Grant it to your terminal app first.");
            std::process::exit(1);
        }
        let (by_pid, names_seen) = server_windows();
        println!(
            "window server: {} layer-0 windows; non-empty kCGWindowName on {} of them\n",
            by_pid.values().map(Vec::len).sum::<usize>(),
            names_seen
        );
        let (mut matched, mut unmatched) = (0, 0);
        for app in NSWorkspace::sharedWorkspace().runningApplications().iter() {
            let pid = app.processIdentifier();
            let name = app
                .localizedName()
                .map(|n| n.to_string())
                .unwrap_or_default();
            // SAFETY: any pid is accepted; a dead pid yields an element whose calls fail.
            let element = unsafe { AXUIElement::new_application(pid) };
            let windows = elements(&element, "AXWindows");
            if windows.is_empty() {
                continue;
            }
            let server = by_pid.get(&pid).cloned().unwrap_or_default();
            println!("{name} (pid {pid}): {} AX windows", windows.len());
            for w in &windows {
                let Some(f) = frame(w) else {
                    println!("  AX  no frame");
                    continue;
                };
                let ok = server.iter().any(|s| near(&f, s));
                if ok {
                    matched += 1;
                } else {
                    unmatched += 1;
                }
                println!(
                    "  AX  ({:.0},{:.0}) {:.0}x{:.0}  {}",
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                    if ok { "MATCH" } else { "NO MATCH" }
                );
            }
            for s in &server {
                println!(
                    "  WS  ({:.0},{:.0}) {:.0}x{:.0}",
                    s.origin.x, s.origin.y, s.size.width, s.size.height
                );
            }
        }
        println!("\nAX frames matched: {matched}, not matched: {unmatched}");
    }

    fn near(a: &CGRect, b: &CGRect) -> bool {
        (a.origin.x - b.origin.x).abs() <= TOLERANCE
            && (a.origin.y - b.origin.y).abs() <= TOLERANCE
            && (a.size.width - b.size.width).abs() <= TOLERANCE
            && (a.size.height - b.size.height).abs() <= TOLERANCE
    }

    /// Layer-0 window bounds by owner pid, from every window the window server knows
    /// (on screen or not), and how many entries carry a window name.
    fn server_windows() -> (HashMap<i32, Vec<CGRect>>, usize) {
        let mut by_pid: HashMap<i32, Vec<CGRect>> = HashMap::new();
        let mut names = 0;
        let options = CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements;
        let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
            println!("no window list");
            return (by_pid, 0);
        };
        // SAFETY: the window list is documented as an array of CFDictionary.
        let list: &CFArray<CFDictionary> = unsafe { list.cast_unchecked() };
        for dict in list.iter() {
            // SAFETY: window info dictionaries have CFString keys and CF object values.
            let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
            // SAFETY (all three): the statics are valid CFStrings provided by CoreGraphics.
            if number(dict, unsafe { kCGWindowLayer }) != 0 {
                continue;
            }
            let pid = number(dict, unsafe { kCGWindowOwnerPID }) as i32;
            if dict
                .get(unsafe { kCGWindowName })
                .and_then(|v| v.downcast::<CFString>().ok())
                .is_some_and(|s| !s.to_string().is_empty())
            {
                names += 1;
            }
            if let Some(r) = rect(dict) {
                by_pid.entry(pid).or_default().push(r);
            }
        }
        (by_pid, names)
    }

    fn rect(dict: &CFDictionary<CFString, CFType>) -> Option<CGRect> {
        // SAFETY: valid static CFString.
        let bounds = dict.get(unsafe { kCGWindowBounds })?;
        let bounds = bounds.downcast::<CFDictionary>().ok()?;
        let mut r = CGRect::default();
        // SAFETY: `bounds` is the dictionary form of a CGRect; `r` is a valid out-pointer.
        let ok = unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut r) };
        ok.then_some(r)
    }

    fn number(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> i64 {
        dict.get(key)
            .and_then(|v| v.downcast::<CFNumber>().ok())
            .and_then(|n| n.as_i64())
            .unwrap_or(-1)
    }

    fn attr(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
        let name = CFString::from_str(name);
        let mut value: *const CFType = std::ptr::null();
        // SAFETY: `value` is a valid out-pointer for the duration of the call.
        let err = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
        if err != AXError::Success {
            return None;
        }
        let value = NonNull::new(value as *mut CFType)?;
        // SAFETY: AX returned a +1 reference that nobody else will release.
        Some(unsafe { CFRetained::from_raw(value) })
    }

    fn elements(element: &AXUIElement, name: &str) -> Vec<CFRetained<AXUIElement>> {
        let Some(array) = attr(element, name).and_then(|v| v.downcast::<CFArray>().ok()) else {
            return Vec::new();
        };
        // SAFETY: AX array attributes such as AXWindows hold AXUIElements.
        let array: CFRetained<CFArray<AXUIElement>> = unsafe { CFRetained::cast_unchecked(array) };
        array.iter().collect()
    }

    fn frame(element: &AXUIElement) -> Option<CGRect> {
        let pos = attr(element, "AXPosition")?.downcast::<AXValue>().ok()?;
        let size = attr(element, "AXSize")?.downcast::<AXValue>().ok()?;
        let mut p = CGPoint::default();
        let mut s = CGSize::default();
        // SAFETY: `p` is a valid CGPoint to write into, matching the requested type.
        let ok_p =
            unsafe { pos.value(AXValueType::CGPoint, NonNull::from(&mut p).cast::<c_void>()) };
        // SAFETY: `s` is a valid CGSize to write into, matching the requested type.
        let ok_s =
            unsafe { size.value(AXValueType::CGSize, NonNull::from(&mut s).cast::<c_void>()) };
        (ok_p && ok_s).then_some(CGRect { origin: p, size: s })
    }
}
