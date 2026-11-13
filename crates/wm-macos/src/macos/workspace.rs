//! App-level notifications from `NSWorkspace`: launch, quit, hide, unhide (spec 5.7), and
//! the screens sleeping and waking (MON-7, D52).
//!
//! `NSNotificationCenter` calls an Objective-C method on an object, so we define a tiny
//! Objective-C class with `objc2::define_class!`. (The block-based API would need the
//! `block2` crate as a new dependency.)

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidHideApplicationNotification,
    NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    NSWorkspaceDidUnhideApplicationNotification, NSWorkspaceScreensDidSleepNotification,
    NSWorkspaceScreensDidWakeNotification,
};
use objc2_foundation::{NSNotification, NSNotificationName, NSObject};
use wm_core::Event;

use super::{Shared, focus, info_ptr, observers, run_or_defer};

/// The observer's only state: the shared-state pointer (see `mod.rs`).
pub(crate) struct Ivars {
    shared: *const c_void,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SorboWorkspaceObserver"]
    #[ivars = Ivars]
    pub(crate) struct WorkspaceObserver;

    impl WorkspaceObserver {
        #[unsafe(method(appLaunched:))]
        fn app_launched(&self, notification: &NSNotification) {
            let Some(pid) = regular_pid(notification) else { return };
            self.run("app launched", move |s| observers::app_launched(s, pid));
        }

        #[unsafe(method(appTerminated:))]
        fn app_terminated(&self, notification: &NSNotification) {
            let Some(pid) = regular_pid(notification) else { return };
            self.run("app quit", move |s| app_terminated(s, pid));
        }

        #[unsafe(method(appActivated:))]
        fn app_activated(&self, notification: &NSNotification) {
            let Some(pid) = regular_pid(notification) else { return };
            self.run("app activated", move |s| {
                let previous = s.active_pid.replace(pid);
                focus::report_activation(s, pid, previous);
            });
        }

        #[unsafe(method(appHidden:))]
        fn app_hidden(&self, notification: &NSNotification) {
            let Some(pid) = regular_pid(notification) else { return };
            self.run("app hidden", move |s| forget_windows(s, pid));
        }

        #[unsafe(method(appUnhidden:))]
        fn app_unhidden(&self, notification: &NSNotification) {
            let Some(pid) = regular_pid(notification) else { return };
            self.run("app unhidden", move |s| {
                // Normally already observed; this is a safety net (observe_app is idempotent).
                observers::observe_app(s, pid);
                for info in observers::track_app_windows(s, pid) {
                    s.push(Event::WindowCreated(info));
                }
            });
        }

        #[unsafe(method(screensSlept:))]
        fn screens_slept(&self, _notification: &NSNotification) {
            self.run("screens slept", observers::screens_slept);
        }

        #[unsafe(method(screensWoke:))]
        fn screens_woke(&self, _notification: &NSNotification) {
            self.run("screens woke", observers::screens_woke);
        }
    }
);

impl WorkspaceObserver {
    /// Does a notification's work on the shared state, now or once sorbo is free (HARD-6).
    fn run(&self, what: &'static str, work: impl FnOnce(&mut Shared) + 'static) {
        // SAFETY: the pointer was set in `install` with `info_ptr`.
        unsafe { run_or_defer(self.ivars().shared, what, work) };
    }
}

/// App `pid` quit: forget everything about it.
fn app_terminated(s: &mut Shared, pid: i32) {
    focus::app_quit(s, pid);
    forget_windows(s, pid);
    // Its lost windows won't come back (MON-7).
    for id in s.lost.take_pid(pid) {
        s.push(Event::WindowDestroyed(id));
    }
    observers::unobserve_app(s, pid);
    s.pending_launch.retain(|(p, _)| *p != pid);
    // Its fullscreen windows won't leave fullscreen any more (SAFE-9).
    s.fullscreen.retain(|(p, _)| *p != pid);
    s.over_window_limit.remove(&pid);
    s.slow_apps.forget(pid);
}

/// Creates the observer and subscribes it to the app notifications.
pub(crate) fn install(shared: &'static RefCell<Shared>) -> Retained<WorkspaceObserver> {
    let mtm = shared.borrow().mtm;
    let this = WorkspaceObserver::alloc(mtm).set_ivars(Ivars {
        shared: info_ptr(shared).cast_const(),
    });
    // SAFETY: calling NSObject's designated initializer on a freshly allocated object.
    let observer: Retained<WorkspaceObserver> = unsafe { msg_send![super(this), init] };

    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: reading extern statics provided by AppKit.
    let subscriptions: [(&NSNotificationName, objc2::runtime::Sel); 7] = unsafe {
        [
            (
                NSWorkspaceDidActivateApplicationNotification,
                sel!(appActivated:),
            ),
            (
                NSWorkspaceDidLaunchApplicationNotification,
                sel!(appLaunched:),
            ),
            (
                NSWorkspaceDidTerminateApplicationNotification,
                sel!(appTerminated:),
            ),
            (NSWorkspaceDidHideApplicationNotification, sel!(appHidden:)),
            (
                NSWorkspaceDidUnhideApplicationNotification,
                sel!(appUnhidden:),
            ),
            (NSWorkspaceScreensDidSleepNotification, sel!(screensSlept:)),
            (NSWorkspaceScreensDidWakeNotification, sel!(screensWoke:)),
        ]
    };
    for (name, selector) in subscriptions {
        let target: &AnyObject = &observer;
        // SAFETY: each selector names a method defined above that takes one NSNotification;
        // the observer stays alive (owned by `MacPlatform`) while it's registered.
        unsafe { center.addObserver_selector_name_object(target, selector, Some(name), None) };
    }
    observer
}

/// The pid of the app a notification is about, if it's a regular (Dock) app.
fn regular_pid(notification: &NSNotification) -> Option<i32> {
    let info = notification.userInfo()?;
    // SAFETY: reading an extern static provided by AppKit.
    let key: &AnyObject = unsafe { NSWorkspaceApplicationKey };
    let app = info
        .objectForKey(key)?
        .downcast::<NSRunningApplication>()
        .ok()?;
    (app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .then(|| app.processIdentifier())
}

/// Tells core every tracked window of `pid` is gone.
fn forget_windows(s: &mut Shared, pid: i32) {
    for id in s.windows.ids_for_pid(pid) {
        observers::untrack(s, id);
    }
}
