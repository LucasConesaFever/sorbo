//! Spike: can a background process focus a specific window of another app on this macOS?
//!
//! Since macOS 14 ("cooperative activation") the system may refuse to bring an app to the
//! front when the request doesn't come from the active app. The `Focus` command (spec 5.2)
//! depends on this working, so we test it before designing around it.
//!
//! Usage (quit AeroSpace first, and grant Accessibility to the terminal you run it from):
//!
//!     cargo run -p wm-macos --example spike_focus -- <ax|ns|both|ax-after> [rounds]
//!
//! Methods:
//! - `ax`:       window AXMain = true, AXRaise on the window, app AXFrontmost = true.
//! - `ns`:       window AXMain = true, AXRaise on the window, NSRunningApplication::activateWithOptions.
//! - `both`:     everything above.
//! - `ax-after`: app AXFrontmost = true first, then window AXMain = true and AXRaise.
//!
//! The spike cycles focus through every standard window of every regular app on the current
//! Space, and after each attempt checks (through AX, not NSWorkspace, whose cached state only
//! refreshes inside a run loop) which app and which window really have focus.

#[cfg(target_os = "macos")]
fn main() {
    spike::main();
}

// Examples are built by `cargo test` too, so this file must compile on Linux.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod spike {
    use std::ptr::NonNull;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use objc2_app_kit::{
        NSApplicationActivationOptions, NSApplicationActivationPolicy, NSWorkspace,
    };
    use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement};
    use objc2_core_foundation::{CFArray, CFBoolean, CFEqual, CFRetained, CFString, CFType};

    #[derive(Clone, Copy, PartialEq)]
    enum Method {
        Ax,
        Ns,
        Both,
        AxAfter,
    }

    /// One window we can try to focus.
    struct Target {
        pid: i32,
        app_name: String,
        title: String,
        window: CFRetained<AXUIElement>,
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let method = match args.get(1).map(String::as_str) {
            Some("ax") => Method::Ax,
            Some("ns") => Method::Ns,
            Some("both") => Method::Both,
            Some("ax-after") => Method::AxAfter,
            _ => {
                eprintln!("usage: spike_focus <ax|ns|both|ax-after> [rounds]");
                std::process::exit(2);
            }
        };
        let rounds: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);

        // SAFETY: no arguments, no preconditions.
        if !unsafe { AXIsProcessTrusted() } {
            eprintln!(
                "Not trusted for Accessibility. Grant it to your terminal app in \
                 System Settings > Privacy & Security > Accessibility, then rerun."
            );
            std::process::exit(1);
        }

        let targets = collect_targets();
        if targets.len() < 2 {
            eprintln!(
                "Need at least two standard windows on screen (ideally 3+ apps, and 2 windows of one app)."
            );
            std::process::exit(1);
        }
        println!("Found {} windows:", targets.len());
        for (i, t) in targets.iter().enumerate() {
            println!("  [{i}] {} (pid {}) - {:?}", t.app_name, t.pid, t.title);
        }
        println!("\nStarting in 3 s. Don't touch the keyboard or mouse.\n");
        sleep(Duration::from_secs(3));

        let mut ok = 0;
        let mut total = 0;
        for round in 0..rounds {
            for t in &targets {
                total += 1;
                let started = Instant::now();
                focus(t, method);
                // Give the target app time to process the requests.
                sleep(Duration::from_millis(400));
                let (app_ok, win_ok) = check(t);
                if app_ok && win_ok {
                    ok += 1;
                }
                println!(
                    "round {round}  {:<20} app_front={:<5} window_focused={:<5} ({} ms incl. wait)",
                    truncate(&format!("{} {:?}", t.app_name, t.title), 20),
                    app_ok,
                    win_ok,
                    started.elapsed().as_millis()
                );
            }
        }
        println!("\n{ok}/{total} attempts focused the right app AND window.");
    }

    fn focus(t: &Target, method: Method) {
        let app = app_element(t.pid);
        if method == Method::AxAfter {
            // Activate the app first: on activation an app may restore its own last key
            // window, so we only pick the window once the app is already in front.
            let _ = set_attr(&app, "AXFrontmost", CFBoolean::new(true));
            let _ = set_attr(&t.window, "AXMain", CFBoolean::new(true));
            let _ = perform(&t.window, "AXRaise");
            return;
        }
        // AXMain makes this the app's main window; AXRaise brings it above the app's
        // other windows. Neither one activates the app by itself.
        let _ = set_attr(&t.window, "AXMain", CFBoolean::new(true));
        let _ = perform(&t.window, "AXRaise");
        if method == Method::Ax || method == Method::Both {
            let _ = set_attr(&app, "AXFrontmost", CFBoolean::new(true));
        }
        if method == Method::Ns || method == Method::Both {
            let running =
                objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(t.pid);
            if let Some(running) = running {
                let accepted = running.activateWithOptions(NSApplicationActivationOptions::empty());
                if !accepted {
                    println!("    activateWithOptions returned NO");
                }
            }
        }
    }

    /// (is the target's app frontmost?, is the target the app's focused window?)
    fn check(t: &Target) -> (bool, bool) {
        // SAFETY: creating the system-wide element has no preconditions.
        let system = unsafe { AXUIElement::new_system_wide() };
        let ax_front_pid = attr(&system, "AXFocusedApplication")
            .and_then(|v| v.downcast::<AXUIElement>().ok())
            .and_then(|app| element_pid(&app));
        // AXFocusedApplication turned out to be unreliable here, so the verdict comes from
        // Launch Services (`lsappinfo`), which is live even without a run loop.
        let front_pid = lsappinfo_front_pid();
        if ax_front_pid != front_pid {
            println!("    AX says front pid {ax_front_pid:?}, lsappinfo says {front_pid:?}");
        }
        let app_ok = front_pid == Some(t.pid);

        let win_ok = attr(&app_element(t.pid), "AXFocusedWindow")
            .map(|w| {
                let w: &CFType = &w;
                let target: &CFType = &t.window;
                CFEqual(Some(w), Some(target))
            })
            .unwrap_or(false);
        (app_ok, win_ok)
    }

    fn collect_targets() -> Vec<Target> {
        let me = std::process::id() as i32;
        let workspace = NSWorkspace::sharedWorkspace();
        let mut targets = Vec::new();
        for app in workspace.runningApplications().iter() {
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                continue;
            }
            let pid = app.processIdentifier();
            if pid == me {
                continue;
            }
            let app_name = app
                .localizedName()
                .map(|n| n.to_string())
                .unwrap_or_default();
            for window in windows(pid) {
                let subrole = string_attr(&window, "AXSubrole").unwrap_or_default();
                let minimized = bool_attr(&window, "AXMinimized").unwrap_or(false);
                if subrole != "AXStandardWindow" || minimized {
                    continue;
                }
                let title = string_attr(&window, "AXTitle").unwrap_or_default();
                targets.push(Target {
                    pid,
                    app_name: app_name.clone(),
                    title,
                    window,
                });
            }
        }
        targets
    }

    // ---- Small AX helpers. Each one wraps exactly one `unsafe` call. ----

    fn app_element(pid: i32) -> CFRetained<AXUIElement> {
        // SAFETY: any pid is accepted; a dead pid just yields an element whose calls fail.
        unsafe { AXUIElement::new_application(pid) }
    }

    fn element_pid(element: &AXUIElement) -> Option<i32> {
        let mut pid: i32 = 0;
        // SAFETY: `pid` is a valid, writable i32 for the duration of the call.
        let err = unsafe { element.pid(NonNull::from(&mut pid)) };
        (err == AXError::Success).then_some(pid)
    }

    /// Reads an attribute. AX follows Core Foundation's "Copy rule": the value comes back
    /// with a +1 reference count that we own, so we wrap it in `CFRetained`, whose `Drop`
    /// releases it.
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

    fn windows(pid: i32) -> Vec<CFRetained<AXUIElement>> {
        let Some(array) =
            attr(&app_element(pid), "AXWindows").and_then(|v| v.downcast::<CFArray>().ok())
        else {
            return Vec::new();
        };
        // SAFETY: AXWindows is documented to hold AX elements, which are all CF objects,
        // so viewing the untyped array as "array of CFType" is sound. Each element is then
        // checked with `downcast`, which verifies its real type at runtime.
        let array: &CFArray<CFType> = unsafe { array.cast_unchecked() };
        array
            .iter()
            .filter_map(|item| item.downcast::<AXUIElement>().ok())
            .collect()
    }

    fn set_attr(element: &AXUIElement, name: &str, value: &CFType) -> AXError {
        let name = CFString::from_str(name);
        // SAFETY: both references are valid CF objects for the duration of the call.
        unsafe { element.set_attribute_value(&name, value) }
    }

    fn perform(element: &AXUIElement, action: &str) -> AXError {
        let action = CFString::from_str(action);
        // SAFETY: `action` is a valid CFString for the duration of the call.
        unsafe { element.perform_action(&action) }
    }

    /// Pid of the frontmost app according to Launch Services.
    fn lsappinfo_front_pid() -> Option<i32> {
        let run = |args: &[&str]| {
            std::process::Command::new("lsappinfo")
                .args(args)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let asn = run(&["front"])?;
        // Output looks like: "pid"=1676
        let out = run(&["info", "-only", "pid", &asn])?;
        out.rsplit('=').next()?.trim().parse().ok()
    }

    fn truncate(s: &str, max: usize) -> String {
        s.chars().take(max).collect()
    }
}
