//! Spike: what does macOS report under the pointer, and how fast?
//!
//! Feeds two design problems (spec section 9): hit testing (3) and window classification (2).
//! The question that matters most: when the pointer is over an extension popup or a dropdown
//! menu, can we tell it's a popup (so FOC-5 keeps it open)?
//!
//! Every tick it asks two independent sources:
//! - Accessibility: the element at the pointer, then the window it belongs to, with that
//!   window's role, subrole, title and whether it can be resized.
//! - The window server (CGWindowList): the frontmost on-screen window containing the
//!   pointer, with owner app and layer. A cross-check for windows AX may not expose.
//!
//! A line is printed only when the answer changes. Timing stats are printed at the end.
//!
//! Usage (Accessibility must be granted to the terminal):
//!
//!     cargo run -p wm-macos --example spike_pointer -- [seconds] [interval_ms]

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
    use std::ptr::NonNull;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use objc2_app_kit::NSRunningApplication;
    use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement};
    use objc2_core_foundation::{
        CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    };
    use objc2_core_graphics::{
        CGEvent, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo,
        CGWindowListOption, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowNumber,
        kCGWindowOwnerName, kCGWindowOwnerPID,
    };

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(90);
        let interval_ms: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100);

        // SAFETY: no arguments, no preconditions.
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!("Not trusted for Accessibility. Grant it to your terminal app first.");
            std::process::exit(1);
        }

        println!("Watching the pointer for {seconds} s, every {interval_ms} ms.\n");

        // SAFETY: creating the system-wide element has no preconditions.
        let system = unsafe { AXUIElement::new_system_wide() };
        let mut app_names: HashMap<i32, String> = HashMap::new();
        let mut ax_times = Vec::new();
        let mut cg_times = Vec::new();
        let mut last = String::new();
        let started = Instant::now();

        while started.elapsed() < Duration::from_secs(seconds) {
            let p = pointer();

            let t = Instant::now();
            let ax = describe_ax(&system, p, &mut app_names);
            ax_times.push(t.elapsed());

            let t = Instant::now();
            let cg = describe_cg(p);
            cg_times.push(t.elapsed());

            let line = format!("AX  {ax}\n      CG  {cg}");
            if line != last {
                println!(
                    "[{:6.1}s] pointer ({:.0}, {:.0})\n      {line}",
                    started.elapsed().as_secs_f64(),
                    p.x,
                    p.y
                );
                last = line;
            }
            sleep(Duration::from_millis(interval_ms));
        }

        println!();
        print_stats("AX hit test + describe", &mut ax_times);
        print_stats("CG window list scan  ", &mut cg_times);
    }

    /// Pointer position in global, top-left-origin coordinates (the same space as AX and
    /// CGWindowList bounds). An event created with no source carries the current location.
    fn pointer() -> CGPoint {
        let event = CGEvent::new(None);
        CGEvent::location(event.as_deref())
    }

    fn describe_ax(
        system: &AXUIElement,
        p: CGPoint,
        app_names: &mut HashMap<i32, String>,
    ) -> String {
        let mut raw: *const AXUIElement = std::ptr::null();
        // SAFETY: `raw` is a valid out-pointer for the duration of the call.
        let err = unsafe {
            system.copy_element_at_position(p.x as f32, p.y as f32, NonNull::from(&mut raw))
        };
        if err != AXError::Success {
            return format!("error {:?}", err.0);
        }
        let Some(raw) = NonNull::new(raw as *mut AXUIElement) else {
            return "no element".into();
        };
        // SAFETY: AX returned a +1 reference ("Copy rule") that we now own.
        let element = unsafe { CFRetained::from_raw(raw) };

        let pid = element_pid(&element).unwrap_or(-1);
        let app = app_names
            .entry(pid)
            .or_insert_with(|| app_name(pid))
            .clone();
        let hit = role_pair(&element);

        // The window the element lives in: the AXWindow attribute when the app provides it,
        // otherwise walk up AXParent until something has role AXWindow.
        let (window, chain) = find_window(&element);
        let window = match window {
            Some(w) => describe_window(&w),
            None => format!("NO WINDOW (ancestors: {chain})"),
        };
        format!("{app} (pid {pid}) | hit {hit} | window {window}")
    }

    fn find_window(element: &AXUIElement) -> (Option<CFRetained<AXUIElement>>, String) {
        if let Some(w) = attr(element, "AXWindow").and_then(|v| v.downcast::<AXUIElement>().ok()) {
            return (Some(w), String::new());
        }
        let mut chain = Vec::new();
        let mut current = attr(element, "AXParent").and_then(|v| v.downcast::<AXUIElement>().ok());
        for _ in 0..30 {
            let Some(el) = current else { break };
            let role = string_attr(&el, "AXRole").unwrap_or_default();
            if role == "AXWindow" {
                return (Some(el), String::new());
            }
            chain.push(role);
            current = attr(&el, "AXParent").and_then(|v| v.downcast::<AXUIElement>().ok());
        }
        (None, chain.join(" > "))
    }

    fn describe_window(w: &AXUIElement) -> String {
        let title = string_attr(w, "AXTitle").unwrap_or_default();
        let resizable = settable(w, "AXSize");
        let modal = bool_attr(w, "AXModal");
        format!(
            "{} resizable={resizable:?} modal={modal:?} title={:?}",
            role_pair(w),
            truncate(&title, 30)
        )
    }

    fn describe_cg(p: CGPoint) -> String {
        let options =
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
        let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
            return "no window list".into();
        };
        // SAFETY: the window list is documented as an array of CFDictionary.
        let list: &CFArray<CFDictionary> = unsafe { list.cast_unchecked() };
        // The list is ordered front to back, so the first window containing the point is the
        // one the user sees there.
        for dict in list.iter() {
            // SAFETY: window info dictionaries have CFString keys and CF object values.
            let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
            let Some(bounds) = rect(dict) else { continue };
            let inside = p.x >= bounds.origin.x
                && p.x < bounds.origin.x + bounds.size.width
                && p.y >= bounds.origin.y
                && p.y < bounds.origin.y + bounds.size.height;
            if !inside {
                continue;
            }
            // SAFETY (all four): the statics are valid CFStrings provided by CoreGraphics.
            let owner = dict
                .get(unsafe { kCGWindowOwnerName })
                .and_then(|v| v.downcast::<CFString>().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let pid = number(dict, unsafe { kCGWindowOwnerPID });
            let layer = number(dict, unsafe { kCGWindowLayer });
            let id = number(dict, unsafe { kCGWindowNumber });
            return format!(
                "{owner} (pid {pid}) window {id} layer {layer} at ({:.0},{:.0}) {:.0}x{:.0}",
                bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height
            );
        }
        "nothing".into()
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

    fn print_stats(label: &str, times: &mut [Duration]) {
        if times.is_empty() {
            return;
        }
        times.sort();
        let us = |d: Duration| d.as_micros();
        let total: Duration = times.iter().sum();
        println!(
            "{label}: n={} min={}µs median={}µs p95={}µs max={}µs avg={}µs",
            times.len(),
            us(times[0]),
            us(times[times.len() / 2]),
            us(times[times.len() * 95 / 100]),
            us(times[times.len() - 1]),
            us(total / times.len() as u32)
        );
    }

    // ---- Small helpers. Each one wraps exactly one `unsafe` call. ----

    fn app_name(pid: i32) -> String {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .and_then(|app| app.localizedName())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into())
    }

    fn role_pair(element: &AXUIElement) -> String {
        let role = string_attr(element, "AXRole").unwrap_or_else(|| "-".into());
        let subrole = string_attr(element, "AXSubrole").unwrap_or_else(|| "-".into());
        format!("{role}/{subrole}")
    }

    fn element_pid(element: &AXUIElement) -> Option<i32> {
        let mut pid: i32 = 0;
        // SAFETY: `pid` is a valid, writable i32 for the duration of the call.
        let err = unsafe { element.pid(NonNull::from(&mut pid)) };
        (err == AXError::Success).then_some(pid)
    }

    /// Reads an attribute. The value comes back +1 retained ("Copy rule"); `CFRetained`
    /// releases it on drop.
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

    fn string_attr(element: &AXUIElement, name: &str) -> Option<String> {
        attr(element, name)?
            .downcast::<CFString>()
            .ok()
            .map(|s| s.to_string())
    }

    fn bool_attr(element: &AXUIElement, name: &str) -> Option<bool> {
        attr(element, name)?
            .downcast::<CFBoolean>()
            .ok()
            .map(|b| b.as_bool())
    }

    fn settable(element: &AXUIElement, name: &str) -> Option<bool> {
        let name = CFString::from_str(name);
        let mut out: u8 = 0;
        // SAFETY: `out` is a valid, writable Boolean (u8) for the duration of the call.
        let err = unsafe { element.is_attribute_settable(&name, NonNull::from(&mut out)) };
        (err == AXError::Success).then_some(out != 0)
    }

    fn truncate(s: &str, max: usize) -> String {
        s.chars().take(max).collect()
    }
}
