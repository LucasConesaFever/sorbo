//! Per-app Accessibility observers: windows appearing, disappearing, minimizing, moving and
//! entering fullscreen (spec 5.7), plus late classification (spec 5.2).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2_application_services::{AXError, AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopTimer, CFString, Type, kCFRunLoopDefaultMode,
};
use wm_core::{Event, Pid, Point, Rect, UsableRect, WindowId, WindowInfo, WindowKind};

use super::windows::facts;
use super::{Shared, arm, ax, disarm, run_or_defer, run_or_retry};
use crate::classify::{classify, reclassify_after, reports_focus_change};
use crate::confirm::{confirmed_frame, next_recheck};
use crate::echo::is_echo;
use crate::hide::hide_retry_after;
use crate::lost::{Intent, LostWindow, is_display_loss, last_place};
use crate::wm_log;

/// Notifications registered on the application element.
const APP_NOTIFICATIONS: [&str; 2] = ["AXWindowCreated", "AXFocusedWindowChanged"];

/// Notifications registered on each tracked window.
const WINDOW_NOTIFICATIONS: [&str; 5] = [
    "AXUIElementDestroyed",
    "AXWindowMiniaturized",
    "AXWindowDeminiaturized",
    "AXWindowMoved",
    "AXWindowResized",
];

/// Starts observing one app. Returns false if AX refused (a just-launched app often isn't
/// ready yet); the caller may retry.
pub(crate) fn observe_app(s: &mut Shared, pid: i32) -> bool {
    if s.observers.contains_key(&pid) {
        return true;
    }
    let mut raw: *mut AXObserver = std::ptr::null_mut();
    // SAFETY: `raw` is a valid out-pointer; `on_notification` matches AXObserverCallback.
    let err = unsafe { AXObserver::create(pid, Some(on_notification), NonNull::from(&mut raw)) };
    let Some(raw) = NonNull::new(raw).filter(|_| err == AXError::Success) else {
        wm_log!("pid {pid}: could not create an AX observer: {err:?}");
        return false;
    };
    // SAFETY: AXObserverCreate follows the Create rule: we own this reference.
    let observer = unsafe { CFRetained::from_raw(raw) };
    let app = ax::app_element(pid);
    let info = s.self_ptr;
    for name in APP_NOTIFICATIONS {
        if !add_notification(&observer, &app, name, info) {
            wm_log!("pid {pid}: not ready for Accessibility notifications yet");
            return false;
        }
    }
    // SAFETY: the observer is valid; its source stays valid while the observer lives.
    let source = unsafe { observer.run_loop_source() };
    if let Some(run_loop) = CFRunLoop::main() {
        // SAFETY: reading an extern static provided by CoreFoundation.
        run_loop.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
    }
    s.observers.insert(pid, observer);
    true
}

/// Stops observing an app (it quit). Dropping the observer removes its run-loop source.
pub(crate) fn unobserve_app(s: &mut Shared, pid: i32) {
    if let Some(observer) = s.observers.remove(&pid) {
        // SAFETY: the observer is valid.
        let source = unsafe { observer.run_loop_source() };
        if let Some(run_loop) = CFRunLoop::main() {
            // SAFETY: reading an extern static provided by CoreFoundation.
            run_loop.remove_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
        }
    }
}

/// Registers one notification; false if AX refused.
fn add_notification(
    observer: &AXObserver,
    element: &AXUIElement,
    name: &str,
    info: *mut c_void,
) -> bool {
    let name = CFString::from_str(name);
    // SAFETY: all three references are valid for the call; `info` is the shared-state
    // pointer, valid as long as `MacPlatform` lives.
    let err = unsafe { observer.add_notification(element, &name, info) };
    let ok = err == AXError::Success || err == AXError::NotificationAlreadyRegistered;
    if !ok {
        wm_log!("could not observe {name}: {err:?}");
    }
    ok
}

/// What the config's window rules say about app `pid`'s windows (RULE-1).
fn app_rule(rules: &[wm_core::Rule], pid: i32) -> wm_core::RuleEffect {
    if rules.is_empty() {
        return wm_core::RuleEffect::default();
    }
    let app = objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
    let bundle = app
        .as_ref()
        .and_then(|a| a.bundleIdentifier())
        .map(|id| id.to_string());
    let name = app
        .as_ref()
        .and_then(|a| a.localizedName())
        .map(|n| n.to_string());
    wm_core::rule_effect(rules, bundle.as_deref(), name.as_deref())
}

/// What `track_window` did with a window.
pub(crate) enum Tracked {
    /// A new window: core must hear `WindowCreated`.
    New(WindowInfo),
    /// A window macOS rebuilt during a display change took over a lost window's ID; core
    /// still has it where it was (MON-7).
    Rebound(WindowId),
}

impl Tracked {
    pub(crate) fn new_window(self) -> Option<WindowInfo> {
        match self {
            Tracked::New(info) => Some(info),
            Tracked::Rebound(_) => None,
        }
    }
}

/// Classifies and tracks one window, registers its notifications, and says what core
/// needs. `None` if it isn't a window, is already tracked, or its frame can't be read.
pub(crate) fn track_window(
    s: &mut Shared,
    pid: i32,
    window: CFRetained<AXUIElement>,
) -> Option<Tracked> {
    if s.windows.find(&window, pid).is_some() {
        return None;
    }
    // An app listing thousands of windows would make tracking crawl (SAFE-4).
    if !crate::limits::may_track(s.windows.count_for(pid)) {
        if s.over_window_limit.insert(pid) {
            wm_log!(
                "app {pid} has over {} windows; tracking the first {} (SAFE-4)",
                crate::limits::MAX_WINDOWS_PER_APP,
                crate::limits::MAX_WINDOWS_PER_APP
            );
        }
        return None;
    }
    let kind = classify(&facts(&window))?;
    let frame = ax::frame(&window)?;
    if let Some(observer) = s.observers.get(&pid) {
        for name in WINDOW_NOTIFICATIONS {
            // A window that refuses one notification is still tracked; we just hear less.
            add_notification(observer, &window, name, s.self_ptr);
        }
    }
    if let Some(lost) = s.lost.claim(pid, kind, frame, Instant::now()) {
        let id = lost.id;
        s.windows.insert_with_id(id, window, pid, kind);
        wm_log!("{id:?} is back after a display change (pid {pid}); keeping its place (MON-7)");
        // Put it where core last wanted it.
        let s = &mut *s;
        match lost.intent {
            Some(Intent::At(rect)) => super::windows::set_frame(&mut s.windows, id, rect),
            Some(Intent::Hidden) => hide_or_retry(s, id),
            None => {}
        }
        return Some(Tracked::Rebound(id));
    }
    let id = s.windows.insert(window, pid, kind);
    if kind == WindowKind::Popup {
        let now = Instant::now();
        s.reclassify.push((id, now, 0));
        arm_reclassify_timer(s, now);
    }
    // Core never places or hit-tests a popup, so it gets no frame; a managed window gets
    // one only once the window server confirms it (SAFE-3, HARD-7).
    let confirmed = if kind == WindowKind::Popup {
        None
    } else {
        confirmed_or_queued(s, id, pid, frame, 0)
    };
    Some(Tracked::New(WindowInfo {
        id,
        pid: Pid(pid),
        kind,
        frame: confirmed,
        position_hint: Some(Point::new(frame.x, frame.y)),
        rule: app_rule(&s.config.rules, pid),
    }))
}

/// `frame` if the window server shows a window of `pid` there (SAFE-3); otherwise queues
/// the window for another look (`attempts_done` re-checks so far) and returns an unusable
/// frame, which core treats as unknown.
fn confirmed_or_queued(
    s: &mut Shared,
    id: WindowId,
    pid: i32,
    frame: Rect,
    attempts_done: u8,
) -> Option<UsableRect> {
    let now = Instant::now();
    let server = super::window_list::bounds_for(&mut s.server_bounds, pid, now);
    let claimed = s.windows.claims_near(pid, id, frame);
    if let Some(usable) = confirmed_frame(frame, &server, claimed) {
        if let Some(w) = s.windows.get_mut(id) {
            w.confirmed = Some(frame);
        }
        return Some(usable);
    }
    match next_recheck(attempts_done) {
        Some(after) => {
            s.reconfirm.retain(|(r, _, _)| *r != id);
            s.reconfirm.push((id, now + after, attempts_done));
            arm_reconfirm_timer(s, now);
        }
        None => {
            wm_log!("{id:?}: the window server never confirmed its frame; ignoring it (SAFE-3)")
        }
    }
    None
}

fn arm_reconfirm_timer(s: &Shared, now: Instant) {
    if let Some(timer) = &s.reconfirm_timer {
        match s.reconfirm.iter().map(|(_, at, _)| *at).min() {
            Some(at) => arm(timer, at.saturating_duration_since(now)),
            None => disarm(timer),
        }
    }
}

/// Looks again at frames the window server didn't confirm (SAFE-3); a confirmed one is
/// reported to core as the window's frame.
pub(crate) unsafe extern "C-unwind" fn on_reconfirm_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "reconfirm timer", reconfirm_due) };
}

fn reconfirm_due(s: &mut Shared) {
    let now = Instant::now();
    let due: Vec<(WindowId, u8)> = s
        .reconfirm
        .iter()
        .filter(|(_, at, _)| *at <= now)
        .map(|(id, _, n)| (*id, *n))
        .collect();
    s.reconfirm.retain(|(_, at, _)| *at > now);
    for (id, attempts_done) in due {
        let Some(w) = s.windows.get(id) else { continue };
        if w.hidden {
            continue;
        }
        let pid = w.pid;
        let Some(frame) = ax::frame(&w.element) else {
            continue;
        };
        if let Some(frame) = confirmed_or_queued(s, id, pid, frame, attempts_done + 1) {
            s.push(Event::WindowFrameChanged { id, frame });
        }
    }
    arm_reconfirm_timer(s, now);
}

/// Tracks every non-minimized window of `pid` not tracked yet.
pub(crate) fn track_app_windows(s: &mut Shared, pid: i32) -> Vec<WindowInfo> {
    let app = ax::app_element(pid);
    ax::elements_attr(&app, "AXWindows")
        .into_iter()
        .filter(|w| ax::bool_attr(w, "AXMinimized") != Some(true))
        .filter_map(|w| track_window(s, pid, w).and_then(Tracked::new_window))
        .collect()
}

/// A window's element is gone. Right after a display change macOS may just be rebuilding
/// it, so it's kept as lost (core isn't told) until its app shows it again or a minute
/// passes (MON-7, D30, D44); otherwise it's untracked as usual. Its app is asked for its
/// windows again on the `RESCAN_AFTER` schedule.
pub(crate) fn lose_or_untrack(s: &mut Shared, id: WindowId) {
    let now = Instant::now();
    let normal = s
        .windows
        .get(id)
        .is_some_and(|w| w.kind != WindowKind::Popup);
    if !normal || !is_display_loss(s.last_display_change, now) {
        untrack(s, id);
        return;
    }
    let Some(w) = s.windows.remove(id) else {
        return;
    };
    s.reclassify.retain(|(r, _, _)| *r != id);
    s.hide_retry.retain(|(r, _, _)| *r != id);
    s.settle.remove(id);
    wm_log!("{id:?} vanished during a display change; waiting for it to come back (MON-7)");
    s.lost.remember(LostWindow {
        id,
        pid: w.pid,
        kind: w.kind,
        frame: last_place(w.hidden, w.parked, w.last_set),
        intent: w.intent,
        lost_at: now,
        rescans_done: 0,
    });
    arm_lost_timer(s, now);
}

fn arm_lost_timer(s: &Shared, now: Instant) {
    if let (Some(timer), Some(at)) = (&s.lost_timer, s.lost.next_wake()) {
        arm(timer, at.saturating_duration_since(now));
    }
}

/// Lost windows nobody claimed in time really are gone: tell core now.
pub(crate) unsafe extern "C-unwind" fn on_lost_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "lost-window timer", lost_due) };
}

fn lost_due(s: &mut Shared) {
    let now = Instant::now();
    // Most apps don't announce a rebuilt window until they're activated: go and look.
    for pid in s.lost.due_rescans(now) {
        for info in track_app_windows(s, pid) {
            s.push(Event::WindowCreated(info));
        }
        wm_log!(
            "looked at app {pid}'s windows again; {} still lost (MON-7)",
            s.lost.count_for(pid)
        );
    }
    for id in s.lost.take_expired(now) {
        wm_log!("{id:?} didn't come back after the display change");
        s.push(Event::WindowDestroyed(id));
    }
    arm_lost_timer(s, now);
}

/// Forgets a window and tells core it's gone.
pub(crate) fn untrack(s: &mut Shared, id: WindowId) {
    if s.windows.remove(id).is_some() {
        s.reclassify.retain(|(r, _, _)| *r != id);
        s.hide_retry.retain(|(r, _, _)| *r != id);
        s.settle.remove(id);
        s.push(Event::WindowDestroyed(id));
    }
}

/// Called by Accessibility for every registered notification.
unsafe extern "C-unwind" fn on_notification(
    _observer: NonNull<AXObserver>,
    element: NonNull<AXUIElement>,
    name: NonNull<CFString>,
    info: *mut c_void,
) {
    // SAFETY: AX passes valid references for the duration of the callback. Retaining the
    // element lets the work keep it past this callback (in the table, or until sorbo is free).
    let element = unsafe { element.as_ref() }.retain();
    // SAFETY: as above.
    let name = unsafe { name.as_ref() }.to_string();
    // SAFETY: `info` is the shared-state pointer registered in `observe_app`.
    unsafe {
        run_or_defer(info, "accessibility notice", move |s| {
            notification(s, element, name)
        })
    };
}

/// An Accessibility notice about `element` (spec 5.2, 5.8).
fn notification(s: &mut Shared, element: CFRetained<AXUIElement>, name: String) {
    let Some(pid) = ax::element_pid(&element) else {
        return;
    };
    if name == "AXFocusedWindowChanged" {
        // A background app (e.g. after its popup closed) isn't a system focus change.
        if reports_focus_change(pid, s.active_pid) {
            super::focus::report_focus(s, pid);
        }
        return;
    }
    let tracked = s.windows.find(&element, pid);
    match (name.as_str(), tracked) {
        ("AXWindowCreated", None) | ("AXWindowDeminiaturized", None) => {
            if let Some(Tracked::New(info)) = track_window(s, pid, element) {
                s.push(Event::WindowCreated(info));
            }
        }
        ("AXUIElementDestroyed", Some(id)) => lose_or_untrack(s, id),
        ("AXWindowMiniaturized", Some(id)) => untrack(s, id),
        ("AXWindowMoved" | "AXWindowResized", Some(id)) => moved(s, id, element),
        ("AXWindowMoved" | "AXWindowResized", None) => left_fullscreen(s, pid, element),
        _ => {}
    }
}

/// A tracked window moved or resized: fullscreen, late classification, or a real change.
fn moved(s: &mut Shared, id: WindowId, element: CFRetained<AXUIElement>) {
    let Some(w) = s.windows.get(id) else { return };
    if w.hidden {
        return;
    }
    let pid = w.pid;
    if ax::bool_attr(&element, "AXFullScreen") == Some(true) {
        untrack(s, id);
        // Listed once, however often it's reported (SAFE-9).
        let listed = s
            .fullscreen
            .iter()
            .any(|(p, e)| *p == pid && objc2_core_foundation::CFEqual(Some(&**e), Some(&*element)));
        if !listed {
            s.fullscreen.push((pid, element));
        }
        return;
    }
    if w.kind == WindowKind::Popup {
        reclassify(s, id);
        return;
    }
    // Don't report yet: wait until the window has been still for a moment (spec 5.8).
    let now = Instant::now();
    s.settle.poke(id, now);
    arm_frame_timer(s, now);
}

fn arm_frame_timer(s: &Shared, now: Instant) {
    if let Some(timer) = &s.frame_timer {
        match s.settle.next_deadline() {
            Some(at) => arm(timer, at.saturating_duration_since(now)),
            None => disarm(timer),
        }
    }
}

/// Fires when a moved or resized window has been still for 300 ms: report its frame unless
/// it's where we put it.
pub(crate) unsafe extern "C-unwind" fn on_frame_settle_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "frame settle timer", frames_settled) };
}

fn frames_settled(s: &mut Shared) {
    let now = Instant::now();
    for id in s.settle.take_due(now) {
        let Some(w) = s.windows.get(id) else { continue };
        if w.hidden {
            continue;
        }
        let last_set = w.last_set;
        let Some(frame) = ax::frame(&w.element) else {
            if ax::attr(&w.element, "AXRole").err() == Some(AXError::InvalidUIElement) {
                lose_or_untrack(s, id);
            }
            continue;
        };
        if !is_echo(last_set, frame) {
            let pid = w.pid;
            if let Some(frame) = confirmed_or_queued(s, id, pid, frame, 0) {
                s.push(Event::WindowFrameChanged { id, frame });
            }
        }
    }
    arm_frame_timer(s, now);
}

/// An untracked window moved: if it was in fullscreen and has left it, report it again.
fn left_fullscreen(s: &mut Shared, pid: i32, element: CFRetained<AXUIElement>) {
    let Some(index) = s.fullscreen.iter().position(|(p, e)| {
        *p == pid && objc2_core_foundation::CFEqual(Some(&**e), Some(&*element))
    }) else {
        return;
    };
    if ax::bool_attr(&element, "AXFullScreen") == Some(true) {
        return;
    }
    s.fullscreen.remove(index);
    if let Some(Tracked::New(info)) = track_window(s, pid, element) {
        s.push(Event::WindowCreated(info));
    }
}

/// Classifies a popup-classified window again; if its kind changed, it's reported as a new
/// window with the new kind (spec 5.2), and its new id is returned. An unchanged window keeps
/// its place in the re-classify schedule.
pub(crate) fn reclassify(s: &mut Shared, id: WindowId) -> Option<WindowId> {
    let w = s.windows.get(id)?;
    let new_kind = classify(&facts(&w.element));
    if new_kind == Some(w.kind) {
        return None;
    }
    let pid = w.pid;
    let element = w.element.clone();
    untrack(s, id);
    match track_window(s, pid, element) {
        Some(Tracked::New(info)) => {
            let new_id = info.id;
            s.push(Event::WindowCreated(info));
            Some(new_id)
        }
        _ => None,
    }
}

/// Arms the re-classify timer for the earliest due entry, or disarms it.
fn arm_reclassify_timer(s: &Shared, now: Instant) {
    let next = s
        .reclassify
        .iter()
        .filter_map(|(_, appeared, done)| reclassify_after(*done).map(|after| *appeared + after))
        .min();
    if let Some(timer) = &s.reclassify_timer {
        match next {
            Some(at) => arm(timer, at.saturating_duration_since(now)),
            None => disarm(timer),
        }
    }
}

/// Fires when the earliest pending re-classification is due.
pub(crate) unsafe extern "C-unwind" fn on_reclassify_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "re-classify timer", reclassify_due) };
}

fn reclassify_due(s: &mut Shared) {
    let now = Instant::now();
    let due: Vec<WindowId> = s
        .reclassify
        .iter()
        .filter(|(_, appeared, done)| {
            reclassify_after(*done).is_some_and(|after| *appeared + after <= now)
        })
        .map(|(id, _, _)| *id)
        .collect();
    for id in due {
        // Counted first: if the kind changed, `untrack` drops the entry; if not, the next
        // check (1 s, then 5 s after it appeared) stays scheduled (HARD-2).
        if let Some(entry) = s.reclassify.iter_mut().find(|(r, _, _)| *r == id) {
            entry.2 += 1;
        }
        reclassify(s, id);
    }
    s.reclassify
        .retain(|(_, _, done)| reclassify_after(*done).is_some());
    arm_reclassify_timer(s, now);
}

/// How often, and how many times, to retry observing a just-launched app.
const LAUNCH_RETRY_EVERY: Duration = Duration::from_millis(250);
const LAUNCH_RETRIES: u32 = 20;

/// A regular app was launched: observe it and report its windows, retrying for up to 5 s
/// while Accessibility isn't ready for it yet.
pub(crate) fn app_launched(s: &mut Shared, pid: i32) {
    if try_observe_launched(s, pid) {
        return;
    }
    retry_observe(s, pid);
}

/// Keeps trying to observe `pid` every 250 ms for up to 5 s. Used for just-launched apps
/// and for apps that were still starting when the WM started (e.g. at login).
pub(crate) fn retry_observe(s: &mut Shared, pid: i32) {
    s.pending_launch.push((pid, 0));
    if let Some(timer) = &s.launch_timer {
        arm(timer, LAUNCH_RETRY_EVERY);
    }
}

fn try_observe_launched(s: &mut Shared, pid: i32) -> bool {
    if !observe_app(s, pid) {
        return false;
    }
    watch_tracked_windows(s, pid);
    for info in track_app_windows(s, pid) {
        s.push(Event::WindowCreated(info));
    }
    true
}

/// Registers window notifications on windows of `pid` that were tracked before the app had
/// an observer (e.g. through a focus change). Without this they'd never report closing.
fn watch_tracked_windows(s: &mut Shared, pid: i32) {
    let Some(observer) = s.observers.get(&pid) else {
        return;
    };
    for id in s.windows.ids_for_pid(pid) {
        if let Some(w) = s.windows.get(id) {
            for name in WINDOW_NOTIFICATIONS {
                add_notification(observer, &w.element, name, s.self_ptr);
            }
        }
    }
}

/// Retries observing apps whose launch arrived before Accessibility was ready.
pub(crate) unsafe extern "C-unwind" fn on_launch_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "launch retry timer", launch_retry_due) };
}

fn launch_retry_due(s: &mut Shared) {
    let pending = std::mem::take(&mut s.pending_launch);
    let mut still_pending = Vec::new();
    for (pid, tries) in pending {
        if try_observe_launched(s, pid) {
            continue;
        }
        if tries + 1 < LAUNCH_RETRIES {
            still_pending.push((pid, tries + 1));
        } else {
            wm_log!("pid {pid}: gave up observing it after {LAUNCH_RETRIES} tries");
        }
    }
    s.pending_launch.extend(still_pending);
    if let Some(timer) = &s.launch_timer {
        if s.pending_launch.is_empty() {
            disarm(timer);
        } else {
            arm(timer, LAUNCH_RETRY_EVERY);
        }
    }
}

/// `Hide`, and if the window didn't move (a busy app), try again later with growing gaps
/// (HARD-3). Only the first failure is logged.
pub(crate) fn hide_or_retry(s: &mut Shared, id: WindowId) {
    if let Err(why) = super::windows::hide(&mut s.windows, id, &s.displays) {
        if s.windows.get(id).is_none() {
            wm_log!("Hide for unknown {id:?}");
            return;
        }
        wm_log!("Hide {id:?} failed: {why}; retrying (HARD-3)");
        let now = Instant::now();
        s.hide_retry.retain(|(r, _, _)| *r != id);
        s.hide_retry.push((id, now + hide_retry_after(0), 0));
        arm_hide_retry_timer(s, now);
    } else {
        s.hide_retry.retain(|(r, _, _)| *r != id);
    }
}

fn arm_hide_retry_timer(s: &Shared, now: Instant) {
    if let Some(timer) = &s.hide_retry_timer {
        match s.hide_retry.iter().map(|(_, due, _)| *due).min() {
            Some(at) => arm(timer, at.saturating_duration_since(now)),
            None => disarm(timer),
        }
    }
}

/// Fires when the earliest failed hide is due to be tried again. It stops for a window that
/// is gone or that core has since placed elsewhere (`SetFrame` changed its intent).
pub(crate) unsafe extern "C-unwind" fn on_hide_retry_timer(
    timer: *mut CFRunLoopTimer,
    info: *mut c_void,
) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`.
    unsafe { run_or_retry(timer, info, "hide retry timer", hide_retry_due) };
}

fn hide_retry_due(s: &mut Shared) {
    let now = Instant::now();
    let due: Vec<(WindowId, u8)> = s
        .hide_retry
        .iter()
        .filter(|(_, at, _)| *at <= now)
        .map(|(id, _, done)| (*id, *done))
        .collect();
    for (id, done) in due {
        s.hide_retry.retain(|(r, _, _)| *r != id);
        let wanted = s
            .windows
            .get(id)
            .is_some_and(|w| w.intent == Some(Intent::Hidden));
        if !wanted {
            continue;
        }
        let s = &mut *s;
        match super::windows::hide(&mut s.windows, id, &s.displays) {
            Ok(()) => wm_log!("Hide {id:?} succeeded after {} retries (HARD-3)", done + 1),
            Err(_) => {
                let done = done.saturating_add(1);
                s.hide_retry.push((id, now + hide_retry_after(done), done));
            }
        }
    }
    arm_hide_retry_timer(s, now);
}
