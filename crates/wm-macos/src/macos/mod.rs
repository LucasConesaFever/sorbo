//! `MacPlatform`: the `wm_core::Platform` implementation for macOS (spec 3.2).
//!
//! Everything runs on the main thread. Event sources are attached to the main run loop;
//! their callbacks push `Event`s into `Shared::queue`. `next_event` runs the run loop until
//! the queue has something. `execute` calls macOS directly.
//!
//! Callbacks reach the shared state through a raw pointer (`info_ptr`). That's sound because
//! the shared state is never freed: `MacPlatform::new` leaks it (`Box::leak`), since the OS
//! registrations (timers, observers, hotkeys, the event tap, signal sources) are never removed
//! and may fire for the rest of the process. Create `MacPlatform` once per process.
//!
//! Every callback goes through `run_or_defer` or `run_or_retry` (HARD-6). They turn the
//! pointer back into the state in one place, never borrow it twice (a callback fired from a
//! nested run loop while it's borrowed is done later instead), and catch panics so none
//! unwinds into C.

mod ax;
mod banner;
mod config_watch;
mod displays;
mod focus;
mod hotkeys;
mod launch;
pub(crate) mod logfile;
mod observers;
mod pointer;
mod ports;
mod signals;
mod statusbar;
mod window_list;
mod windows;
mod workspace;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_application_services::{AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext,
    kCFRunLoopCommonModes,
};
use wm_core::{Command, Config, ConfigError, Event, Platform, WindowId};

use crate::echo::Debounce;
use crate::guard::{Ran, retry_after, run_guarded};
use crate::wm_log;
use displays::Display;
use windows::WindowTable;

/// When to redraw the banner after a runtime error, so its 5 seconds are over by then.
const RUNTIME_ERROR_REFRESH: Duration = Duration::from_millis(5100);

/// A runtime error's time is up: redraw the banner without it.
unsafe extern "C-unwind" fn on_banner_timer(timer: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "banner timer", banner::refresh) };
}

/// Subtype that marks our own wake-up events.
const WAKE_SUBTYPE: i16 = 0x6d77;

/// The signature CoreFoundation timers call.
type TimerCallback = unsafe extern "C-unwind" fn(*mut CFRunLoopTimer, *mut c_void);

/// State shared between `MacPlatform` and the run-loop callbacks.
pub(crate) struct Shared {
    pub mtm: MainThreadMarker,
    pub queue: VecDeque<Event>,
    pub displays: Vec<Display>,
    pub debounce: Debounce,
    pub display_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// Holds back snapshots that lose a monitor for a moment (MON-6).
    pub removal_grace: crate::echo::RemovalGrace,
    /// When macOS last reported a display change.
    pub last_display_change: Option<Instant>,
    /// Windows that vanished during a display change, waiting to come back (MON-7).
    pub lost: crate::lost::LostWindows,
    pub lost_timer: Option<CFRetained<CFRunLoopTimer>>,
    pub windows: WindowTable,
    /// One Accessibility observer per observed app (spec 5.7).
    pub observers: HashMap<i32, CFRetained<AXObserver>>,
    /// Windows in native fullscreen: reported gone, re-reported when they leave it.
    pub fullscreen: Vec<(i32, CFRetained<AXUIElement>)>,
    /// Apps already logged as over the per-app window limit (SAFE-4), so it's said once.
    pub over_window_limit: HashSet<i32>,
    /// Apps skipped by the pointer hit test after overrunning its budget (HARD-5).
    pub slow_apps: crate::limits::SlowApps,
    /// Popup-classified windows to look at again, and when (spec 5.2, late classification).
    /// Popup-classified windows to classify again: (id, when it appeared, checks done).
    pub reclassify: Vec<(WindowId, Instant, u8)>,
    pub reclassify_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// Windows whose reported frame the window server didn't confirm yet: when to look again
    /// and how many re-checks are done (SAFE-3).
    pub reconfirm: Vec<(WindowId, Instant, u8)>,
    pub reconfirm_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// Hides that failed, to try again: (id, when, retries done) (HARD-3).
    pub hide_retry: Vec<(WindowId, Instant, u8)>,
    pub hide_retry_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// The window server's window list, reused for a moment (SAFE-3).
    pub server_bounds: window_list::Cached,
    /// The on-screen windows the pointer hit test checks first (HARD-5).
    pub on_screen: window_list::OnScreen,
    /// Windows that moved or resized and haven't settled yet (spec 5.8).
    pub settle: crate::echo::SettleQueue,
    pub frame_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// Launched apps Accessibility wasn't ready for yet, with how many retries so far.
    pub pending_launch: Vec<(i32, u32)>,
    pub launch_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// Latest pointer position not hit-tested yet, and the last one that was (spec 5.3).
    pub pending_pointer: Option<wm_core::Point>,
    pub last_hit: Option<wm_core::Point>,
    pub event_tap: Option<CFRetained<objc2_core_foundation::CFMachPort>>,
    pub poll_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// The action of each registered hotkey, by its Carbon hotkey id.
    pub hotkeys: crate::keys::HotkeyActions,
    /// The registered hotkeys, to unregister them when the config changes.
    pub hotkey_refs: Vec<hotkeys::Hotkey>,
    /// The resize mode's hotkeys, registered only while the mode is on (MODE-5).
    pub mode_hotkeys: Vec<hotkeys::Hotkey>,
    /// `~/.config/sorbo/sorbo.toml`.
    pub config_path: PathBuf,
    /// The config in effect.
    pub config: Config,
    /// The file's text when it was last loaded (`None`: no file), to skip unchanged saves.
    pub config_text: crate::config_load::FileText,
    /// The config's problems: the file's own, plus the bindings macOS refused (CFG-6).
    pub config_errors: Vec<ConfigError>,
    /// The bindings macOS refused when the current bindings were registered.
    pub refused: Vec<ConfigError>,
    /// Debounces file events into one reload.
    pub config_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// What the error banner and the menu bar dot show (CFG-6).
    pub banner_state: crate::banner_state::BannerState,
    /// The error banner; `None` only while `MacPlatform::new` runs.
    pub banner: Option<banner::Banner>,
    /// Re-renders the banner when a runtime error expires.
    pub banner_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// The workspace row in the menu bar; `None` only while `MacPlatform::new` runs.
    pub status_bar: Option<statusbar::StatusBar>,
    /// Set when the WM should exit; `next_event` then returns `None`.
    pub quit: bool,
    /// The last `Focus` we sent, until its app confirms it (`focus_guard`).
    pub pending_focus: Option<crate::focus_guard::PendingFocus>,
    /// An activation waiting to see whether the previous app quits (FOC-8).
    pub held_activation: Option<crate::focus_guard::HeldActivation>,
    pub activation_timer: Option<CFRetained<CFRunLoopTimer>>,
    /// The frontmost app, from `NSWorkspace`'s activation notifications.
    pub active_pid: Option<i32>,
    /// True while a wake-up event is queued in AppKit, so we post at most one.
    pub wake_posted: bool,
    /// `info_ptr` of the `RefCell` holding this struct: the `info` pointer callbacks
    /// registered later (per-window AX notifications) must receive.
    pub self_ptr: *mut c_void,
}

impl Shared {
    /// Logs and queues an event, and wakes `next_event` so it sees it.
    pub fn push(&mut self, event: Event) {
        wm_log!("event {event:?}");
        self.queue.push_back(event);
        self.wake();
    }

    /// Makes `pump` return now. AppKit keeps waiting for an *event* even after a run-loop
    /// source fires (stopping the run loop isn't enough: measured, 1 s lag), so we post an
    /// application-defined event to ourselves. At most one is queued at a time.
    pub fn wake(&mut self) {
        use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
        use objc2_foundation::NSPoint;

        if self.wake_posted {
            return;
        }
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            WAKE_SUBTYPE,
            0,
            0,
        );
        if let Some(event) = event {
            NSApplication::sharedApplication(self.mtm).postEvent_atStart(&event, false);
            self.wake_posted = true;
        }
    }
}

/// The `info` pointer every OS registration gets: the address of the (leaked) shared state.
pub(crate) fn info_ptr(shared: &'static RefCell<Shared>) -> *mut c_void {
    (shared as *const RefCell<Shared>).cast_mut().cast()
}

/// Turns a callback's `info` pointer back into the shared state. The only place that's done.
///
/// # Safety
/// `info` must come from `info_ptr`, and the call must be on the main thread (the state is
/// a `RefCell`, and the deferred work lives in a thread-local).
unsafe fn shared_from(info: *const c_void) -> &'static RefCell<Shared> {
    // SAFETY: `info_ptr` points at the shared state, which is leaked and so lives for the
    // rest of the process; callbacks only run on the main thread.
    unsafe { &*(info as *const RefCell<Shared>) }
}

/// A callback's work. It owns everything it needs, so it can wait until sorbo is free.
type Work = Box<dyn FnOnce(&mut Shared)>;

thread_local! {
    /// Callback work that arrived while the shared state was borrowed; `next_event` runs it
    /// once the state is free (HARD-6).
    static DEFERRED: RefCell<Vec<Work>> = const { RefCell::new(Vec::new()) };
}

/// For a callback: does `work` now, or, if sorbo is busy, as soon as it's free (HARD-6).
///
/// # Safety
/// `info` must come from `info_ptr`; main thread only. Every registration sorbo makes is on
/// the main run loop or the main dispatch queue.
pub(crate) unsafe fn run_or_defer(
    info: *const c_void,
    what: &'static str,
    work: impl FnOnce(&mut Shared) + 'static,
) {
    // SAFETY: guaranteed by the caller.
    let shared = unsafe { shared_from(info) };
    if shared.try_borrow_mut().is_err() {
        wm_log!("{what}: sorbo was busy; doing it once it's free (HARD-6)");
        DEFERRED.with(|d| d.borrow_mut().push(Box::new(work)));
        return;
    }
    report(what, run_guarded(shared, work));
}

/// For a timer callback: does `work` now, or, if sorbo is busy, fires `timer` again shortly.
///
/// # Safety
/// `info` must come from `info_ptr`; `timer` is the timer CoreFoundation is firing; main
/// thread only.
pub(crate) unsafe fn run_or_retry(
    timer: *mut CFRunLoopTimer,
    info: *const c_void,
    what: &'static str,
    work: impl FnOnce(&mut Shared),
) {
    // SAFETY: guaranteed by the caller.
    let shared = unsafe { shared_from(info) };
    let ran = run_guarded(shared, work);
    // SAFETY: CFRunLoop passes the timer that is firing, alive during the call.
    if let Some(after) = retry_after(&ran)
        && let Some(timer) = unsafe { timer.as_ref() }
    {
        arm(timer, after);
    }
    report(what, ran);
}

/// Runs the work callbacks left while sorbo was busy. Called with the state free.
fn run_deferred(shared: &RefCell<Shared>) {
    let work = DEFERRED.with(|d| std::mem::take(&mut *d.borrow_mut()));
    for w in work {
        report("deferred callback", run_guarded(shared, w));
    }
}

fn report<R>(what: &str, ran: Ran<R>) {
    if let Ran::Panicked = ran {
        wm_log!("{what}: panicked (message above); sorbo carries on (HARD-6)");
    }
}

/// A timer on the main run loop that only fires when armed with `arm`. It repeats with a
/// huge interval, so firing never invalidates it and it can be re-armed forever.
pub(crate) fn make_timer(
    shared: &'static RefCell<Shared>,
    callback: TimerCallback,
) -> Result<CFRetained<CFRunLoopTimer>, String> {
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: info_ptr(shared),
        retain: None,
        release: None,
        copyDescription: None,
    };
    let far_future = CFAbsoluteTimeGetCurrent() + 1.0e9;
    // SAFETY: `context` is valid for the call (CF copies it); its `info` points at the
    // shared state, which `MacPlatform` keeps alive as long as the timer exists.
    let timer =
        unsafe { CFRunLoopTimer::new(None, far_future, 1.0e9, 0, 0, Some(callback), &mut context) }
            .ok_or("could not create a run-loop timer")?;
    let run_loop = CFRunLoop::main().ok_or("no main run loop")?;
    // SAFETY: reading an extern static provided by CoreFoundation.
    run_loop.add_timer(Some(&timer), unsafe { kCFRunLoopCommonModes });
    Ok(timer)
}

/// Makes `timer` fire once, `after` from now.
pub(crate) fn arm(timer: &CFRunLoopTimer, after: Duration) {
    timer.set_next_fire_date(CFAbsoluteTimeGetCurrent() + after.as_secs_f64());
}

/// Stops `timer` from firing until it's armed again.
pub(crate) fn disarm(timer: &CFRunLoopTimer) {
    timer.set_next_fire_date(CFAbsoluteTimeGetCurrent() + 1.0e9);
}

/// Waits for the next AppKit event (up to 1 s) and dispatches it.
///
/// Carbon hotkeys are delivered as AppKit events, so a plain `CFRunLoopRunInMode` isn't
/// enough: AppKit has to fetch and dispatch them. Fetching runs the run loop, so every
/// other source (AX observers, timers, the event tap) fires in here too; they end the wait
/// early with `Shared::wake`.
fn pump(mtm: MainThreadMarker) {
    use objc2_app_kit::{NSApplication, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    let app = NSApplication::sharedApplication(mtm);
    let until = NSDate::dateWithTimeIntervalSinceNow(1.0);
    // SAFETY: reading an extern static provided by Foundation.
    let mode = unsafe { NSDefaultRunLoopMode };
    if let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        Some(&until),
        mode,
        true,
    ) {
        app.sendEvent(&event);
    }
}

/// True if this process may use Accessibility. With `prompt`, macOS shows its permission
/// dialog when it isn't.
pub fn accessibility_trusted(prompt: bool) -> bool {
    use objc2_application_services::{AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType};

    // SAFETY: reading an extern static provided by ApplicationServices.
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let value: &CFType = CFBoolean::new(prompt);
    let options = CFDictionary::<CFString, CFType>::from_slices(&[key], &[value]);
    // SAFETY: `options` is a valid dictionary with the documented key and a CFBoolean value.
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
}

/// The macOS platform. Create it on the main thread.
pub struct MacPlatform {
    mtm: MainThreadMarker,
    shared: &'static RefCell<Shared>,
    /// Set once hidden windows were put back, so `Drop` doesn't do it again.
    restored: bool,
    /// Kept alive so NSWorkspace keeps delivering app notifications to it.
    _workspace_observer: Retained<workspace::WorkspaceObserver>,
}

impl MacPlatform {
    /// `config_path` is the config file (`config_load::config_path`); it may not exist.
    pub fn new(config_path: PathBuf) -> Result<MacPlatform, String> {
        let mtm =
            MainThreadMarker::new().ok_or("MacPlatform must be created on the main thread")?;
        // NSScreen reports a wrong visible frame for secondary displays (menu bar not
        // excluded) until the shared NSApplication exists, so create it first.
        let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        // No Dock icon or menu bar for the WM; then let AppKit finish starting so it
        // dispatches events (Carbon hotkeys arrive as events, spec 5.5).
        app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        let err = ax::set_global_timeout(0.05);
        wm_log!("AX global timeout 50 ms: {err:?}");

        let displays = displays::snapshot(mtm);
        let config_path_for_banner = config_path.clone();
        // Leaked on purpose: the OS keeps pointers to it for the rest of the process (see the
        // module comment), so `'static` is simply true.
        let shared: &'static RefCell<Shared> = Box::leak(Box::new(RefCell::new(Shared {
            mtm,
            queue: VecDeque::new(),
            displays,
            debounce: Debounce::new(),
            display_timer: None,
            removal_grace: crate::echo::RemovalGrace::new(),
            last_display_change: None,
            lost: crate::lost::LostWindows::default(),
            lost_timer: None,
            windows: WindowTable::new(),
            observers: HashMap::new(),
            fullscreen: Vec::new(),
            over_window_limit: HashSet::new(),
            slow_apps: crate::limits::SlowApps::default(),
            reclassify: Vec::new(),
            reclassify_timer: None,
            reconfirm: Vec::new(),
            reconfirm_timer: None,
            hide_retry: Vec::new(),
            hide_retry_timer: None,
            server_bounds: None,
            on_screen: None,
            settle: crate::echo::SettleQueue::new(),
            frame_timer: None,
            pending_launch: Vec::new(),
            launch_timer: None,
            pending_pointer: None,
            last_hit: None,
            event_tap: None,
            poll_timer: None,
            hotkeys: crate::keys::HotkeyActions::default(),
            hotkey_refs: Vec::new(),
            mode_hotkeys: Vec::new(),
            config_path: config_path.clone(),
            // Replaced by the loaded config in `config_watch::install`.
            config: wm_core::default_config(),
            config_text: crate::config_load::FileText::Missing,
            config_errors: Vec::new(),
            refused: Vec::new(),
            config_timer: None,
            banner_state: crate::banner_state::BannerState::new(&crate::config_load::file_name(
                &config_path_for_banner,
            )),
            banner: None,
            banner_timer: None,
            status_bar: None,
            quit: false,
            wake_posted: false,
            pending_focus: None,
            held_activation: None,
            activation_timer: None,
            active_pid: objc2_app_kit::NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .map(|app| app.processIdentifier()),
            self_ptr: std::ptr::null_mut(),
        })));
        shared.borrow_mut().self_ptr = info_ptr(shared);
        {
            let info = info_ptr(shared).cast_const();
            let mut s = shared.borrow_mut();
            s.status_bar = Some(statusbar::StatusBar::new(mtm, info));
            s.banner = Some(banner::Banner::new(mtm, info));
        }
        let banner_timer = make_timer(shared, on_banner_timer)?;
        let activation_timer = make_timer(shared, focus::on_activation_timer)?;
        let lost_timer = make_timer(shared, observers::on_lost_timer)?;
        let display_timer = make_timer(shared, displays::on_settle_timer)?;
        let reclassify_timer = make_timer(shared, observers::on_reclassify_timer)?;
        let reconfirm_timer = make_timer(shared, observers::on_reconfirm_timer)?;
        let hide_retry_timer = make_timer(shared, observers::on_hide_retry_timer)?;
        let launch_timer = make_timer(shared, observers::on_launch_timer)?;
        let frame_timer = make_timer(shared, observers::on_frame_settle_timer)?;
        {
            let mut s = shared.borrow_mut();
            s.frame_timer = Some(frame_timer);
            s.display_timer = Some(display_timer);
            s.reclassify_timer = Some(reclassify_timer);
            s.reconfirm_timer = Some(reconfirm_timer);
            s.hide_retry_timer = Some(hide_retry_timer);
            s.launch_timer = Some(launch_timer);
            s.banner_timer = Some(banner_timer);
            s.activation_timer = Some(activation_timer);
            s.lost_timer = Some(lost_timer);
        }
        displays::install(shared)?;
        pointer::install(shared)?;
        hotkeys::install_handler(shared);
        // Before `MonitorsChanged` below, so the configured gaps apply to the first layout.
        if let Err(err) = config_watch::install(shared) {
            wm_log!("config: {err}");
        }
        signals::install(shared);

        // Start listening before listing windows, so a window created during startup isn't
        // missed; windows already tracked are ignored by the creation handlers (spec 5.7).
        let workspace_observer = workspace::install(shared);
        let apps = windows::regular_apps();
        for &(pid, _) in &apps {
            // An app that refuses now (common at login, while apps are still starting) still
            // gets its windows listed below, and is retried until it accepts.
            let mut s = shared.borrow_mut();
            if !observers::observe_app(&mut s, pid) {
                observers::retry_observe(&mut s, pid);
            }
        }
        {
            let mut s = shared.borrow_mut();
            let monitors = displays::monitor_infos(&s.displays);
            s.push(Event::MonitorsChanged(monitors));
            let mut discovered = Vec::new();
            // Windows of hidden apps are reported when the app is unhidden.
            for &(pid, hidden) in &apps {
                if !hidden {
                    discovered.extend(observers::track_app_windows(&mut s, pid));
                }
            }
            s.push(Event::WindowsDiscovered(discovered));
        }
        Ok(MacPlatform {
            mtm,
            shared,
            restored: false,
            _workspace_observer: workspace_observer,
        })
    }
}

impl MacPlatform {
    /// Shows every window the WM hid, centered on the display it was hidden from. Call it
    /// after `wm_core::run` returns, so quitting doesn't leave windows parked in corners.
    pub fn restore_hidden_windows(&mut self) {
        let Ok(mut s) = self.shared.try_borrow_mut() else {
            wm_log!("could not put hidden windows back: the state is busy");
            return;
        };
        let s = &mut *s;
        windows::restore_hidden(&mut s.windows, &s.displays);
        self.restored = true;
    }
}

/// If sorbo stops by panicking out of `wm_core::run`, the platform is dropped while the
/// panic unwinds: put hidden windows back then too, instead of leaving them parked in
/// corners until the next start (HARD-6).
impl Drop for MacPlatform {
    fn drop(&mut self) {
        if !self.restored {
            wm_log!("stopping without a clean exit; putting hidden windows back");
            self.restore_hidden_windows();
        }
    }
}

impl Platform for MacPlatform {
    fn next_event(&mut self) -> Option<Event> {
        loop {
            run_deferred(self.shared);
            {
                let mut s = self.shared.borrow_mut();
                if s.quit {
                    return None;
                }
                if let Some(event) = s.queue.pop_front() {
                    return Some(event);
                }
                if let Some((position, target)) = pointer::take_pointer_event(&mut s) {
                    let event = Event::PointerMoved { position, target };
                    wm_log!("event {event:?}");
                    return Some(event);
                }
            } // The borrow ends here: callbacks may run while we wait below.
            pump(self.mtm);
            self.shared.borrow_mut().wake_posted = false;
        }
    }

    fn execute(&mut self, command: Command) {
        wm_log!("execute {command:?}");
        let mut s = self.shared.borrow_mut();
        // Reborrow as a plain `&mut Shared` so we can borrow two fields at once.
        let s = &mut *s;
        match command {
            Command::SetFrame { id, frame } => {
                windows::set_frame(&mut s.windows, id, frame);
                observers::check_frame_later(s, id);
            }
            Command::Hide(id) => observers::hide_or_retry(s, id),
            Command::Focus(id) => focus::focus(s, id),
            Command::WarpPointer(p) => focus::warp(p),
            Command::Close(id) => launch::close(&s.windows, id, s.active_pid),
            Command::LaunchNewWindow(app) => {
                if let Err(message) = launch::launch_new_window(&app) {
                    // A missing app is about this keypress, not the file: shown for a few
                    // seconds, no dot (spec section 6).
                    wm_log!("config: {message}");
                    s.banner_state.runtime_error(message, Instant::now());
                    banner::refresh(s);
                    if let Some(timer) = &s.banner_timer {
                        arm(timer, RUNTIME_ERROR_REFRESH);
                    }
                }
            }
            Command::ModeChanged(mode) => {
                hotkeys::set_mode(s, mode);
                if let Some(bar) = &s.status_bar {
                    bar.set_mode(mode);
                }
            }
            Command::ShowWorkspaces(row) => {
                if let Some(bar) = &s.status_bar {
                    bar.show(row);
                }
            }
        }
    }
}
