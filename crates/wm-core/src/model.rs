//! The state core keeps about windows, workspaces and monitors.

/// How a window is managed. Decided by wm-macos when it reports the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// Tiles by default.
    Normal,
    /// Starts floating: dialogs, panels, fixed-size windows.
    Floating,
    /// Transient app UI (menus, popovers, sheets, extension popups). Never managed.
    Popup,
}

use crate::{MonitorId, Node, Pid, Rect, WindowId, WorkspaceNumber};

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub id: WindowId,
    pub pid: Pid,
    pub kind: WindowKind,
    /// The workspace the window belongs to. Meaningless for popups.
    pub workspace: WorkspaceNumber,
    /// Currently floating (Floating kind, or toggled with KEY-5).
    pub floating: bool,
    /// Last frame while floating; `None` until it has floated once.
    pub floating_frame: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub number: WorkspaceNumber,
    /// Assigned monitor (MON-4). Where the workspace lives is derived from it.
    pub home: MonitorId,
    /// Tiled windows.
    pub tree: Option<Node>,
    /// Floating windows, back-to-front.
    pub floating: Vec<WindowId>,
    /// Restored when the workspace is shown again.
    pub last_focused: Option<WindowId>,
}

impl Workspace {
    pub fn new(number: WorkspaceNumber, home: MonitorId) -> Workspace {
        Workspace {
            number,
            home,
            tree: None,
            floating: Vec::new(),
            last_focused: None,
        }
    }

    /// No tiled and no floating windows. Popups never count.
    pub fn is_empty(&self) -> bool {
        self.tree.is_none() && self.floating.is_empty()
    }

    pub fn contains(&self, id: WindowId) -> bool {
        self.floating.contains(&id) || self.tree.as_ref().is_some_and(|t| t.contains(id))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub id: MonitorId,
    /// Usable area: excludes the menu bar and Dock.
    pub frame: Rect,
    pub is_main: bool,
    /// Each monitor shows exactly one workspace.
    pub visible: WorkspaceNumber,
}

/// Where core last told wm-macos to put a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    At(Rect),
    Hidden,
}
