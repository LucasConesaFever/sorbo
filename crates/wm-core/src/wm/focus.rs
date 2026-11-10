//! Focus: follows the mouse without ever touching popups (spec 6.4).

use super::Wm;
use crate::{
    Direction, MonitorId, POSITION_TOLERANCE, Placement, Point, PointerTarget, WindowId,
    WindowKind, tree,
};

/// How far `to` lies in `direction` from `from` (primary), and how far off to the side
/// (perpendicular). `None` if `to` is not in that direction.
fn offset_in_direction(from: Point, to: Point, direction: Direction) -> Option<(f64, f64)> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let (primary, perpendicular) = match direction {
        Direction::Right => (dx, dy),
        Direction::Left => (-dx, dy),
        Direction::Down => (dy, dx),
        Direction::Up => (-dy, dx),
    };
    (primary > POSITION_TOLERANCE).then_some((primary, perpendicular.abs()))
}

/// The candidate nearest along `direction`; near-equal distances are broken by the smallest
/// sideways offset.
fn nearest_in_direction<T>(
    from: Point,
    direction: Direction,
    candidates: impl IntoIterator<Item = (T, Point)>,
) -> Option<T> {
    let mut best: Option<(T, f64, f64)> = None;
    for (item, center) in candidates {
        let Some((primary, perpendicular)) = offset_in_direction(from, center, direction) else {
            continue;
        };
        let better = match &best {
            None => true,
            Some((_, best_primary, best_perpendicular)) => {
                primary < best_primary - POSITION_TOLERANCE
                    || ((primary - best_primary).abs() <= POSITION_TOLERANCE
                        && perpendicular < *best_perpendicular)
            }
        };
        if better {
            best = Some((item, primary, perpendicular));
        }
    }
    best.map(|(item, _, _)| item)
}

impl Wm {
    /// FOC-1, FOC-2, FOC-4, FOC-5, FOC-10. Only crossing into a *different*, managed,
    /// non-popup window changes focus, or entering a monitor whose workspace is empty.
    /// Everything else (the same window, a popup, a menu, the desktop, an unknown window, the
    /// echo of our own warp) produces no command at all.
    pub(super) fn on_pointer_moved(&mut self, position: Point, target: PointerTarget) {
        let entered = self.pointer_entered_monitor(position);
        // FOC-4: the first event at the point we just warped to is that warp's echo.
        if let Some(warp) = self.pending_warp.take()
            && position.near(warp)
        {
            self.pointer_over = Some(target);
            return;
        }
        if let Some(monitor) = entered {
            self.focus_if_empty(monitor);
        }
        let target = self.floating_over_tile(position, target);
        // FOC-2: no crossing, nothing to do.
        if self.pointer_over == Some(target) {
            return;
        }
        self.pointer_over = Some(target);

        let PointerTarget::Window(id) = target else {
            return;
        };
        if self.focused_window == Some(id) {
            return;
        }
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        if window.kind == WindowKind::Popup || !self.is_visible(window.workspace) {
            return;
        }
        self.set_focus(Some(id), true);
    }

    /// The monitor the pointer just moved into, if it changed. Outside every monitor's usable
    /// area (a menu bar) the last one is kept, so a trip to the menu bar isn't a crossing.
    fn pointer_entered_monitor(&mut self, position: Point) -> Option<MonitorId> {
        let id = self.monitor_at(position)?.id.clone();
        if self.pointer_monitor.as_ref() == Some(&id) {
            return None;
        }
        self.pointer_monitor = Some(id.clone());
        Some(id)
    }

    /// FOC-10: entering a monitor whose visible workspace is empty focuses that monitor, with
    /// no window focused, so a new window opens there. A monitor with windows waits for the
    /// pointer to cross into one (FOC-1).
    fn focus_if_empty(&mut self, monitor: MonitorId) {
        if self.focused_monitor.as_ref() == Some(&monitor) {
            return;
        }
        let empty = self
            .visible_workspace(&monitor)
            .and_then(|n| self.workspaces.get(&n))
            .is_some_and(|ws| ws.is_empty());
        if empty {
            self.set_focus(None, false);
            self.focused_monitor = Some(monitor);
        }
    }

    /// FOC-9: a floating window's area belongs to it, as if it were above the tiles. macOS
    /// stacks the focused tile's app over other apps' floating windows, so when it reports a
    /// tiled window, a visible floating window whose frame contains `position` is the real
    /// target (the latest added wins). Any other target is returned unchanged.
    fn floating_over_tile(&self, position: Point, target: PointerTarget) -> PointerTarget {
        let PointerTarget::Window(id) = target else {
            return target;
        };
        let Some(window) = self.windows.get(&id) else {
            return target;
        };
        if window.floating || window.kind == WindowKind::Popup {
            return target;
        }
        self.monitors
            .iter()
            .filter_map(|m| self.workspaces.get(&m.visible))
            .flat_map(|ws| ws.floating.iter().rev())
            .find(|f| {
                self.windows
                    .get(f)
                    .and_then(|w| w.floating_frame)
                    .is_some_and(|frame| frame.contains(position))
            })
            .map_or(target, |&f| PointerTarget::Window(f))
    }

    /// The current center of a window that is on screen.
    pub(super) fn window_center(&self, id: WindowId) -> Option<Point> {
        match self.placements.get(&id) {
            Some(Placement::At(rect)) => Some(rect.center()),
            _ => None,
        }
    }

    /// Where directional focus and swap start from: the focused window's center, else the
    /// focused monitor's center.
    pub(super) fn focus_origin(&self) -> Option<Point> {
        self.focused_window
            .and_then(|id| self.window_center(id))
            .or_else(|| {
                let m = self.monitor(self.focused_monitor.as_ref()?)?;
                Some(m.frame.center())
            })
    }

    /// The nearest window in `direction` among windows on visible workspaces (all monitors),
    /// excluding the focused one. Popups never count; floating windows only if not `tiled_only`.
    pub(super) fn find_in_direction(
        &self,
        from: Point,
        direction: Direction,
        tiled_only: bool,
    ) -> Option<WindowId> {
        let mut candidates = Vec::new();
        for monitor in &self.monitors {
            let Some(ws) = self.workspaces.get(&monitor.visible) else {
                continue;
            };
            let mut ids: Vec<WindowId> = ws.tree.as_ref().map(|t| t.windows()).unwrap_or_default();
            if !tiled_only {
                ids.extend(ws.floating.iter().copied());
            }
            for id in ids {
                if Some(id) == self.focused_window {
                    continue;
                }
                if let Some(center) = self.window_center(id) {
                    candidates.push((id, center));
                }
            }
        }
        nearest_in_direction(from, direction, candidates)
    }

    /// The nearest other monitor in `direction`, by frame center.
    pub(super) fn monitor_in_direction(
        &self,
        from: Point,
        direction: Direction,
    ) -> Option<MonitorId> {
        let candidates = self
            .monitors
            .iter()
            .filter(|m| Some(&m.id) != self.focused_monitor.as_ref())
            .map(|m| (m.id.clone(), m.frame.center()));
        nearest_in_direction(from, direction, candidates)
    }

    /// KEY-4, FOC-3: focus the nearest window in `direction` and warp the pointer to it.
    /// With no window there, an empty monitor in that direction gets focus instead.
    pub(super) fn focus_direction(&mut self, direction: Direction) {
        let Some(origin) = self.focus_origin() else {
            return;
        };
        if let Some(id) = self.find_in_direction(origin, direction, false) {
            self.set_focus(Some(id), true);
            self.warp_request = Some(id);
            return;
        }
        let Some(monitor) = self.monitor_in_direction(origin, direction) else {
            return;
        };
        let empty = self
            .visible_workspace(&monitor)
            .and_then(|n| self.workspaces.get(&n))
            .is_some_and(|ws| ws.is_empty());
        if empty {
            self.set_focus(None, false);
            self.focused_monitor = Some(monitor);
        }
    }

    /// KEY-8: swaps the focused tiled window with its neighbor in `direction`. A neighbor on
    /// another monitor gets the window inserted next to it; with no neighbor, a monitor in that
    /// direction takes the window. Focus stays on the window and the pointer follows it.
    pub(super) fn swap_direction(&mut self, direction: Direction) {
        let Some(id) = self.focused_window else {
            return;
        };
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        if window.floating {
            return;
        }
        let from_n = window.workspace;
        let Some(origin) = self.window_center(id) else {
            return;
        };

        if let Some(neighbor) = self.find_in_direction(origin, direction, true) {
            let Some(neighbor_n) = self.windows.get(&neighbor).map(|w| w.workspace) else {
                return;
            };
            if neighbor_n == from_n {
                if let Some(ws) = self.workspaces.get_mut(&from_n) {
                    tree::swap(&mut ws.tree, id, neighbor);
                }
            } else {
                self.detach_window(id);
                self.insert_tiled(id, neighbor_n, Some(neighbor));
            }
        } else if let Some(monitor) = self.monitor_in_direction(origin, direction) {
            let Some(n) = self.visible_workspace(&monitor) else {
                return;
            };
            self.detach_window(id);
            let target = self.tile_target(n);
            self.insert_tiled(id, n, target);
        } else {
            return;
        }
        self.set_focus(Some(id), false);
        self.warp_request = Some(id);
    }

    /// macOS focused a window by itself (click, Cmd+Tab, Cmd+`, Dock). Our own `Focus` echo
    /// is ignored; a popup of another app clears focus, one of the focused app keeps it
    /// (HARD-2). A window on a hidden workspace brings that workspace into view (FOC-7). The
    /// pointer warps to the window unless it is already over it.
    pub(super) fn on_window_focused(&mut self, id: WindowId) {
        if self.focused_window == Some(id) {
            return;
        }
        let Some(window) = self.windows.get(&id) else {
            return;
        };
        if window.kind == WindowKind::Popup {
            // Another app's "popup" may really be a window classified while its app was busy
            // (HARD-2): keeping focus on the previous window would make ⌃⌥Q close it. A popup
            // of the focused window's own app (a menu, a completion list) leaves focus alone.
            let pid = window.pid;
            let same_app = self
                .focused_window
                .and_then(|f| self.windows.get(&f))
                .is_some_and(|f| f.pid == pid);
            if !same_app {
                self.set_focus(None, false);
            }
            return;
        }
        let n = window.workspace;
        if self.is_visible(n) {
            self.set_focus(Some(id), false);
        } else {
            self.show_workspace(n, Some(id), false);
        }
        if self.pointer_over != Some(PointerTarget::Window(id)) {
            self.warp_request = Some(id);
        }
    }

    /// macOS focus went to something core doesn't manage. The pointer hasn't moved, so a
    /// nudge inside the same window is still not a crossing.
    pub(super) fn on_focus_left(&mut self) {
        self.set_focus(None, false);
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    fn moved(wm: &mut Wm, x: f64, y: f64, t: PointerTarget) -> Vec<Command> {
        wm.handle(Event::PointerMoved {
            position: p(x, y),
            target: t,
        })
    }

    fn win(n: u64) -> PointerTarget {
        PointerTarget::Window(w(n))
    }

    /// windows 1 (left) and 2 (right), 2 focused
    fn two() -> Wm {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(normal(2)));
        wm
    }

    /// A normal window that a rule makes float, at `frame`.
    fn floating_at(id: u64, frame: Rect) -> WindowInfo {
        let mut info = window(id, 100 + id as i32, WindowKind::Normal, frame);
        info.rule = RuleEffect {
            floating: Some(true),
        };
        info
    }

    /// Tiled window 1 fills the screen; floating windows 2 (and 3 if `two_floats`) sit over it.
    /// Window 1 is focused, so on macOS it covers the floating windows.
    fn tile_over_floats(two_floats: bool) -> Wm {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(floating_at(
            2,
            r(100., 100., 300., 200.),
        )));
        if two_floats {
            wm.handle(Event::WindowCreated(floating_at(
                3,
                r(200., 150., 300., 200.),
            )));
        }
        moved(&mut wm, 800., 700., win(1));
        assert_eq!(wm.focused_window(), Some(w(1)));
        wm
    }

    fn sets_frame_of(commands: &[Command], id: WindowId) -> bool {
        commands
            .iter()
            .any(|c| matches!(c, Command::SetFrame { id: i, .. } if *i == id))
    }

    #[test]
    fn safe_1_infinite_floating_frame_never_takes_focus() {
        let mut wm = two();
        wm.handle(Event::WindowCreated(floating_at(
            9,
            r(5000., 5000., f64::INFINITY, f64::INFINITY),
        )));
        let into_2 = moved(&mut wm, 750., 400., win(2));
        assert!(!into_2.contains(&Command::Focus(w(9))), "{into_2:?}");
        assert!(!sets_frame_of(&into_2, w(9)), "{into_2:?}");
        let into_1 = moved(&mut wm, 250., 400., win(1));
        assert!(into_1.contains(&Command::Focus(w(1))), "{into_1:?}");
        assert!(!sets_frame_of(&into_1, w(9)), "{into_1:?}");
    }

    #[test]
    fn safe_1_nan_frame_change_is_ignored() {
        let mut wm = tile_over_floats(false);
        // A NaN frame can't even be sent: `WindowFrameChanged` only carries a `UsableRect`
        // (HARD-7), and wm-macos drops a frame `UsableRect::new` refuses.
        assert_eq!(UsableRect::new(r(100., 100., f64::NAN, 200.)), None);
        let out = moved(&mut wm, 800., 700., win(1));
        assert!(!sets_frame_of(&out, w(2)), "{out:?}");
        // The last usable frame still owns its area (FOC-9).
        assert_eq!(
            moved(&mut wm, 150., 150., win(1)),
            vec![Command::Focus(w(2))]
        );
    }

    #[test]
    fn safe_1_window_added_with_unusable_frame_is_left_alone() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        let created = wm.handle(Event::WindowCreated(floating_at(
            9,
            r(0., 0., f64::INFINITY, f64::INFINITY),
        )));
        assert!(!sets_frame_of(&created, w(9)), "{created:?}");
        // Left where it is, not hidden: the frame may just not be confirmed yet (SAFE-3).
        assert!(!created.contains(&Command::Hide(w(9))), "{created:?}");
        let later = moved(&mut wm, 800., 700., win(1));
        assert!(!later.contains(&Command::Hide(w(9))), "{later:?}");
        let out = moved(&mut wm, 150., 150., win(1));
        assert!(!out.contains(&Command::Focus(w(9))), "{out:?}");
    }

    #[test]
    fn safe_1_frameless_floating_window_comes_back_after_a_workspace_round_trip() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(floating_at(9, r(0., 0., 0., 0.))));
        wm.handle(Event::Action(Action::GoToWorkspace(ws(2))));
        let back = wm.handle(Event::Action(Action::GoToWorkspace(ws(1))));
        assert!(sets_frame_of(&back, w(9)), "{back:?}");
    }

    #[test]
    fn safe_3_late_first_frame_keeps_the_window_on_its_workspace() {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                5,
                50,
                WindowKind::Normal,
                r(1100., 100., 500., 500.),
            )],
        );
        moved(&mut wm, 1300., 300., win(5));
        let ext_ws = wm.window(w(5)).unwrap().workspace;
        wm.handle(Event::WindowCreated(floating_at(9, r(0., 0., 0., 0.))));
        assert_eq!(wm.window(w(9)).unwrap().workspace, ext_ws);
        // Confirmed late, at where the app opened it (on main): like a new window, it stays
        // on the workspace it joined and is brought onto that monitor (WIN-1, WIN-3).
        wm.handle(Event::WindowFrameChanged {
            id: w(9),
            frame: ur(100., 100., 300., 200.),
        });
        let window = wm.window(w(9)).unwrap();
        assert_eq!(window.workspace, ext_ws);
        let frame = window.floating_frame.unwrap();
        assert!(ext_monitor().frame.contains(frame.center()), "{frame:?}");
    }

    #[test]
    fn safe_2_huge_floating_frame_is_capped_to_its_monitor() {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                5,
                50,
                WindowKind::Normal,
                r(1100., 100., 500., 500.),
            )],
        );
        let created = wm.handle(Event::WindowCreated(floating_at(9, r(10., 40., 1e9, 1e9))));
        assert!(
            created.contains(&Command::SetFrame {
                id: w(9),
                frame: r(0., 0., 1000., 800.)
            }),
            "{created:?}"
        );
        // The capped window owns its own monitor only.
        let out = moved(&mut wm, 1500., 400., win(5));
        assert!(!out.contains(&Command::Focus(w(9))), "{out:?}");
    }

    #[test]
    fn foc_9_pointer_over_covered_floating_area_focuses_the_floating_window() {
        let mut wm = tile_over_floats(false);
        assert_eq!(
            moved(&mut wm, 150., 150., win(1)),
            vec![Command::Focus(w(2))]
        );
        assert_eq!(wm.focused_window(), Some(w(2)));
    }

    #[test]
    fn foc_9_moving_within_the_floating_area_emits_nothing() {
        let mut wm = tile_over_floats(false);
        moved(&mut wm, 150., 150., win(1));
        assert_eq!(moved(&mut wm, 160., 160., win(2)), vec![]);
        assert_eq!(moved(&mut wm, 170., 170., win(1)), vec![]);
        assert_eq!(wm.focused_window(), Some(w(2)));
    }

    #[test]
    fn foc_9_leaving_the_floating_area_focuses_the_tile() {
        let mut wm = tile_over_floats(false);
        moved(&mut wm, 150., 150., win(2));
        assert_eq!(
            moved(&mut wm, 800., 700., win(1)),
            vec![Command::Focus(w(1))]
        );
    }

    #[test]
    fn foc_9_latest_floating_window_wins_where_they_overlap() {
        let mut wm = tile_over_floats(true);
        assert_eq!(
            moved(&mut wm, 250., 200., win(1)),
            vec![Command::Focus(w(3))]
        );
    }

    #[test]
    fn foc_9_popup_over_the_floating_area_still_emits_nothing() {
        let mut wm = tile_over_floats(false);
        wm.handle(Event::WindowCreated(popup_of(10, 101)));
        assert_eq!(moved(&mut wm, 150., 150., win(10)), vec![]);
        assert_eq!(wm.focused_window(), Some(w(1)));
    }

    #[test]
    fn foc_9_floating_window_on_a_hidden_workspace_is_ignored() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        wm.handle(Event::WindowCreated(floating_at(
            2,
            r(100., 100., 300., 200.),
        )));
        act(&mut wm, Action::GoToWorkspace(ws(1)));
        moved(&mut wm, 800., 700., PointerTarget::Other);
        assert_eq!(moved(&mut wm, 150., 150., win(1)), vec![]);
        assert_eq!(wm.focused_window(), Some(w(1)));
    }

    #[test]
    fn foc_1_crossing_into_other_window_focuses_it() {
        let mut wm = two();
        assert_eq!(
            moved(&mut wm, 100., 100., win(1)),
            vec![Command::Focus(w(1))]
        );
        assert_eq!(wm.focused_window(), Some(w(1)));
    }

    #[test]
    fn foc_2_moving_within_focused_window_emits_nothing() {
        let mut wm = two();
        assert_eq!(moved(&mut wm, 600., 100., win(2)), vec![]);
        assert_eq!(moved(&mut wm, 610., 110., win(2)), vec![]);
    }

    #[test]
    fn foc_5_pointer_over_popup_emits_nothing() {
        let mut wm = two();
        wm.handle(Event::WindowCreated(popup_of(10, 102)));
        assert_eq!(moved(&mut wm, 600., 100., win(10)), vec![]);
        assert_eq!(moved(&mut wm, 620., 100., win(2)), vec![]);
        assert_eq!(wm.focused_window(), Some(w(2)));
    }

    #[test]
    fn foc_5_pointer_over_other_or_unknown_emits_nothing() {
        let mut wm = two();
        assert_eq!(moved(&mut wm, 10., 10., PointerTarget::Other), vec![]);
        assert_eq!(moved(&mut wm, 10., 10., win(99)), vec![]);
        assert_eq!(wm.focused_window(), Some(w(2)));
    }

    #[test]
    fn focus_left_clears_focus_and_nudge_is_silent() {
        let mut wm = two();
        moved(&mut wm, 600., 100., win(2));
        assert_eq!(wm.handle(Event::FocusLeft), vec![]);
        assert_eq!(wm.focused_window(), None);
        assert_eq!(moved(&mut wm, 605., 100., win(2)), vec![]);
        assert_eq!(
            moved(&mut wm, 100., 100., win(1)),
            vec![Command::Focus(w(1))]
        );
    }

    #[test]
    fn foc_1_crossing_to_other_monitor_moves_focused_monitor() {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                2,
                102,
                WindowKind::Normal,
                r(1200., 100., 200., 200.),
            )],
        );
        assert_eq!(
            moved(&mut wm, 1500., 400., win(2)),
            vec![Command::Focus(w(2))]
        );
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    }

    fn act(wm: &mut Wm, a: Action) -> Vec<Command> {
        wm.handle(Event::Action(a))
    }

    #[test]
    fn hard_2_focus_on_another_apps_popup_clears_focus() {
        let mut wm = two();
        wm.handle(Event::WindowCreated(popup_of(9, 555)));
        wm.handle(Event::WindowFocused(w(9)));
        assert_eq!(wm.focused_window(), None);
        assert_eq!(act(&mut wm, Action::Close), vec![]);
    }

    #[test]
    fn hard_2_focus_on_own_apps_popup_keeps_focus() {
        let mut wm = two();
        wm.handle(Event::WindowCreated(popup_of(9, 102)));
        wm.handle(Event::WindowFocused(w(9)));
        assert_eq!(act(&mut wm, Action::Close), vec![Command::Close(w(2))]);
    }

    #[test]
    fn hard_2_focus_comes_back_after_another_apps_popup() {
        let mut wm = two();
        wm.handle(Event::WindowCreated(popup_of(9, 555)));
        wm.handle(Event::WindowFocused(w(9)));
        wm.handle(Event::WindowFocused(w(2)));
        assert_eq!(act(&mut wm, Action::Close), vec![Command::Close(w(2))]);
    }

    fn two_monitors_one_window_each() -> Wm {
        started(
            vec![main_monitor(), ext_monitor()],
            vec![
                window(1, 101, WindowKind::Normal, r(100., 100., 200., 200.)),
                window(2, 102, WindowKind::Normal, r(1200., 100., 200., 200.)),
            ],
        )
    }

    #[test]
    fn key_4_focus_left_picks_neighbor_and_warps() {
        let mut wm = two();
        assert_eq!(
            act(&mut wm, Action::Focus(Direction::Left)),
            vec![Command::Focus(w(1)), Command::WarpPointer(p(251.5, 400.))]
        );
    }

    #[test]
    fn foc_4_warp_echo_does_not_refocus() {
        let mut wm = two();
        act(&mut wm, Action::Focus(Direction::Left));
        assert_eq!(moved(&mut wm, 251.9, 400., win(2)), vec![]);
        assert_eq!(wm.focused_window(), Some(w(1)));
        assert_eq!(moved(&mut wm, 260., 400., win(1)), vec![]);
    }

    #[test]
    fn key_4_focus_crosses_monitors() {
        let mut wm = two_monitors_one_window_each();
        assert_eq!(
            act(&mut wm, Action::Focus(Direction::Right)),
            vec![Command::Focus(w(2)), Command::WarpPointer(p(1500., 400.))]
        );
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    }

    #[test]
    fn ws_7_focus_to_empty_monitor_keeps_workspaces() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![normal(1)]);
        act(&mut wm, Action::GoToWorkspace(ws(1)));
        assert_eq!(act(&mut wm, Action::Focus(Direction::Right)), vec![]);
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
        assert_eq!(wm.focused_window(), None);
        assert_eq!(nums(&wm), vec![1, 2]);
    }

    /// Window 1 on "main" is focused; "ext" shows empty workspace 2.
    fn empty_ext() -> Wm {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![normal(1)]);
        act(&mut wm, Action::GoToWorkspace(ws(1)));
        moved(&mut wm, 500., 400., win(1));
        wm
    }

    #[test]
    fn foc_10_pointer_onto_an_empty_monitor_focuses_it() {
        let mut wm = empty_ext();
        assert_eq!(moved(&mut wm, 1500., 400., PointerTarget::Other), vec![]);
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
        assert_eq!(wm.focused_window(), None);
        assert_eq!(nums(&wm), vec![1, 2]);
    }

    #[test]
    fn foc_10_from_the_desktop_of_one_monitor_to_an_empty_one() {
        let mut wm = empty_ext();
        moved(&mut wm, 2., 2., PointerTarget::Other);
        moved(&mut wm, 1500., 400., PointerTarget::Other);
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    }

    #[test]
    fn foc_10_a_new_window_opens_on_the_monitor_the_pointer_went_to() {
        let mut wm = empty_ext();
        moved(&mut wm, 1500., 400., PointerTarget::Other);
        wm.handle(Event::WindowCreated(normal(2)));
        assert_eq!(wm.focused_window(), Some(w(2)));
        assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    }

    #[test]
    fn foc_10_a_monitor_with_windows_waits_for_a_window_crossing() {
        let mut wm = two_monitors_one_window_each();
        act(&mut wm, Action::Focus(Direction::Left));
        moved(&mut wm, 500., 400., win(1));
        moved(&mut wm, 1002., 2., PointerTarget::Other);
        assert_eq!(wm.focused_window(), Some(w(1)));
        assert_eq!(wm.focused_monitor(), Some(&"main".into()));
    }

    #[test]
    fn foc_10_moving_within_the_monitor_doesnt_take_focus_back() {
        // Keyboard focus went to the other monitor while the pointer rested on the empty one.
        let mut wm = empty_ext();
        moved(&mut wm, 1500., 400., PointerTarget::Other);
        act(&mut wm, Action::Focus(Direction::Left));
        assert_eq!(wm.focused_window(), Some(w(1)));
        moved(&mut wm, 1510., 410., PointerTarget::Other);
        assert_eq!(wm.focused_window(), Some(w(1)));
        assert_eq!(wm.focused_monitor(), Some(&"main".into()));
    }

    #[test]
    fn foc_10_outside_every_monitor_changes_nothing() {
        let mut wm = empty_ext();
        moved(&mut wm, 1500., -10., PointerTarget::Other);
        assert_eq!(wm.focused_monitor(), Some(&"main".into()));
    }

    #[test]
    fn key_4_nothing_in_direction_is_noop() {
        let mut wm = two();
        assert_eq!(act(&mut wm, Action::Focus(Direction::Up)), vec![]);
    }

    #[test]
    fn foc_7_os_focus_on_hidden_workspace_switches_to_it() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        wm.handle(Event::WindowCreated(normal(2)));
        assert_eq!(
            wm.handle(Event::WindowFocused(w(1))),
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 790.)
                },
                Command::Hide(w(2)),
                Command::WarpPointer(p(500., 400.)),
            ]
        );
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(1)));
    }

    #[test]
    fn foc_7_os_focus_on_visible_window_warps_only() {
        let mut wm = two();
        assert_eq!(
            wm.handle(Event::WindowFocused(w(1))),
            vec![Command::WarpPointer(p(251.5, 400.))]
        );
        assert_eq!(wm.focused_window(), Some(w(1)));
    }

    #[test]
    fn os_focus_echo_and_popup_are_ignored() {
        let mut wm = two();
        wm.handle(Event::WindowCreated(popup_of(10, 102)));
        assert_eq!(wm.handle(Event::WindowFocused(w(2))), vec![]);
        assert_eq!(wm.handle(Event::WindowFocused(w(10))), vec![]);
        assert_eq!(wm.focused_window(), Some(w(2)));
    }
}
