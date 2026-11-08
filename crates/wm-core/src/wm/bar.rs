//! The menu bar's workspace row (BAR-1 to BAR-3): which workspaces exist and how each one looks.

use super::Wm;
use crate::{WorkspaceState, WorkspaceStatus};

impl Wm {
    /// Every existing workspace in number order, with how it looks (BAR-1, BAR-2).
    /// Empty when no monitor is connected.
    pub fn workspace_row(&self) -> Vec<WorkspaceStatus> {
        if self.monitors.is_empty() {
            return Vec::new();
        }
        self.workspaces
            .keys()
            .map(|&number| {
                let shown_on = self.monitors.iter().find(|m| m.visible == number);
                let state = match shown_on {
                    Some(m) if Some(&m.id) == self.focused_monitor.as_ref() => {
                        WorkspaceState::Focused
                    }
                    Some(_) => WorkspaceState::Visible,
                    None => WorkspaceState::Hidden,
                };
                WorkspaceStatus { number, state }
            })
            .collect()
    }

    /// The row, if it differs from the one returned last time (or none was returned yet) (BAR-3).
    pub fn take_row_change(&mut self) -> Option<Vec<WorkspaceStatus>> {
        let row = self.workspace_row();
        if self.last_row.as_ref() == Some(&row) {
            return None;
        }
        self.last_row = Some(row.clone());
        Some(row)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;
    use WorkspaceState::*;

    fn row(v: &[(u32, WorkspaceState)]) -> Vec<WorkspaceStatus> {
        v.iter()
            .map(|&(number, state)| WorkspaceStatus {
                number: ws(number),
                state,
            })
            .collect()
    }

    #[test]
    fn bar_1_lists_existing_workspaces_in_order() {
        let wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        assert_eq!(wm.workspace_row(), row(&[(1, Focused), (2, Visible)]));
    }

    #[test]
    fn bar_2_hidden_workspace_with_windows_is_hidden() {
        let mut wm = started(vec![main_monitor()], vec![normal(1)]);
        wm.handle(Event::Action(Action::GoToWorkspace(ws(3))));
        assert_eq!(wm.workspace_row(), row(&[(1, Hidden), (3, Focused)]));
    }

    #[test]
    fn bar_1_pruned_workspace_leaves_the_row() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::Action(Action::GoToWorkspace(ws(2))));
        assert_eq!(wm.workspace_row(), row(&[(2, Focused)]));
    }

    #[test]
    fn bar_3_focus_on_other_monitor_swaps_states() {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![
                window(1, 101, WindowKind::Normal, r(100., 100., 200., 200.)),
                window(2, 102, WindowKind::Normal, r(1200., 100., 200., 200.)),
            ],
        );
        wm.handle(Event::Action(Action::Focus(Direction::Right)));
        assert_eq!(wm.workspace_row(), row(&[(1, Visible), (2, Focused)]));
    }

    #[test]
    fn bar_3_first_row_is_reported() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        assert_eq!(wm.take_row_change(), Some(wm.workspace_row()));
    }

    #[test]
    fn bar_3_unchanged_row_is_not_reported_again() {
        let mut wm = started(vec![main_monitor()], vec![normal(1)]);
        assert!(wm.take_row_change().is_some());
        assert_eq!(wm.take_row_change(), None);
        wm.handle(Event::PointerMoved {
            position: p(500., 500.),
            target: PointerTarget::Other,
        });
        assert_eq!(wm.take_row_change(), None);
        wm.handle(Event::Action(Action::GoToWorkspace(ws(2))));
        assert!(wm.take_row_change().is_some());
    }

    #[test]
    fn bar_3_no_monitors_reports_empty_row() {
        // Before the first monitor snapshot there are no monitors and no workspaces.
        let mut wm = Wm::new();
        assert_eq!(wm.take_row_change(), Some(vec![]));
    }

    #[test]
    fn bar_3_empty_monitor_snapshot_keeps_the_row() {
        // Core ignores an empty snapshot (lid closed, sleep), so the row stays as it was.
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.take_row_change();
        wm.handle(Event::MonitorsChanged(vec![]));
        assert_eq!(wm.take_row_change(), None);
        assert_eq!(wm.workspace_row(), row(&[(1, Focused)]));
    }
}
