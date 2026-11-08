//! The window table, window discovery, and the SetFrame / Hide commands (spec 3.3, 5.2, 5.4).

use std::collections::HashMap;

use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
use objc2_application_services::AXUIElement;
use objc2_core_foundation::{CFEqual, CFRetained, CFType};
use wm_core::{Rect, WindowId, WindowKind};

use super::ax;
use super::displays::Display;
use crate::classify::WindowFacts;
use crate::hide::{hide_position, parking_size, restore_frame};
use crate::lost::Intent;
use crate::wm_log;

/// What `wm-macos` remembers about a window it has reported to core.
pub(crate) struct TrackedWindow {
    pub element: CFRetained<AXUIElement>,
    pub pid: i32,
    pub kind: WindowKind,
    /// The frame we last set, for echo suppression (spec 5.8).
    pub last_set: Option<Rect>,
    /// The last frame of this window the window server confirmed (SAFE-3).
    pub confirmed: Option<Rect>,
    pub hidden: bool,
    /// The usable frame of the display the window was hidden from (for restore on exit).
    pub hidden_from: Option<Rect>,
    /// The last placement core asked for, even if applying it failed (MON-7).
    pub intent: Option<Intent>,
}

/// Every reported window, keyed by the `WindowId` we assigned (spec 3.3).
pub(crate) struct WindowTable {
    next_id: u64,
    windows: HashMap<WindowId, TrackedWindow>,
}

impl WindowTable {
    pub fn new() -> WindowTable {
        WindowTable {
            next_id: 1,
            windows: HashMap::new(),
        }
    }

    /// Starts tracking a window and returns its new ID.
    pub fn insert(
        &mut self,
        element: CFRetained<AXUIElement>,
        pid: i32,
        kind: WindowKind,
    ) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        self.windows.insert(
            id,
            TrackedWindow {
                element,
                pid,
                kind,
                last_set: None,
                confirmed: None,
                hidden: false,
                hidden_from: None,
                intent: None,
            },
        );
        id
    }

    /// Tracks a window under an ID core already knows: a window macOS rebuilt (MON-7).
    pub fn insert_with_id(
        &mut self,
        id: WindowId,
        element: CFRetained<AXUIElement>,
        pid: i32,
        kind: WindowKind,
    ) {
        self.next_id = self.next_id.max(id.0 + 1);
        self.windows.insert(
            id,
            TrackedWindow {
                element,
                pid,
                kind,
                last_set: None,
                confirmed: None,
                hidden: false,
                hidden_from: None,
                intent: None,
            },
        );
    }

    /// How many of app `pid`'s tracked windows other than `except` are known to be at
    /// `frame`: sorbo put them there or the window server confirmed them there (SAFE-3).
    pub fn claims_near(&self, pid: i32, except: WindowId, frame: Rect) -> usize {
        self.windows
            .iter()
            .filter(|(id, w)| w.pid == pid && **id != except)
            .filter(|(_, w)| {
                [w.last_set, w.confirmed]
                    .iter()
                    .flatten()
                    .any(|known| crate::confirm::near(known, &frame))
            })
            .count()
    }

    /// How many windows of app `pid` are tracked (SAFE-4).
    pub fn count_for(&self, pid: i32) -> usize {
        self.windows.values().filter(|w| w.pid == pid).count()
    }

    /// The ID of a tracked window, compared with `CFEqual` (only that app's windows are
    /// scanned, which is a handful).
    pub fn find(&self, element: &AXUIElement, pid: i32) -> Option<WindowId> {
        let wanted: &CFType = element;
        self.windows
            .iter()
            .filter(|(_, w)| w.pid == pid)
            .find(|(_, w)| {
                let tracked: &CFType = &w.element;
                CFEqual(Some(tracked), Some(wanted))
            })
            .map(|(id, _)| *id)
    }

    pub fn get(&self, id: WindowId) -> Option<&TrackedWindow> {
        self.windows.get(&id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut TrackedWindow> {
        self.windows.get_mut(&id)
    }

    pub fn remove(&mut self, id: WindowId) -> Option<TrackedWindow> {
        self.windows.remove(&id)
    }

    /// IDs of every hidden window, sorted.
    pub fn hidden_ids(&self) -> Vec<WindowId> {
        let mut ids: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, w)| w.hidden)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }

    /// IDs of every tracked window of one app, sorted so events come out in a stable order.
    pub fn ids_for_pid(&self, pid: i32) -> Vec<WindowId> {
        let mut ids: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, w)| w.pid == pid)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }
}

/// Reads the attributes classification needs.
pub(crate) fn facts(window: &AXUIElement) -> WindowFacts {
    WindowFacts {
        role: ax::string_attr(window, "AXRole"),
        subrole: ax::string_attr(window, "AXSubrole"),
        resizable: ax::settable(window, "AXSize"),
    }
}

/// The apps we manage: regular apps (with a Dock icon), never ourselves, as
/// `(pid, is_hidden)`. Hidden apps (⌘H) are observed too, so that once unhidden their
/// windows report closing, minimizing and moving.
pub(crate) fn regular_apps() -> Vec<(i32, bool)> {
    let me = std::process::id() as i32;
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
        .map(|app| (app.processIdentifier(), app.isHidden()))
        .filter(|&(pid, _)| pid != me)
        .collect()
}

/// `SetFrame`: position, size, then position again, because an app may refuse a size that
/// doesn't fit at the old position.
pub(crate) fn set_frame(table: &mut WindowTable, id: WindowId, frame: Rect) {
    let Some(w) = table.get_mut(id) else {
        wm_log!("SetFrame for unknown {id:?}");
        return;
    };
    w.intent = Some(Intent::At(frame));
    ax::set_position(&w.element, frame.x, frame.y);
    ax::set_size(&w.element, frame.width, frame.height);
    let err = ax::set_position(&w.element, frame.x, frame.y);
    if err.0 != 0 {
        wm_log!("SetFrame {id:?} failed: {err:?}");
    }
    w.last_set = Some(frame);
    w.hidden = false;
    w.hidden_from = None;
}

/// `Hide`: park the window in the bottom corner that shows least of it (spec 5.4).
///
/// `Err` says why it didn't move (HARD-3); the window then still counts as shown, and the
/// caller retries. Without a readable frame (a busy app), the size sorbo last set or last
/// confirmed picks the corner.
pub(crate) fn hide(
    table: &mut WindowTable,
    id: WindowId,
    displays: &[Display],
) -> Result<(), String> {
    let Some(w) = table.get_mut(id) else {
        return Err("unknown window".into());
    };
    w.intent = Some(Intent::Hidden);
    let read = ax::frame(&w.element);
    let Some((width, height)) = parking_size(read, w.last_set, w.confirmed) else {
        return Err("could not read its frame, and no earlier frame is known".into());
    };
    let center = [read, w.last_set, w.confirmed]
        .into_iter()
        .flatten()
        .find(Rect::is_usable)
        .map(|r| r.center());
    let own = center
        .and_then(|c| displays.iter().position(|d| d.bounds.contains(c)))
        .unwrap_or(0);
    let bounds: Vec<Rect> = displays.iter().map(|d| d.bounds).collect();
    let p = hide_position(width, height, own, &bounds);
    let err = ax::set_position(&w.element, p.x, p.y);
    if err.0 != 0 {
        return Err(format!("{err:?}"));
    }
    w.hidden = true;
    w.hidden_from = displays.get(own).map(|d| d.info.frame);
    Ok(())
}

/// After a display change, parks every hidden window again using the new display list:
/// the best corner may have moved, or macOS may have dragged a parked window into view.
pub(crate) fn repark_hidden(table: &mut WindowTable, displays: &[Display]) {
    let bounds: Vec<Rect> = displays.iter().map(|d| d.bounds).collect();
    for id in table.hidden_ids() {
        let Some(w) = table.get_mut(id) else { continue };
        let Some(frame) = ax::frame(&w.element) else {
            continue;
        };
        // The display it was hidden from, if still connected; otherwise the first one.
        let own = w
            .hidden_from
            .and_then(|from| {
                displays
                    .iter()
                    .position(|d| d.bounds.contains(from.center()))
            })
            .unwrap_or(0);
        let p = hide_position(frame.width, frame.height, own, &bounds);
        ax::set_position(&w.element, p.x, p.y);
    }
}

/// On exit: shows every hidden window again, centered on the display it was hidden from.
pub(crate) fn restore_hidden(table: &mut WindowTable, displays: &[Display]) {
    let main = displays
        .iter()
        .find(|d| d.info.is_main)
        .or(displays.first())
        .map(|d| d.info.frame);
    for id in table.hidden_ids() {
        let Some(w) = table.get(id) else { continue };
        let Some(frame) = ax::frame(&w.element) else {
            continue;
        };
        let Some(display) = w.hidden_from.or(main) else {
            continue;
        };
        let target = restore_frame(frame.width, frame.height, display);
        wm_log!("restoring hidden {id:?} to {target:?}");
        set_frame(table, id, target);
    }
}
