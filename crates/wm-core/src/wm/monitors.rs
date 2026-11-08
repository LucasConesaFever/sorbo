//! Monitor snapshots (startup and changes) and windows discovered at startup.

use super::Wm;
use crate::{Monitor, MonitorId, MonitorInfo, WindowInfo, WorkspaceNumber};

impl Wm {
    pub(super) fn on_monitors_changed(&mut self, infos: Vec<MonitorInfo>) {
        // An empty snapshot (lid closed, sleep) is ignored; state stays as it was.
        if infos.is_empty() {
            return;
        }
        if self.monitors.is_empty() {
            self.start_monitors(infos);
        } else {
            self.update_monitors(infos);
        }
    }

    /// Every snapshot after the first (spec 6.3).
    fn update_monitors(&mut self, infos: Vec<MonitorInfo>) {
        // Frames and main flag of monitors that are still connected.
        for info in &infos {
            if let Some(monitor) = self.monitor_mut(&info.id) {
                monitor.frame = info.frame;
                monitor.is_main = info.is_main;
            }
        }

        // MON-3: disconnected monitors go away. Their workspaces keep `home`, so their location
        // now resolves to the main monitor; reconcile hides their windows.
        let focused_gone = self
            .focused_monitor
            .as_ref()
            .is_some_and(|f| !infos.iter().any(|i| &i.id == f));
        self.monitors.retain(|m| infos.iter().any(|i| i.id == m.id));
        // Workspaces left empty and hidden by the disconnect free their numbers before
        // new monitors pick one (WS-2).
        self.prune_workspaces();

        // MON-1, MON-4, MON-5: newly connected monitors.
        for info in infos {
            if self.monitor(&info.id).is_none() {
                self.connect_monitor(info);
            }
        }

        // After connecting, so a main monitor exists even when every monitor was replaced.
        if focused_gone {
            self.focus_main_monitor();
        }

        // A visible workspace must live on the monitor showing it (invariant 1). This can break
        // when the main flag moves while a workspace from a disconnected monitor is visible.
        let shown: Vec<(MonitorId, WorkspaceNumber)> = self
            .monitors
            .iter()
            .map(|m| (m.id.clone(), m.visible))
            .collect();
        for (monitor, n) in shown {
            if self.workspace_location(n) != Some(&monitor)
                && let Some(ws) = self.workspaces.get_mut(&n)
            {
                ws.home = monitor;
            }
        }
    }

    /// Focus moves to the main monitor and its workspace's last focused window, without
    /// sending a `Focus` command.
    fn focus_main_monitor(&mut self) {
        let Some(main) = self.main_monitor() else {
            self.focused_monitor = None;
            self.set_focus(None, false);
            return;
        };
        let (id, n) = (main.id.clone(), main.visible);
        self.focused_monitor = Some(id);
        let target = self
            .workspaces
            .get(&n)
            .and_then(|ws| ws.last_focused.filter(|w| ws.contains(*w)));
        self.set_focus(target, false);
    }

    /// Workspaces whose home is this monitor come back, except any that is visible elsewhere
    /// right now: that one stays and adopts the monitor showing it as its new home (MON-5).
    /// The monitor shows the lowest returning workspace, or a new one (MON-1, WS-2, WS-3).
    fn connect_monitor(&mut self, info: MonitorInfo) {
        let homed_here: Vec<WorkspaceNumber> = self
            .workspaces
            .values()
            .filter(|ws| ws.home == info.id)
            .map(|ws| ws.number)
            .collect();
        let mut returning = Vec::new();
        for n in homed_here {
            let shown_on = self
                .monitors
                .iter()
                .find(|m| m.visible == n)
                .map(|m| m.id.clone());
            match shown_on {
                Some(other) => {
                    if let Some(ws) = self.workspaces.get_mut(&n) {
                        ws.home = other;
                    }
                }
                None => returning.push(n),
            }
        }
        let visible = match returning.first() {
            Some(n) => *n,
            None => {
                let n = self.lowest_free_number();
                self.create_workspace(n, info.id.clone());
                n
            }
        };
        self.monitors.push(Monitor {
            id: info.id,
            frame: info.frame,
            is_main: info.is_main,
            visible,
        });
    }

    /// First snapshot (spec 6.1): the main monitor gets workspace 1, the others follow in order.
    fn start_monitors(&mut self, mut infos: Vec<MonitorInfo>) {
        // Stable sort: `false` (main) sorts before `true`, everything else keeps its order.
        infos.sort_by_key(|m| !m.is_main);
        for info in infos {
            let n = self.lowest_free_number();
            self.create_workspace(n, info.id.clone());
            self.monitors.push(Monitor {
                id: info.id,
                frame: info.frame,
                is_main: info.is_main,
                visible: n,
            });
        }
        self.focused_monitor = self.main_monitor().map(|m| m.id.clone());
    }

    /// Each window goes to the visible workspace of the monitor under its center,
    /// or the main monitor if its center is on none.
    pub(super) fn on_windows_discovered(&mut self, infos: Vec<WindowInfo>) {
        for info in infos {
            if self.windows.contains_key(&info.id) {
                continue;
            }
            // A window whose frame isn't confirmed yet goes by where the app says it is; with
            // neither, or a point on no monitor (including NaN), to the main monitor.
            let center = info.frame.map(|f| f.rect().center()).or(info.position_hint);
            let monitor = self
                .monitors
                .iter()
                .find(|m| center.is_some_and(|c| m.frame.contains(c)))
                .or(self.main_monitor());
            let Some(n) = monitor.map(|m| m.visible) else {
                continue;
            };
            // Split the most recently added window, as if they had been opened one by one.
            let target = self
                .workspaces
                .get(&n)
                .and_then(|ws| ws.tree.as_ref())
                .and_then(|t| t.windows().last().copied());
            self.add_window(info, n, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    fn act(wm: &mut Wm, a: Action) -> Vec<Command> {
        wm.handle(Event::Action(a))
    }

    /// main shows 1 (empty); ext has 2 (window 2, hidden) and 3 (window 3, visible).
    fn ext_with_two_workspaces() -> Wm {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                2,
                102,
                WindowKind::Normal,
                r(1200., 100., 200., 200.),
            )],
        );
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        act(&mut wm, Action::GoToWorkspace(ws(3)));
        wm.handle(Event::WindowCreated(normal(3)));
        wm
    }

    #[test]
    fn hard_7_unconfirmed_window_keeps_its_monitor_at_startup() {
        // Not yet confirmed by the window server (no usable frame), but reported at a point on
        // "ext": it starts on ext's workspace, not the main one.
        let info = window(1, 101, WindowKind::Normal, r(1200., 100., 0., 0.));
        assert_eq!(info.frame, None);
        let wm = started(vec![main_monitor(), ext_monitor()], vec![info]);
        assert_eq!(wm.window(w(1)).unwrap().workspace, ws(2));
    }

    #[test]
    fn mon_1_new_monitor_gets_lowest_free_workspace() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
        assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(2)));
        assert_eq!(wm.focused_monitor(), Some(&"main".into()));
    }

    #[test]
    fn ws_2_new_monitor_fills_lowest_gap() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        act(&mut wm, Action::GoToWorkspace(ws(3)));
        wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
        assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(2)));
    }

    #[test]
    fn ws_3_monitor_after_ten_workspaces_gets_11() {
        let mut wm = started(vec![main_monitor()], vec![]);
        for n in 1..=10 {
            act(&mut wm, Action::GoToWorkspace(ws(n)));
            wm.handle(Event::WindowCreated(normal(u64::from(n))));
        }
        wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
        assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(11)));
    }

    #[test]
    fn mon_3_disconnect_moves_workspaces_to_main() {
        let mut wm = ext_with_two_workspaces();
        assert_eq!(
            wm.handle(Event::MonitorsChanged(vec![main_monitor()])),
            vec![Command::Hide(w(3))]
        );
        assert_eq!(wm.workspace_location(ws(2)), Some(&"main".into()));
        assert_eq!(wm.workspace_location(ws(3)), Some(&"main".into()));
        assert_eq!(wm.workspace(ws(3)).unwrap().home, "ext".into());
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(1)));
        assert_eq!(wm.focused_monitor(), Some(&"main".into()));
    }

    #[test]
    fn mon_4_reconnect_returns_workspaces() {
        let mut wm = ext_with_two_workspaces();
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        assert_eq!(
            wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()])),
            vec![Command::SetFrame {
                id: w(2),
                frame: r(1005., 5., 990., 790.)
            }]
        );
        assert_eq!(wm.workspace_location(ws(3)), Some(&"ext".into()));
        assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(2)));
    }

    #[test]
    fn mon_5_visible_workspace_stays_on_reconnect() {
        let mut wm = ext_with_two_workspaces();
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        act(&mut wm, Action::GoToWorkspace(ws(3)));
        wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
        assert_eq!(wm.workspace(ws(3)).unwrap().home, "main".into());
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(3)));
        assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(2)));
    }

    #[test]
    fn ws_5_disconnect_deletes_empty_visible_workspace() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        assert_eq!(nums(&wm), vec![1]);
    }

    #[test]
    fn empty_snapshot_is_ignored() {
        let mut wm = ext_with_two_workspaces();
        assert_eq!(wm.handle(Event::MonitorsChanged(vec![])), vec![]);
        assert_eq!(wm.monitors().len(), 2);
    }

    #[test]
    fn monitor_frame_change_relayouts() {
        let mut wm = started(vec![main_monitor()], vec![normal(1)]);
        assert_eq!(
            wm.handle(Event::MonitorsChanged(vec![monitor(
                "main",
                r(0., 25., 1000., 775.),
                true
            )])),
            vec![Command::SetFrame {
                id: w(1),
                frame: r(5., 30., 990., 765.)
            }]
        );
    }

    #[test]
    fn main_flag_change_rehomes_visible_orphan_workspace() {
        let mut wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                2,
                102,
                WindowKind::Normal,
                r(1200., 100., 200., 200.),
            )],
        );
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        wm.handle(Event::MonitorsChanged(vec![
            monitor("main", r(0., 0., 1000., 800.), false),
            monitor("side", r(-1000., 0., 1000., 800.), true),
        ]));
        assert_eq!(wm.workspace(ws(2)).unwrap().home, "main".into());
        assert_eq!(wm.workspace_location(ws(2)), Some(&"main".into()));
    }

    #[test]
    fn mon_3_complete_monitor_replacement_keeps_focus_valid() {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::MonitorsChanged(vec![monitor(
            "other",
            r(0., 0., 1000., 800.),
            true,
        )]));
        assert_eq!(wm.focused_monitor(), Some(&"other".into()));
        assert_eq!(wm.focused_window(), None);
        wm.handle(Event::WindowCreated(normal(2)));
        assert!(wm.window(w(2)).is_some());
    }

    #[test]
    fn ws_2_replaced_monitor_reuses_freed_number() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        wm.handle(Event::MonitorsChanged(vec![
            main_monitor(),
            monitor("ext2", r(1000., 0., 1000., 800.), false),
        ]));
        assert_eq!(wm.visible_workspace(&"ext2".into()), Some(ws(2)));
        assert_eq!(nums(&wm), vec![1, 2]);
    }

    #[test]
    fn mon_3_floating_window_from_disconnected_monitor_stays_on_screen() {
        let mut wm = started(vec![main_monitor(), ext_monitor()], vec![]);
        act(&mut wm, Action::GoToWorkspace(ws(2)));
        wm.handle(Event::WindowCreated(window(
            3,
            103,
            WindowKind::Floating,
            r(1200., 100., 300., 200.),
        )));
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        let cmds = act(&mut wm, Action::GoToWorkspace(ws(2)));
        assert_eq!(
            cmds[0],
            Command::SetFrame {
                id: w(3),
                frame: r(700., 100., 300., 200.)
            }
        );
    }
}
