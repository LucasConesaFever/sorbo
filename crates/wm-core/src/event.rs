//! The boundary with wm-macos: events come in, commands go out.

use crate::{
    Action, Gaps, MonitorId, Pid, Point, Rect, UsableRect, WindowId, WindowKind, WorkspaceNumber,
};

/// Something that happened, reported by wm-macos.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Full snapshot of connected monitors: at startup and after any display change.
    MonitorsChanged(Vec<MonitorInfo>),
    /// Windows that already existed when the WM started. Sent once, after the first MonitorsChanged.
    WindowsDiscovered(Vec<WindowInfo>),
    /// A window appeared while running.
    WindowCreated(WindowInfo),
    /// A window went away (also sent for minimize, Cmd+H and native fullscreen).
    WindowDestroyed(WindowId),
    /// macOS moved focus by itself: click, Cmd+Tab, Cmd+`, Dock click, or the echo of our Focus.
    WindowFocused(WindowId),
    /// macOS focus moved to something core doesn't manage (desktop, unreported window).
    FocusLeft,
    /// A window's frame changed for a reason other than our own SetFrame/Hide.
    WindowFrameChanged { id: WindowId, frame: UsableRect },
    /// The pointer moved. wm-macos does the hit test against the real z-order.
    PointerMoved {
        position: Point,
        target: PointerTarget,
    },
    /// A hotkey from the keymap fired.
    Action(Action),
    /// The config file set new gaps (CFG-4). May arrive before the first `MonitorsChanged`.
    GapsChanged(Gaps),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    /// Usable area: excludes the menu bar and Dock.
    pub frame: Rect,
    pub is_main: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub pid: Pid,
    pub kind: WindowKind,
    /// `None`: no usable frame is known yet (SAFE-1, SAFE-3); the window isn't moved and
    /// doesn't own the pointer until one arrives with `WindowFrameChanged`.
    pub frame: Option<UsableRect>,
    /// Where the app says the window's top-left corner is, unchecked. Used only to pick the
    /// monitor of a window found at startup before its frame is confirmed (HARD-7); never to
    /// place a window or to decide what's under the pointer.
    pub position_hint: Option<Point>,
    /// What the config's window rules say about this window's app (RULE-1).
    pub rule: RuleEffect,
}

/// The window rules' verdict for one window (RULE-2): `None` means "as usual".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuleEffect {
    /// Start floating (`true`) or tiled (`false`), whatever sorbo would have picked.
    pub floating: Option<bool>,
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerTarget {
    /// Topmost window under the pointer that wm-macos has reported to core.
    Window(WindowId),
    /// Menu bar, a menu, Dock, desktop, or any window core doesn't know about.
    Other,
}

/// Something core wants done, executed by wm-macos in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Also how a hidden window is shown again.
    SetFrame {
        id: WindowId,
        frame: Rect,
    },
    /// Move off-screen; wm-macos picks how and where.
    Hide(WindowId),
    /// Focus the window and raise it.
    Focus(WindowId),
    WarpPointer(Point),
    Close(WindowId),
    LaunchNewWindow(App),
    /// The menu bar's workspace row changed (BAR-1).
    ShowWorkspaces(Vec<WorkspaceStatus>),
    /// The key mode changed (MODE-1): wm-macos registers that mode's keys and shows it.
    ModeChanged(Mode),
}

/// Which keys are live (MODE-1). In `Resize`, plain arrows / `h j k l` resize and Escape or
/// Enter leave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Resize,
}

/// How a workspace looks in the menu bar row (BAR-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceState {
    /// The visible workspace of the focused monitor.
    Focused,
    /// Visible on a monitor that isn't the focused one.
    Visible,
    /// Not visible anywhere (it has windows, or it would have been removed).
    Hidden,
}

/// One entry of the menu bar row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub number: WorkspaceNumber,
    pub state: WorkspaceState,
}

/// The app a `new-window` binding opens (CFG-3). Resolved to an installed app by wm-macos
/// when the key is pressed, since apps come and go while the WM runs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum App {
    /// The system's default web browser.
    Browser,
    /// A bundle identifier, e.g. `com.apple.Terminal`.
    Bundle(String),
    /// An app name as in `/Applications`, e.g. `kitty` or `Google Chrome`.
    Named(String),
}
