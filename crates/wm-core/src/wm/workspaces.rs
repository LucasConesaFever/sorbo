//! Workspace actions: go to, move to, next/previous (spec 6.2).

use super::Wm;
use crate::{PointerTarget, WindowId, WorkspaceNumber, tree};

impl Wm {
    /// Makes `n` visible on the monitor it lives on and focuses that monitor. Focus goes to
    /// `focus`, else the workspace's last focused window, else its first window, else nothing.
    /// The pointer warps to the focused window unless it is already over it.
    /// `emit_focus` is false when macOS already focused the window itself.
    pub(super) fn show_workspace(
        &mut self,
        n: WorkspaceNumber,
        focus: Option<WindowId>,
        emit_focus: bool,
    ) {
        let Some(location) = self.workspace_location(n).cloned() else {
            return;
        };
        if let Some(monitor) = self.monitor_mut(&location) {
            monitor.visible = n;
        }
        self.focused_monitor = Some(location);

        let Some(ws) = self.workspaces.get(&n) else {
            return;
        };
        let target = focus
            .or(ws.last_focused.filter(|id| ws.contains(*id)))
            .or(ws.tree.as_ref().map(|t| t.first_window()))
            .or(ws.floating.last().copied());
        self.set_focus(target, emit_focus);
        if let Some(id) = target
            && self.pointer_over != Some(PointerTarget::Window(id))
        {
            self.warp_request = Some(id);
        }
    }

    /// KEY-1: creates `n` on the focused monitor if it doesn't exist, then shows it.
    pub(super) fn go_to_workspace(&mut self, n: WorkspaceNumber) {
        if !self.workspaces.contains_key(&n) {
            let Some(home) = self.focused_monitor.clone() else {
                return;
            };
            self.create_workspace(n, home);
        }
        self.show_workspace(n, None, true);
    }

    /// KEY-2: moves the focused window to `n` and follows it. A new `n` is created on the
    /// monitor the window was on (WS-8).
    pub(super) fn move_to_workspace(&mut self, n: WorkspaceNumber) {
        let Some(id) = self.focused_window else {
            return;
        };
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        let (from_n, floating) = (window.workspace, window.floating);
        if from_n == n {
            return;
        }
        if !self.workspaces.contains_key(&n) {
            let Some(home) = self.workspace_location(from_n).cloned() else {
                return;
            };
            self.create_workspace(n, home);
        }

        self.detach_window(id);
        let new_frame = self
            .windows
            .get(&id)
            .and_then(|w| w.floating_frame)
            .map(|frame| self.translate_to_workspace(frame, n));
        let target = self.tile_target(n);
        let area = self.workspace_area(n);
        if let Some(ws) = self.workspaces.get_mut(&n) {
            if floating {
                ws.floating.push(id);
            } else if let Some(area) = area {
                tree::insert(&mut ws.tree, target, id, area);
            }
        }
        if let Some(window) = self.windows.get_mut(&id) {
            window.workspace = n;
            if floating {
                window.floating_frame = new_frame;
            }
        }
        self.show_workspace(n, Some(id), true);
    }

    /// KEY-3 / WS-4: shows the next (or previous) workspace living on the focused monitor,
    /// wrapping around at the ends.
    pub(super) fn cycle_workspace(&mut self, forward: bool) {
        let Some(monitor) = self.focused_monitor.clone() else {
            return;
        };
        let Some(current) = self.visible_workspace(&monitor) else {
            return;
        };
        let on_monitor: Vec<WorkspaceNumber> = self
            .workspaces
            .keys()
            .copied()
            .filter(|n| self.workspace_location(*n) == Some(&monitor))
            .collect();
        if on_monitor.len() < 2 {
            return;
        }
        let Some(pos) = on_monitor.iter().position(|n| *n == current) else {
            return;
        };
        let len = on_monitor.len();
        let next = if forward {
            (pos + 1) % len
        } else {
            (pos + len - 1) % len
        };
        self.show_workspace(on_monitor[next], None, true);
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    fn act(wm: &mut Wm, a: Action) -> Vec<Command> {
        wm.handle(Event::Action(a))
    }

    fn one() -> Wm {
        started(vec![main_monitor()], vec![])
    }

    fn ext_with_window() -> Wm {
        started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                2,
                102,
                WindowKind::Normal,
                r(1200., 100., 200., 200.),
            )],
        )
    }

    #[test]
    fn key_1_goto_new_workspace_creates_it_on_focused_monitor() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(
            act(&mut wm, Action::GoToWorkspace(ws(3))),
            vec![Command::Hide(w(1))]
        );
        assert_eq!(wm.workspace(ws(3)).unwrap().home, "main".into());
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(3)));
        assert_eq!(wm.focused_window(), None);
    }

    #[test]
    fn ws_5_empty_workspace_removed_when_switched_away() {
        let mut wm = one();
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        assert_eq!(nums(&wm), vec![2]);
    }

    #[test]
    fn ws_6_visible_empty_workspace_is_kept() {
        assert_eq!(nums(&one()), vec![1]);
    }

    #[test]
    fn goto_hidden_workspace_shows_it_and_restores_focus() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        wm.handle(Event::WindowCreated(normal(2)));
        assert_eq!(
            act(&mut wm, Action::GoToWorkspace(ws(1))),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 790.)
                },
                Command::Hide(w(2)),
                Command::Focus(w(1)),
                Command::WarpPointer(p(500., 400.)),
            ]
        );
    }

    #[test]
    fn goto_workspace_visible_on_other_monitor_moves_focus() {
        let mut wm = ext_with_window();
        assert_eq!(
            act(&mut wm, Action::GoToWorkspace(ws(2))),
            vec![Command::Focus(w(2)), Command::WarpPointer(p(1500., 400.))]
        );
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    }

    #[test]
    fn key_2_move_to_workspace_follows_window() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(normal(2)));
        assert_eq!(
            act(&mut wm, Action::MoveToWorkspace(ws(3))),
            vec![
                Command::Hide(w(1)),
                Command::SetFrame {
                    id: w(2),
                    frame: r(5., 5., 990., 790.)
                },
                Command::WarpPointer(p(500., 400.)),
            ]
        );
        assert_eq!(wm.window(w(2)).unwrap().workspace, ws(3));
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(3)));
    }

    #[test]
    fn ws_8_move_to_new_workspace_creates_it_on_windows_monitor() {
        let mut wm = ext_with_window();
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        act(&mut wm, Action::MoveToWorkspace(ws(5)));
        assert_eq!(wm.workspace_location(ws(5)), Some(&"ext".into()));
    }

    #[test]
    fn move_without_focused_window_is_noop() {
        let mut wm = one();
        assert_eq!(act(&mut wm, Action::MoveToWorkspace(ws(3))), vec![]);
        assert_eq!(nums(&wm), vec![1]);
    }

    #[test]
    fn move_to_current_workspace_is_noop() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(act(&mut wm, Action::MoveToWorkspace(ws(1))), vec![]);
    }

    #[test]
    fn floating_window_moved_to_other_monitor_is_translated() {
        let mut wm = ext_with_window();
        wm.handle(Event::WindowCreated(window(
            3,
            103,
            WindowKind::Floating,
            r(100., 100., 300., 200.),
        )));
        act(&mut wm, Action::MoveToWorkspace(ws(2)));
        assert_eq!(
            wm.window(w(3)).unwrap().floating_frame,
            Some(r(1100., 100., 300., 200.))
        );
    }

    #[test]
    fn ws_4_next_and_prev_cycle_with_wrap() {
        let mut wm = one();
        for n in 1..=3 {
            act(&mut wm, Action::GoToWorkspace(ws(n)));
            wm.handle(Event::WindowCreated(normal(u64::from(n))));
        }
        act(&mut wm, Action::NextWorkspace);
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(1)));
        act(&mut wm, Action::PrevWorkspace);
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(3)));
        act(&mut wm, Action::PrevWorkspace);
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(2)));
    }

    #[test]
    fn ws_4_cycle_with_single_workspace_is_noop() {
        let mut wm = one();
        assert_eq!(act(&mut wm, Action::NextWorkspace), vec![]);
    }
}
