//! Window classification and the pointer-target rule (spec 5.2 and 5.3).
//!
//! Pure functions over plain data, so they're tested on any OS. When unsure, they lean
//! toward "popup" / `Other`: a missed focus change costs less than a closed popup.

use std::time::Duration;

use wm_core::{Point, PointerTarget, Rect, WindowId, WindowKind};

/// The Accessibility attributes classification looks at, read once when a window is first
/// seen. `None` means the read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowFacts {
    /// `AXRole`
    pub role: Option<String>,
    /// `AXSubrole`
    pub subrole: Option<String>,
    /// Whether `AXSize` is settable.
    pub resizable: Option<bool>,
}

/// Decides how core should manage a window. `None` means it isn't a window at all and is
/// never reported (sheets, for example, have role `AXSheet`).
pub fn classify(facts: &WindowFacts) -> Option<WindowKind> {
    // `as_deref` turns `Option<String>` into `Option<&str>` so we can match on literals.
    let Some(role) = facts.role.as_deref() else {
        return Some(WindowKind::Popup);
    };
    if role != "AXWindow" {
        return None;
    }
    let kind = match (facts.subrole.as_deref(), facts.resizable) {
        (Some("AXStandardWindow"), Some(true)) => WindowKind::Normal,
        (Some("AXStandardWindow"), Some(false)) => WindowKind::Floating,
        (Some("AXDialog" | "AXSystemDialog" | "AXFloatingWindow"), _) => WindowKind::Floating,
        // Unknown subrole, failed read, or unknown resizability: lean toward popup.
        _ => WindowKind::Popup,
    };
    Some(kind)
}

/// When to classify a popup-classified window again, counted from when it appeared: after
/// 200 ms, 1 s and 5 s, then never (HARD-2). An app busy while its window opens can make the
/// first reads fail, which classifies the window as a popup.
pub fn reclassify_after(attempts_done: u8) -> Option<Duration> {
    match attempts_done {
        0 => Some(Duration::from_millis(200)),
        1 => Some(Duration::from_secs(1)),
        2 => Some(Duration::from_secs(5)),
        _ => None,
    }
}

/// The app owning the front-most window under `p`, from the window server's list (front to
/// back). Asking the window server sends no message to the app, so this works even for a
/// hung app, and lets the hit test skip a slow one before talking to it (HARD-5).
pub fn topmost_pid_at(windows: &[(i32, Rect)], p: Point) -> Option<i32> {
    windows
        .iter()
        .find(|(_, bounds)| bounds.contains(p))
        .map(|(pid, _)| *pid)
}

/// True for the roles of menus: anything under the pointer with one of these roles is `Other`.
pub fn is_menu_role(role: &str) -> bool {
    matches!(
        role,
        "AXMenu" | "AXMenuItem" | "AXMenuBar" | "AXMenuBarItem"
    )
}

/// What core should be told is under the pointer.
///
/// - `path_roles`: roles from the hit element up to (not including) its window.
/// - `window`: the tracked window that element belongs to, and whether it's hidden.
pub fn pointer_target(path_roles: &[String], window: Option<(WindowId, bool)>) -> PointerTarget {
    if path_roles.iter().any(|role| is_menu_role(role)) {
        return PointerTarget::Other;
    }
    match window {
        Some((id, false)) => PointerTarget::Window(id),
        // No window, or a hidden window's sliver in a screen corner.
        _ => PointerTarget::Other,
    }
}

/// Whether an app changing its focused window is a system focus change. Only the frontmost
/// app's is: a background app picking another of its own windows (for example after its
/// popup closes) must not pull focus back to it.
pub fn reports_focus_change(app_pid: i32, frontmost_pid: Option<i32>) -> bool {
    frontmost_pid == Some(app_pid)
}

/// Whether `close` may press a window's close button: only when its app is frontmost and
/// the app itself says this is its focused window (HARD-2). Closing can't be undone, so a
/// focus sorbo got wrong (a misclassified window, a race) must not close another window.
pub fn may_close(
    window_pid: i32,
    frontmost_pid: Option<i32>,
    is_apps_focused_window: bool,
) -> bool {
    frontmost_pid == Some(window_pid) && is_apps_focused_window
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_5_owner_of_the_topmost_window_under_the_pointer() {
        // Front to back, as the window server lists them.
        let windows = [
            (7, Rect::new(0., 0., 100., 100.)),
            (8, Rect::new(0., 0., 500., 500.)),
        ];
        assert_eq!(topmost_pid_at(&windows, Point::new(50., 50.)), Some(7));
        assert_eq!(topmost_pid_at(&windows, Point::new(300., 300.)), Some(8));
        assert_eq!(topmost_pid_at(&windows, Point::new(900., 900.)), None);
    }

    #[test]
    fn hard_2_close_needs_frontmost_app() {
        assert!(may_close(7, Some(7), true));
        assert!(!may_close(7, Some(8), true));
        assert!(!may_close(7, None, true));
    }

    #[test]
    fn hard_2_close_needs_apps_focused_window() {
        assert!(!may_close(7, Some(7), false));
    }

    #[test]
    fn hard_2_reclassify_schedule() {
        assert_eq!(reclassify_after(0), Some(Duration::from_millis(200)));
        assert_eq!(reclassify_after(1), Some(Duration::from_secs(1)));
        assert_eq!(reclassify_after(2), Some(Duration::from_secs(5)));
        assert_eq!(reclassify_after(3), None);
    }

    fn facts(role: Option<&str>, subrole: Option<&str>, resizable: Option<bool>) -> WindowFacts {
        WindowFacts {
            role: role.map(String::from),
            subrole: subrole.map(String::from),
            resizable,
        }
    }

    #[test]
    fn foc_5_unknown_subrole_is_popup() {
        assert_eq!(
            classify(&facts(Some("AXWindow"), Some("AXUnknown"), Some(false))),
            Some(WindowKind::Popup)
        );
    }

    #[test]
    fn foc_5_failed_read_is_popup() {
        assert_eq!(classify(&facts(None, None, None)), Some(WindowKind::Popup));
        assert_eq!(
            classify(&facts(Some("AXWindow"), None, Some(true))),
            Some(WindowKind::Popup)
        );
        assert_eq!(
            classify(&facts(Some("AXWindow"), Some("AXStandardWindow"), None)),
            Some(WindowKind::Popup)
        );
    }

    #[test]
    fn standard_resizable_window_is_normal() {
        assert_eq!(
            classify(&facts(
                Some("AXWindow"),
                Some("AXStandardWindow"),
                Some(true)
            )),
            Some(WindowKind::Normal)
        );
    }

    #[test]
    fn standard_fixed_size_window_is_floating() {
        assert_eq!(
            classify(&facts(
                Some("AXWindow"),
                Some("AXStandardWindow"),
                Some(false)
            )),
            Some(WindowKind::Floating)
        );
    }

    #[test]
    fn dialog_subroles_are_floating() {
        for s in ["AXDialog", "AXSystemDialog", "AXFloatingWindow"] {
            assert_eq!(
                classify(&facts(Some("AXWindow"), Some(s), Some(true))),
                Some(WindowKind::Floating)
            );
        }
    }

    #[test]
    fn non_window_role_is_not_reported() {
        assert_eq!(
            classify(&facts(Some("AXSheet"), Some("AXUnknown"), Some(false))),
            None
        );
    }

    #[test]
    fn foc_5_menu_under_pointer_is_other() {
        let path = vec!["AXMenuItem".to_string(), "AXMenu".to_string()];
        assert_eq!(
            pointer_target(&path, Some((WindowId(3), false))),
            PointerTarget::Other
        );
    }

    #[test]
    fn foc_1_known_window_under_pointer() {
        let path = vec!["AXGroup".to_string(), "AXScrollArea".to_string()];
        assert_eq!(
            pointer_target(&path, Some((WindowId(3), false))),
            PointerTarget::Window(WindowId(3))
        );
    }

    #[test]
    fn hidden_window_under_pointer_is_other() {
        assert_eq!(
            pointer_target(&[], Some((WindowId(3), true))),
            PointerTarget::Other
        );
    }

    #[test]
    fn foc_5_background_app_focus_change_is_ignored() {
        // Chrome (pid 10) closes its popup while Brave (pid 20) is in front: Chrome picking
        // its own focused window must not count as a system focus change.
        assert!(!reports_focus_change(10, Some(20)));
    }

    #[test]
    fn frontmost_app_focus_change_is_reported() {
        assert!(reports_focus_change(20, Some(20)));
    }

    #[test]
    fn foc_5_no_window_is_other() {
        assert_eq!(
            pointer_target(&["AXButton".to_string()], None),
            PointerTarget::Other
        );
    }
}
