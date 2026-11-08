//! Identifiers. Newtypes stop a window ID being passed where a process ID is expected.

use std::num::NonZeroU32;

/// A window, as identified by wm-macos (e.g. the CGWindowID). Opaque to core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub u64);

/// The process that owns a window. Used to tell "same app".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pid(pub i32);

/// Stable monitor identity provided by wm-macos (MON-2). Opaque to core.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MonitorId(pub String);

impl From<&str> for MonitorId {
    fn from(s: &str) -> Self {
        MonitorId(s.to_string())
    }
}

/// Workspaces are identified by their number, 1 and up. A `NonZeroU32` inside, so a
/// workspace 0 can't exist anywhere (HARD-7); `new` is the only way in from a plain number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceNumber(NonZeroU32);

impl WorkspaceNumber {
    pub const ONE: WorkspaceNumber = WorkspaceNumber(NonZeroU32::MIN);

    /// `None` for 0.
    pub fn new(n: u32) -> Option<WorkspaceNumber> {
        NonZeroU32::new(n).map(WorkspaceNumber)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// The number after this one (`None` past `u32::MAX`).
    pub fn next(self) -> Option<WorkspaceNumber> {
        self.0.checked_add(1).map(WorkspaceNumber)
    }

    /// The number before this one (`None` before 1).
    pub fn prev(self) -> Option<WorkspaceNumber> {
        WorkspaceNumber::new(self.get() - 1)
    }
}

impl std::fmt::Display for WorkspaceNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::w;

    #[test]
    fn hard_7_workspace_number_is_never_zero() {
        assert_eq!(WorkspaceNumber::new(0), None);
        let three = WorkspaceNumber::new(3).unwrap();
        assert_eq!(three.get(), 3);
        assert_eq!(three.to_string(), "3");
        assert_eq!(three.next().map(WorkspaceNumber::get), Some(4));
        assert_eq!(three.prev().map(WorkspaceNumber::get), Some(2));
        assert_eq!(WorkspaceNumber::ONE.prev(), None);
        assert!(WorkspaceNumber::ONE < three);
    }

    #[test]
    fn safe_8_window_ids_are_64_bit() {
        let big = u64::from(u32::MAX) + 1;
        assert_eq!(w(big).0, big);
        assert_eq!(format!("{:?}", w(big)), "WindowId(4294967296)");
    }
}
