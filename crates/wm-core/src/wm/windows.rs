//! Window lifecycle: created, destroyed, frame changed (spec 6.5).

use super::Wm;
use crate::{
    App, Command, Monitor, Placement, Point, Rect, ResizeStep, UsableRect, WindowId, WindowInfo,
    WindowKind, WorkspaceNumber, tree,
};

/// Share of the monitor a window gets the first time it floats.
const FIRST_FLOAT_SHARE: f64 = 0.6;

/// Where a window floats when core picks the spot: centered, `FIRST_FLOAT_SHARE` of `area`.
pub(super) fn first_float_frame(area: Rect) -> Rect {
    let (width, height) = (
        area.width * FIRST_FLOAT_SHARE,
        area.height * FIRST_FLOAT_SHARE,
    );
    Rect::new(
        area.x + (area.width - width) / 2.0,
        area.y + (area.height - height) / 2.0,
        width,
        height,
    )
}

impl Wm {
    /// Moves focus to `id` (or clears it). With `emit`, a `Focus` command is sent when
    /// the window wasn't already focused; clearing focus never sends anything.
    pub(super) fn set_focus(&mut self, id: Option<WindowId>, emit: bool) {
        let previous = self.focused_window;
        self.focused_window = id;
        let Some(id) = id else {
            return;
        };
        let Some(n) = self.windows.get(&id).map(|w| w.workspace) else {
            return;
        };
        if let Some(monitor) = self.workspace_location(n).cloned() {
            self.focused_monitor = Some(monitor);
        }
        if let Some(ws) = self.workspaces.get_mut(&n) {
            ws.last_focused = Some(id);
        }
        if emit && previous != Some(id) {
            self.focus_request = Some(id);
        }
    }

    /// The tiled window a new window in workspace `n` should split: the focused window if
    /// it is tiled there, else the workspace's last focused window if tiled, else `None`.
    pub(super) fn tile_target(&self, n: WorkspaceNumber) -> Option<WindowId> {
        let ws = self.workspaces.get(&n)?;
        let tree = ws.tree.as_ref()?;
        [self.focused_window, ws.last_focused]
            .into_iter()
            .flatten()
            .find(|id| tree.contains(*id))
    }

    /// Takes a window out of its workspace's tree or floating list. Returns the window that
    /// should get focus if this one had it: the one that took its space, else the last
    /// floating window, else whatever remains tiled.
    pub(super) fn detach_window(&mut self, id: WindowId) -> Option<WindowId> {
        let n = self.windows.get(&id)?.workspace;
        let ws = self.workspaces.get_mut(&n)?;
        let mut successor = None;
        if let Some(pos) = ws.floating.iter().position(|f| *f == id) {
            ws.floating.remove(pos);
        } else {
            successor = tree::remove(&mut ws.tree, id);
        }
        if ws.last_focused == Some(id) {
            ws.last_focused = None;
        }
        successor
            .or(ws.floating.last().copied())
            .or(ws.last_focused)
            .or(ws.tree.as_ref().map(|t| t.first_window()))
    }

    /// The monitor whose frame contains `point`.
    pub(super) fn monitor_at(&self, point: Point) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.frame.contains(point))
    }

    /// The visible workspace of the focused monitor.
    pub(super) fn focused_workspace(&self) -> Option<WorkspaceNumber> {
        self.focused_monitor
            .as_ref()
            .and_then(|m| self.visible_workspace(m))
    }

    /// WIN-1, WIN-3: a new window joins the focused workspace, wherever the app opened it.
    pub(super) fn on_window_created(&mut self, info: WindowInfo) {
        if self.windows.contains_key(&info.id) {
            return;
        }
        let Some(n) = self.focused_workspace() else {
            return;
        };
        let target = self.tile_target(n);
        self.add_window(info, n, target);
        if info.kind == WindowKind::Popup {
            return;
        }
        // No usable frame stays unknown (SAFE-1).
        if let Some(frame) = info.frame
            && self.windows.get(&info.id).is_some_and(|w| w.floating)
        {
            let frame = self.translate_to_workspace(frame.rect(), n);
            if let Some(window) = self.windows.get_mut(&info.id) {
                window.floating_frame = Some(frame);
            }
        }
        self.set_focus(Some(info.id), true);
    }

    /// Moves `frame` from the monitor it is on to the monitor workspace `n` lives on,
    /// keeping its offset, then keeps it inside that monitor.
    pub(super) fn translate_to_workspace(&self, frame: Rect, n: WorkspaceNumber) -> Rect {
        let Some(to) = self.workspace_area(n) else {
            return frame;
        };
        match self.monitor_at(frame.center()) {
            Some(from) if from.frame != to => frame
                .translate(to.x - from.frame.x, to.y - from.frame.y)
                .clamp_inside(&to),
            _ => frame,
        }
    }

    /// LAY-2: the window's space goes to its sibling; focus follows if it was focused.
    pub(super) fn on_window_destroyed(&mut self, id: WindowId) {
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        let successor = if window.kind == WindowKind::Popup {
            None
        } else {
            self.detach_window(id)
        };
        self.windows.remove(&id);
        self.placements.remove(&id);
        if self.focused_window == Some(id) {
            self.focused_window = None;
            self.set_focus(successor, true);
        }
    }

    /// Inserts a tiled window into workspace `n` next to `target` and records the move.
    /// The window must already be detached from its previous workspace.
    pub(super) fn insert_tiled(
        &mut self,
        id: WindowId,
        n: WorkspaceNumber,
        target: Option<WindowId>,
    ) {
        let Some(area) = self.workspace_area(n) else {
            return;
        };
        if let Some(ws) = self.workspaces.get_mut(&n) {
            tree::insert(&mut ws.tree, target, id, area);
        }
        if let Some(window) = self.windows.get_mut(&id) {
            window.workspace = n;
        }
    }

    /// KEY-5: floats a tiled window (at its last floating frame, or centered at 60% of the
    /// monitor), or tiles a floating one next to the workspace's most recent tiled window.
    pub(super) fn toggle_floating(&mut self) {
        let Some(id) = self.focused_window else {
            return;
        };
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        let (n, floating, last_frame) = (window.workspace, window.floating, window.floating_frame);
        let Some(area) = self.workspace_area(n) else {
            return;
        };
        if floating {
            if let Some(ws) = self.workspaces.get_mut(&n) {
                ws.floating.retain(|f| *f != id);
            }
            let target = self.tile_target(n);
            self.insert_tiled(id, n, target);
            if let Some(window) = self.windows.get_mut(&id) {
                window.floating = false;
            }
        } else {
            let frame = last_frame.unwrap_or_else(|| first_float_frame(area));
            if let Some(ws) = self.workspaces.get_mut(&n) {
                tree::remove(&mut ws.tree, id);
                ws.floating.push(id);
            }
            if let Some(window) = self.windows.get_mut(&id) {
                window.floating = true;
                window.floating_frame = Some(frame);
            }
        }
    }

    /// The focused window, if it is tiled, with its workspace.
    pub(super) fn focused_tiled(&self) -> Option<(WindowId, WorkspaceNumber)> {
        let id = self.focused_window?;
        let window = self.windows.get(&id)?;
        (!window.floating).then_some((id, window.workspace))
    }

    /// KEY-9: grows or shrinks the focused window's side of its split.
    pub(super) fn resize_focused(&mut self, step: ResizeStep) {
        if let Some((id, n)) = self.focused_tiled()
            && let Some(ws) = self.workspaces.get_mut(&n)
        {
            tree::resize(&mut ws.tree, id, step);
        }
    }

    /// KEY-10: flips the direction of the focused window's split.
    pub(super) fn toggle_split_focused(&mut self) {
        if let Some((id, n)) = self.focused_tiled()
            && let Some(ws) = self.workspaces.get_mut(&n)
        {
            tree::toggle_split(&mut ws.tree, id);
        }
    }

    /// KEY-11: asks the app to close the window. State changes only when `WindowDestroyed`
    /// arrives, because the app may refuse (e.g. unsaved changes).
    pub(super) fn close_focused(&mut self) {
        if let Some(id) = self.focused_window {
            self.out.push(Command::Close(id));
        }
    }

    /// KEY-6: the new window arrives later as a normal `WindowCreated` (WIN-3).
    pub(super) fn launch(&mut self, app: App) {
        self.out.push(Command::LaunchNewWindow(app));
    }

    /// Tiled windows snap back to their layout frame; floating windows remember where they
    /// were moved, and change workspace if dragged onto another monitor.
    pub(super) fn on_window_frame_changed(&mut self, id: WindowId, frame: UsableRect) {
        // An app can report anything; only a usable frame gets this far (SAFE-1, HARD-7).
        let frame = frame.rect();
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        if window.kind == WindowKind::Popup || !self.is_visible(window.workspace) {
            return;
        }
        // Recording what macOS reports makes reconcile resend the layout frame for tiled windows.
        self.placements.insert(id, Placement::At(frame));
        if !window.floating {
            return;
        }
        let old_n = window.workspace;
        // The first usable frame of a window that had none (confirmed late, SAFE-3) is where
        // the app opened it, not a drag: place it like a new window (WIN-1, WIN-3).
        if window.floating_frame.is_none() {
            let frame = self.translate_to_workspace(frame, old_n);
            if let Some(window) = self.windows.get_mut(&id) {
                window.floating_frame = Some(frame);
            }
            return;
        }
        if let Some(window) = self.windows.get_mut(&id) {
            window.floating_frame = Some(frame);
        }
        let Some(new_n) = self.monitor_at(frame.center()).map(|m| m.visible) else {
            return;
        };
        if new_n == old_n {
            return;
        }
        if let Some(ws) = self.workspaces.get_mut(&old_n) {
            ws.floating.retain(|f| *f != id);
            if ws.last_focused == Some(id) {
                ws.last_focused = None;
            }
        }
        if let Some(ws) = self.workspaces.get_mut(&new_n) {
            ws.floating.push(id);
        }
        if let Some(window) = self.windows.get_mut(&id) {
            window.workspace = new_n;
        }
        if self.focused_window == Some(id) {
            self.set_focus(Some(id), false);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    fn one() -> Wm {
        started(vec![main_monitor()], vec![])
    }

    #[test]
    fn win_3_new_window_goes_to_focused_workspace() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        let cmds = wm.handle(Event::WindowCreated(window(
            1,
            101,
            WindowKind::Normal,
            r(1200., 100., 200., 200.),
        )));
        assert_eq!(
            cmds,
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 790.)
                },
                Command::Focus(w(1))
            ]
        );
        assert_eq!(wm.window(w(1)).unwrap().workspace, ws(1));
    }

    #[test]
    fn lay_1_second_window_splits_focused_window() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(
            wm.handle(Event::WindowCreated(normal(2))),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 493., 790.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(502., 5., 493., 790.)
                },
                Command::Focus(w(2)),
            ]
        );
    }

    #[test]
    fn floating_window_keeps_its_frame() {
        let mut wm = one();
        let cmds = wm.handle(Event::WindowCreated(window(
            3,
            103,
            WindowKind::Floating,
            r(100., 100., 300., 200.),
        )));
        assert_eq!(cmds, vec![Command::Focus(w(3))]);
        assert!(wm.window(w(3)).unwrap().floating);
    }

    #[test]
    fn win_3_floating_window_from_other_monitor_is_translated() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        let cmds = wm.handle(Event::WindowCreated(window(
            3,
            103,
            WindowKind::Floating,
            r(1100., 100., 300., 200.),
        )));
        assert_eq!(
            cmds,
            vec![
                Command::SetFrame {
                    id: w(3),
                    frame: r(100., 100., 300., 200.)
                },
                Command::Focus(w(3))
            ]
        );
    }

    #[test]
    fn popup_created_emits_nothing_and_keeps_focus() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(wm.handle(Event::WindowCreated(popup_of(10, 101))), vec![]);
        assert_eq!(wm.focused_window(), Some(w(1)));
    }

    #[test]
    fn duplicate_window_created_is_ignored() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(wm.handle(Event::WindowCreated(normal(1))), vec![]);
    }

    #[test]
    fn lay_2_destroy_gives_space_and_focus_to_sibling() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(normal(2)));
        assert_eq!(
            wm.handle(Event::WindowDestroyed(w(2))),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 790.)
                },
                Command::Focus(w(1))
            ]
        );
    }

    #[test]
    fn unknown_window_events_are_ignored() {
        let mut wm = one();
        assert_eq!(wm.handle(Event::WindowDestroyed(w(42))), vec![]);
        assert_eq!(
            wm.handle(Event::WindowFrameChanged {
                id: w(42),
                frame: ur(0., 0., 1., 1.)
            }),
            vec![]
        );
        assert_eq!(wm.handle(Event::WindowFocused(w(42))), vec![]);
    }

    #[test]
    fn tiled_window_dragged_snaps_back() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(
            wm.handle(Event::WindowFrameChanged {
                id: w(1),
                frame: ur(50., 50., 300., 300.)
            }),
            vec![Command::SetFrame {
                id: w(1),
                frame: r(5., 5., 990., 790.)
            }]
        );
    }

    #[test]
    fn floating_window_moved_is_remembered() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(window(
            3,
            103,
            WindowKind::Floating,
            r(100., 100., 300., 200.),
        )));
        assert_eq!(
            wm.handle(Event::WindowFrameChanged {
                id: w(3),
                frame: ur(200., 200., 300., 200.)
            }),
            vec![]
        );
        assert_eq!(
            wm.window(w(3)).unwrap().floating_frame,
            Some(r(200., 200., 300., 200.))
        );
    }

    #[test]
    fn floating_window_dragged_to_other_monitor_changes_workspace() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        wm.handle(Event::WindowCreated(window(
            3,
            103,
            WindowKind::Floating,
            r(100., 100., 300., 200.),
        )));
        assert_eq!(
            wm.handle(Event::WindowFrameChanged {
                id: w(3),
                frame: ur(1200., 100., 300., 200.)
            }),
            vec![]
        );
        assert_eq!(wm.window(w(3)).unwrap().workspace, ws(2));
    }

    fn act(wm: &mut Wm, a: Action) -> Vec<Command> {
        wm.handle(Event::Action(a))
    }

    /// windows 1 (left) and 2 (right), 2 focused
    fn two() -> Wm {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(normal(2)));
        wm
    }

    fn approx(a: Rect, b: Rect) -> bool {
        (a.x - b.x).abs() < 1e-9
            && (a.y - b.y).abs() < 1e-9
            && (a.width - b.width).abs() < 1e-9
            && (a.height - b.height).abs() < 1e-9
    }

    #[test]
    fn key_5_toggle_floating_centers_at_60_percent() {
        let mut wm = two();
        assert_eq!(
            act(&mut wm, Action::ToggleFloating),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 790.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(200., 160., 600., 480.)
                },
            ]
        );
        assert!(wm.window(w(2)).unwrap().floating);
    }

    #[test]
    fn key_5_toggle_back_retiles_and_remembers_float_frame() {
        let mut wm = two();
        act(&mut wm, Action::ToggleFloating);
        assert_eq!(
            act(&mut wm, Action::ToggleFloating),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 493., 790.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(502., 5., 493., 790.)
                },
            ]
        );
        assert_eq!(
            wm.window(w(2)).unwrap().floating_frame,
            Some(r(200., 160., 600., 480.))
        );
    }

    #[test]
    fn key_8_swap_within_workspace() {
        let mut wm = two();
        assert_eq!(
            act(&mut wm, Action::Swap(Direction::Left)),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(502., 5., 493., 790.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(5., 5., 493., 790.)
                },
                Command::WarpPointer(p(251.5, 400.)),
            ]
        );
        assert_eq!(wm.focused_window(), Some(w(2)));
    }

    #[test]
    fn key_8_swap_at_edge_moves_window_to_next_monitor() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(
            act(&mut wm, Action::Swap(Direction::Right)),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(1005., 5., 990., 790.)
                },
                Command::WarpPointer(p(1500., 400.)),
            ]
        );
        assert_eq!(wm.window(w(1)).unwrap().workspace, ws(2));
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    }

    #[test]
    fn key_8_swap_into_neighbor_on_other_monitor() {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                2,
                102,
                WindowKind::Normal,
                r(1200., 100., 200., 200.),
            )],
        );
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(
            act(&mut wm, Action::Swap(Direction::Right)),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(1502., 5., 493., 790.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(1005., 5., 493., 790.)
                },
                Command::WarpPointer(p(1748.5, 400.)),
            ]
        );
    }

    #[test]
    fn key_9_resize_grows_focused_side() {
        let mut wm = two();
        let cmds = act(&mut wm, Action::Resize(ResizeStep::Grow));
        let frame_of = |id| {
            cmds.iter()
                .find_map(|c| match c {
                    Command::SetFrame { id: i, frame } if *i == w(id) => Some(*frame),
                    _ => None,
                })
                .unwrap()
        };
        assert!(approx(frame_of(1), r(5., 5., 443.5, 790.)));
        assert!(approx(frame_of(2), r(452.5, 5., 542.5, 790.)));
    }

    #[test]
    fn key_10_toggle_split_stacks_windows() {
        let mut wm = two();
        assert_eq!(
            act(&mut wm, Action::ToggleSplit),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 393.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(5., 402., 990., 393.)
                },
            ]
        );
    }

    #[test]
    fn key_11_close_emits_close_and_keeps_state() {
        let mut wm = two();
        assert_eq!(act(&mut wm, Action::Close), vec![Command::Close(w(2))]);
        assert!(wm.window(w(2)).is_some());
    }

    #[test]
    fn key_6_launch_emits_launch_even_without_focus() {
        let mut wm = one();
        assert_eq!(
            act(
                &mut wm,
                Action::Launch(App::Bundle("com.apple.Terminal".into()))
            ),
            vec![Command::LaunchNewWindow(App::Bundle(
                "com.apple.Terminal".into()
            ))]
        );
    }

    #[test]
    fn actions_without_focused_window_are_noops() {
        let mut wm = one();
        for a in [
            Action::Swap(Direction::Left),
            Action::Resize(ResizeStep::Grow),
            Action::ToggleSplit,
            Action::ToggleFloating,
            Action::Close,
        ] {
            assert_eq!(act(&mut wm, a), vec![]);
        }
    }

    #[test]
    fn key_5_refloat_after_moving_monitor_uses_new_monitor() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        wm.handle(Event::WindowCreated(normal(3)));
        act(&mut wm, Action::ToggleFloating);
        act(&mut wm, Action::ToggleFloating);
        act(&mut wm, Action::MoveToWorkspace(ws(1)));
        let cmds = act(&mut wm, Action::ToggleFloating);
        assert_eq!(
            cmds,
            vec![Command::SetFrame {
                id: w(3),
                frame: r(200., 160., 600., 480.)
            }]
        );
    }

    #[test]
    fn frame_within_tolerance_is_not_resent() {
        let mut wm = one();
        wm.handle(Event::WindowCreated(normal(1)));
        assert_eq!(
            wm.handle(Event::WindowFrameChanged {
                id: w(1),
                frame: ur(5., 5., 990., 790.4)
            }),
            vec![]
        );
    }
}
