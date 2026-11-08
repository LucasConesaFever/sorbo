//! Helpers for tests: short constructors, a started `Wm`, and a fake platform.
//!
//! Public (not `#[cfg(test)]`) so integration tests in `tests/` can use them too.

use std::collections::VecDeque;

use crate::{
    Command, Event, MonitorId, MonitorInfo, Pid, Platform, Point, Rect, WindowId, WindowInfo,
    WindowKind, Wm,
};

pub fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h)
}

/// The workspace numbers in use, as plain numbers.
pub fn nums(wm: &Wm) -> Vec<u32> {
    wm.workspace_numbers().iter().map(|n| n.get()).collect()
}

/// Workspace number `n` (1 or more).
pub fn ws(n: u32) -> crate::WorkspaceNumber {
    crate::WorkspaceNumber::new(n).expect("workspace numbers start at 1")
}

/// A usable frame, as `WindowFrameChanged` carries; panics on an unusable one.
pub fn ur(x: f64, y: f64, w: f64, h: f64) -> crate::UsableRect {
    crate::UsableRect::new(r(x, y, w, h)).expect("a usable test frame")
}

pub fn p(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

pub fn w(n: u64) -> WindowId {
    WindowId(n)
}

pub fn monitor(id: &str, frame: Rect, is_main: bool) -> MonitorInfo {
    MonitorInfo {
        id: MonitorId::from(id),
        frame,
        is_main,
    }
}

/// `"main"`: 1000x800 at the origin, the macOS main display.
pub fn main_monitor() -> MonitorInfo {
    monitor("main", r(0., 0., 1000., 800.), true)
}

/// `"ext"`: 1000x800 to the right of `"main"`.
pub fn ext_monitor() -> MonitorInfo {
    monitor("ext", r(1000., 0., 1000., 800.), false)
}

/// A window reporting `frame`; an unusable frame becomes "no frame", as wm-macos sends it.
pub fn window(id: u64, pid: i32, kind: WindowKind, frame: Rect) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        pid: Pid(pid),
        kind,
        frame: crate::UsableRect::new(frame),
        position_hint: Some(Point::new(frame.x, frame.y)),
        rule: crate::RuleEffect::default(),
    }
}

/// A normal window with pid `100 + id`, at `r(0,0,100,100)`.
pub fn normal(id: u64) -> WindowInfo {
    window(
        id,
        100 + id as i32,
        WindowKind::Normal,
        r(0., 0., 100., 100.),
    )
}

pub fn popup_of(id: u64, owner_pid: i32) -> WindowInfo {
    window(id, owner_pid, WindowKind::Popup, r(0., 0., 50., 50.))
}

/// A `Wm` that has received its first monitors and discovered windows; their commands are discarded.
pub fn started(monitors: Vec<MonitorInfo>, discovered: Vec<WindowInfo>) -> Wm {
    let mut wm = Wm::new();
    wm.handle(Event::MonitorsChanged(monitors));
    wm.handle(Event::WindowsDiscovered(discovered));
    wm
}

/// Replays scripted events and records every executed command.
pub struct FakePlatform {
    events: VecDeque<Event>,
    pub executed: Vec<Command>,
}

impl FakePlatform {
    pub fn new(events: Vec<Event>) -> FakePlatform {
        FakePlatform {
            events: events.into(),
            executed: Vec::new(),
        }
    }
}

impl Platform for FakePlatform {
    fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    fn execute(&mut self, command: Command) {
        self.executed.push(command);
    }
}
