//! Spike: how far off-screen will macOS let us park a window? (spec section 9, problem 4)
//!
//! Workspaces are emulated by moving windows off-screen. For each display and each corner,
//! this asks for a position that leaves 1 px of a TextEdit window inside that display, then
//! reads back where macOS really put it (right away and after 1 s), how many pixels stay
//! visible on that display, and whether any part lands on another display.
//! The window is put back where it was at the end.
//!
//! Usage (Accessibility must be granted to the terminal; open a TextEdit document first):
//!
//!     cargo run -p wm-macos --example spike_hide

#[cfg(target_os = "macos")]
fn main() {
    spike::main();
}

// Examples are built by `cargo test` too, so this file must compile on Linux.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod spike {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::thread::sleep;
    use std::time::Duration;

    use objc2_app_kit::NSWorkspace;
    use objc2_application_services::{
        AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType,
    };
    use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGDisplayBounds, CGDisplayIsBuiltin, CGGetActiveDisplayList};

    pub fn main() {
        // SAFETY: no arguments, no preconditions.
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!("Not trusted for Accessibility. Grant it to your terminal app first.");
            std::process::exit(1);
        }

        let Some(window) = textedit_window() else {
            eprintln!("No TextEdit window found. Open a TextEdit document first.");
            std::process::exit(1);
        };
        let original_pos = point_attr(&window, "AXPosition").expect("window position");
        let size = size_attr(&window, "AXSize").expect("window size");
        println!(
            "TextEdit window at ({:.0},{:.0}), size {:.0}x{:.0}\n",
            original_pos.x, original_pos.y, size.width, size.height
        );

        let displays = displays();
        for (i, (id, d)) in displays.iter().enumerate() {
            println!(
                "Display {i} (id {id}, {}): origin ({:.0},{:.0}) size {:.0}x{:.0}",
                if CGDisplayIsBuiltin(*id) {
                    "built-in"
                } else {
                    "external"
                },
                d.origin.x,
                d.origin.y,
                d.size.width,
                d.size.height
            );
        }
        println!();

        for (i, (_, d)) in displays.iter().enumerate() {
            let left = d.origin.x;
            let top = d.origin.y;
            let right = left + d.size.width;
            let bottom = top + d.size.height;
            let (w, h) = (size.width, size.height);
            let corners = [
                ("bottom-right", CGPoint::new(right - 1.0, bottom - 1.0)),
                ("bottom-left", CGPoint::new(left - w + 1.0, bottom - 1.0)),
                ("top-right", CGPoint::new(right - 1.0, top - h + 1.0)),
                ("top-left", CGPoint::new(left - w + 1.0, top - h + 1.0)),
            ];
            for (name, want) in corners {
                set_position(&window, want);
                sleep(Duration::from_millis(300));
                let now = point_attr(&window, "AXPosition").unwrap_or(want);
                sleep(Duration::from_millis(1000));
                let later = point_attr(&window, "AXPosition").unwrap_or(want);
                let later_size = size_attr(&window, "AXSize").unwrap_or(size);
                let frame = CGRect::new(later, later_size);

                let visible_here = area(intersect(frame, *d));
                let elsewhere: f64 = displays
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, (_, other))| area(intersect(frame, *other)))
                    .sum();
                println!(
                    "display {i} {name:<12} asked ({:.0},{:.0}) got ({:.0},{:.0}) after 1s ({:.0},{:.0}) \
                     size {:.0}x{:.0} | visible here {visible_here:.0} px² | on other displays {elsewhere:.0} px²",
                    want.x,
                    want.y,
                    now.x,
                    now.y,
                    later.x,
                    later.y,
                    later_size.width,
                    later_size.height
                );
            }
        }

        set_position(&window, original_pos);
        println!(
            "\nWindow restored to ({:.0},{:.0}).",
            original_pos.x, original_pos.y
        );
    }

    fn displays() -> Vec<(u32, CGRect)> {
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        // SAFETY: `ids` has room for 16 ids and `count` is a valid out-pointer.
        unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut count) };
        ids[..count as usize]
            .iter()
            .map(|&id| (id, CGDisplayBounds(id)))
            .collect()
    }

    fn intersect(a: CGRect, b: CGRect) -> CGRect {
        let x1 = a.origin.x.max(b.origin.x);
        let y1 = a.origin.y.max(b.origin.y);
        let x2 = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
        let y2 = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
        CGRect::new(
            CGPoint::new(x1, y1),
            CGSize::new((x2 - x1).max(0.0), (y2 - y1).max(0.0)),
        )
    }

    fn area(r: CGRect) -> f64 {
        r.size.width * r.size.height
    }

    fn textedit_window() -> Option<CFRetained<AXUIElement>> {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.runningApplications().iter().find(|a| {
            a.bundleIdentifier()
                .is_some_and(|b| b.to_string() == "com.apple.TextEdit")
        })?;
        // SAFETY: any pid is accepted.
        let element = unsafe { AXUIElement::new_application(app.processIdentifier()) };
        let windows = attr(&element, "AXWindows")?.downcast::<CFArray>().ok()?;
        // SAFETY: AXWindows holds AX elements; each is checked with `downcast`.
        let windows: &CFArray<CFType> = unsafe { windows.cast_unchecked() };
        windows
            .iter()
            .filter_map(|w| w.downcast::<AXUIElement>().ok())
            .next()
    }

    // ---- Small AX helpers. Each one wraps exactly one `unsafe` call. ----

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

    fn point_attr(element: &AXUIElement, name: &str) -> Option<CGPoint> {
        let value = attr(element, name)?.downcast::<AXValue>().ok()?;
        let mut p = CGPoint::default();
        // SAFETY: `p` is a valid CGPoint to write into, matching the requested type.
        let ok =
            unsafe { value.value(AXValueType::CGPoint, NonNull::from(&mut p).cast::<c_void>()) };
        ok.then_some(p)
    }

    fn size_attr(element: &AXUIElement, name: &str) -> Option<CGSize> {
        let value = attr(element, name)?.downcast::<AXValue>().ok()?;
        let mut s = CGSize::default();
        // SAFETY: `s` is a valid CGSize to write into, matching the requested type.
        let ok =
            unsafe { value.value(AXValueType::CGSize, NonNull::from(&mut s).cast::<c_void>()) };
        ok.then_some(s)
    }

    fn set_position(element: &AXUIElement, p: CGPoint) {
        let mut p = p;
        // SAFETY: `p` is a valid CGPoint for the duration of the call.
        let Some(value) =
            (unsafe { AXValue::new(AXValueType::CGPoint, NonNull::from(&mut p).cast()) })
        else {
            return;
        };
        let name = CFString::from_str("AXPosition");
        // SAFETY: both are valid CF objects for the duration of the call.
        let _ = unsafe { element.set_attribute_value(&name, &value) };
    }
}
