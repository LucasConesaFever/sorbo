//! The window manager state machine: `handle` takes an event and returns commands.
//!
//! Every event goes through the same two phases:
//! 1. a handler (in the submodules) changes state and may request focus or a pointer warp;
//! 2. `reconcile` works out where every window should be, emits only what changed,
//!    then the requested focus, then the requested warp.

mod bar;
mod focus;
mod gaps;
mod mode;
mod monitors;
mod rules;
mod windows;
mod workspaces;

use std::collections::{BTreeMap, HashMap};

use crate::{
    Action, Command, Event, Gaps, Mode, Monitor, MonitorId, Placement, Point, PointerTarget, Rect,
    UsableRect, Window, WindowId, WindowInfo, WindowKind, Workspace, WorkspaceNumber,
    WorkspaceStatus, tree,
};

use windows::Refusal;

#[derive(Debug, Default)]
pub struct Wm {
    /// Connected monitors only.
    monitors: Vec<Monitor>,
    /// Sorted by number, which makes "lowest free" and "next on this monitor" simple walks.
    workspaces: BTreeMap<WorkspaceNumber, Workspace>,
    windows: HashMap<WindowId, Window>,
    /// Never a popup.
    focused_window: Option<WindowId>,
    /// `None` only before the first `MonitorsChanged`.
    focused_monitor: Option<MonitorId>,
    /// What the pointer was over at the last `PointerMoved`.
    pointer_over: Option<PointerTarget>,
    /// The monitor the pointer was last inside (FOC-10).
    pointer_monitor: Option<MonitorId>,
    /// Where we last warped the pointer; the echo of that warp is ignored (FOC-4).
    pending_warp: Option<Point>,
    /// The last placement sent (or reported) for each managed window.
    placements: HashMap<WindowId, Placement>,
    /// Tiled windows answering their tile with another frame (LAY-4).
    refusals: HashMap<WindowId, Refusal>,
    /// Emit `Focus` for this window in reconcile.
    focus_request: Option<WindowId>,
    /// Warp the pointer to this window's center in reconcile.
    warp_request: Option<WindowId>,
    /// `Close` / `LaunchNewWindow` commands queued by handlers.
    out: Vec<Command>,
    /// The menu bar row last returned by `take_row_change`; `None` before the first one.
    last_row: Option<Vec<WorkspaceStatus>>,
    /// Space around tiled windows.
    gaps: Gaps,
    /// Which keys are live (MODE-1).
    mode: Mode,
}

impl Wm {
    pub fn new() -> Wm {
        Wm::default()
    }

    pub fn handle(&mut self, event: Event) -> Vec<Command> {
        // Gaps can arrive first: the config is loaded before monitors are known.
        if self.monitors.is_empty()
            && !matches!(event, Event::MonitorsChanged(_) | Event::GapsChanged(_))
        {
            return Vec::new();
        }
        match event {
            Event::MonitorsChanged(infos) => self.on_monitors_changed(infos),
            Event::WindowsDiscovered(infos) => self.on_windows_discovered(infos),
            Event::WindowCreated(info) => self.on_window_created(info),
            Event::WindowDestroyed(id) => self.on_window_destroyed(id),
            Event::WindowFrameChanged { id, frame } => self.on_window_frame_changed(id, frame),
            Event::PointerMoved { position, target } => self.on_pointer_moved(position, target),
            Event::FocusLeft => self.on_focus_left(),
            Event::WindowFocused(id) => self.on_window_focused(id),
            Event::Action(action) => self.on_action(action),
            Event::GapsChanged(gaps) => self.on_gaps_changed(gaps),
        }
        if self.monitors.is_empty() {
            self.out.clear();
            return Vec::new();
        }
        let commands = self.reconcile();
        #[cfg(debug_assertions)]
        self.debug_assert_invariants();
        commands
    }

    fn on_action(&mut self, action: Action) {
        // MODE-3: any binding that isn't the mode's own ends the mode, then runs.
        if !matches!(
            action,
            Action::ResizeMode | Action::ExitMode | Action::ResizeToward(_)
        ) {
            self.set_mode(Mode::Normal);
        }
        match action {
            Action::GoToWorkspace(n) => self.go_to_workspace(n),
            Action::MoveToWorkspace(n) => self.move_to_workspace(n),
            Action::NextWorkspace => self.cycle_workspace(true),
            Action::PrevWorkspace => self.cycle_workspace(false),
            Action::Focus(direction) => self.focus_direction(direction),
            Action::Swap(direction) => self.swap_direction(direction),
            Action::Resize(step) => self.resize_focused(step),
            Action::ToggleSplit => self.toggle_split_focused(),
            Action::ToggleFloating => self.toggle_floating(),
            Action::Close => self.close_focused(),
            Action::Launch(app) => self.launch(app),
            Action::ResizeMode => self.toggle_resize_mode(),
            Action::ExitMode => self.set_mode(Mode::Normal),
            Action::ResizeToward(direction) => self.resize_toward_focused(direction),
        }
    }

    // ----- read-only accessors -----

    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    pub fn workspace(&self, n: WorkspaceNumber) -> Option<&Workspace> {
        self.workspaces.get(&n)
    }

    pub fn workspace_numbers(&self) -> Vec<WorkspaceNumber> {
        self.workspaces.keys().copied().collect()
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    pub fn focused_window(&self) -> Option<WindowId> {
        self.focused_window
    }

    pub fn focused_monitor(&self) -> Option<&MonitorId> {
        self.focused_monitor.as_ref()
    }

    pub fn visible_workspace(&self, monitor: &MonitorId) -> Option<WorkspaceNumber> {
        self.monitor(monitor).map(|m| m.visible)
    }

    /// The monitor a workspace currently lives on: its home if connected, else the main monitor.
    pub fn workspace_location(&self, n: WorkspaceNumber) -> Option<&MonitorId> {
        let ws = self.workspaces.get(&n)?;
        if self.monitor(&ws.home).is_some() {
            Some(&ws.home)
        } else {
            self.main_monitor().map(|m| &m.id)
        }
    }

    pub fn placement(&self, id: WindowId) -> Option<Placement> {
        self.placements.get(&id).copied()
    }

    // ----- internal helpers shared by the submodules -----

    fn monitor(&self, id: &MonitorId) -> Option<&Monitor> {
        self.monitors.iter().find(|m| &m.id == id)
    }

    fn monitor_mut(&mut self, id: &MonitorId) -> Option<&mut Monitor> {
        self.monitors.iter_mut().find(|m| &m.id == id)
    }

    fn main_monitor(&self) -> Option<&Monitor> {
        self.monitors
            .iter()
            .find(|m| m.is_main)
            .or(self.monitors.first())
    }

    /// The frame of the monitor workspace `n` lives on.
    fn workspace_area(&self, n: WorkspaceNumber) -> Option<Rect> {
        let location = self.workspace_location(n)?;
        self.monitor(location).map(|m| m.frame)
    }

    fn is_visible(&self, n: WorkspaceNumber) -> bool {
        self.monitors.iter().any(|m| m.visible == n)
    }

    /// WS-2 / WS-3: the smallest number not in use, with no upper bound.
    fn lowest_free_number(&self) -> WorkspaceNumber {
        let mut n = WorkspaceNumber::ONE;
        while self.workspaces.contains_key(&n) {
            // More workspaces than `u32::MAX` would need more windows than exist.
            n = n.next().expect("a free workspace number below u32::MAX");
        }
        n
    }

    fn create_workspace(&mut self, n: WorkspaceNumber, home: MonitorId) {
        self.workspaces.insert(n, Workspace::new(n, home));
    }

    /// Records a new window and puts it in workspace `n`. Popups are stored only.
    /// `target` is the tiled window to split; `None` falls back to the tree's first window.
    fn add_window(&mut self, info: WindowInfo, n: WorkspaceNumber, target: Option<WindowId>) {
        // A rule overrides what the window's kind would pick (RULE-2); popups are never
        // managed, so they never float (RULE-4).
        let frame = info.frame.map(UsableRect::rect);
        let floating = info.kind != WindowKind::Popup
            && info
                .rule
                .floating
                .unwrap_or(info.kind == WindowKind::Floating);
        self.windows.insert(
            info.id,
            Window {
                id: info.id,
                pid: info.pid,
                kind: info.kind,
                workspace: n,
                floating,
                // No usable frame (SAFE-1): the window isn't moved and doesn't own the
                // pointer until a usable one arrives.
                floating_frame: frame.filter(|_| floating),
            },
        );
        if info.kind == WindowKind::Popup {
            return;
        }
        if let Some(frame) = frame {
            self.placements.insert(info.id, Placement::At(frame));
        }
        // The area only picks the split direction for the new tile.
        let area = self
            .workspace_area(n)
            .or(frame)
            .or_else(|| {
                let m = self.monitor(self.focused_monitor.as_ref()?)?;
                Some(m.frame)
            })
            .unwrap_or(Rect::new(0., 0., 1., 1.));
        if let Some(ws) = self.workspaces.get_mut(&n) {
            if floating {
                ws.floating.push(info.id);
            } else {
                tree::insert(&mut ws.tree, target, info.id, area);
            }
        }
    }

    // ----- reconcile -----

    /// Invariant 3: every workspace is visible or non-empty (WS-5, WS-6, WS-7).
    fn prune_workspaces(&mut self) {
        let visible: Vec<WorkspaceNumber> = self.monitors.iter().map(|m| m.visible).collect();
        self.workspaces
            .retain(|n, ws| visible.contains(n) || !ws.is_empty());
    }

    /// Floating frames are absolute, so they can point at a monitor the workspace no longer
    /// lives on (disconnect, move, resolution change). Bring every visible floating window
    /// onto its workspace's monitor: carried over from the monitor it was on, or just moved
    /// inside if it was on none.
    fn fit_floating_windows(&mut self) {
        let mut fixes = Vec::new();
        for monitor in &self.monitors {
            let Some(ws) = self.workspaces.get(&monitor.visible) else {
                continue;
            };
            for id in &ws.floating {
                let Some(reported) = self.windows.get(id).and_then(|w| w.floating_frame) else {
                    continue;
                };
                // Never larger than its monitor (SAFE-2); a capped frame is also kept inside.
                let frame = reported.capped_to(&monitor.frame);
                let capped = frame != reported;
                let center = frame.center();
                if monitor.frame.contains(center) {
                    if capped {
                        fixes.push((*id, frame.clamp_inside(&monitor.frame)));
                    }
                    continue;
                }
                let fitted = match self.monitors.iter().find(|m| m.frame.contains(center)) {
                    Some(from) => frame.translate(
                        monitor.frame.x - from.frame.x,
                        monitor.frame.y - from.frame.y,
                    ),
                    None => frame,
                }
                .clamp_inside(&monitor.frame);
                fixes.push((*id, fitted));
            }
        }
        for (id, frame) in fixes {
            if let Some(window) = self.windows.get_mut(&id) {
                window.floating_frame = Some(frame);
            }
        }
    }

    fn reconcile(&mut self) -> Vec<Command> {
        self.prune_workspaces();
        self.fit_floating_windows();

        // Where every window on a visible workspace should be.
        let mut desired: HashMap<WindowId, Placement> = HashMap::new();
        let mut unknown: Vec<WindowId> = Vec::new();
        for monitor in &self.monitors {
            let Some(ws) = self.workspaces.get(&monitor.visible) else {
                continue;
            };
            for (id, rect) in tree::layout(&ws.tree, monitor.frame, self.gaps) {
                desired.insert(id, Placement::At(rect));
            }
            for id in &ws.floating {
                if let Some(frame) = self.windows.get(id).and_then(|w| w.floating_frame) {
                    desired.insert(*id, Placement::At(frame));
                } else if self.placements.get(id) == Some(&Placement::Hidden) {
                    // Hidden while its frame was unknown: core picks where it comes back,
                    // or it would stay parked off screen (SAFE-1, SAFE-3).
                    desired.insert(
                        *id,
                        Placement::At(windows::first_float_frame(monitor.frame)),
                    );
                } else {
                    // No known frame yet (SAFE-1, SAFE-3): leave it where it is.
                    unknown.push(*id);
                }
            }
        }

        // Everything else that is managed is hidden. Emit only what changed, sorted by id.
        let mut managed: Vec<WindowId> = self
            .windows
            .values()
            .filter(|w| w.kind != WindowKind::Popup && !unknown.contains(&w.id))
            .map(|w| w.id)
            .collect();
        managed.sort();

        // A frame core picked becomes the window's floating frame.
        for (id, placement) in &desired {
            if let Placement::At(frame) = placement
                && let Some(window) = self.windows.get_mut(id)
                && window.floating
                && window.floating_frame.is_none()
            {
                window.floating_frame = Some(*frame);
            }
        }

        let mut commands = std::mem::take(&mut self.out);
        for id in managed {
            let placement = desired.get(&id).copied().unwrap_or(Placement::Hidden);
            let unchanged = match (self.placements.get(&id), placement) {
                (Some(Placement::At(old)), Placement::At(new)) => old.near(&new),
                (Some(Placement::Hidden), Placement::Hidden) => true,
                _ => false,
            };
            if !unchanged {
                commands.push(match placement {
                    Placement::At(frame) => Command::SetFrame { id, frame },
                    Placement::Hidden => Command::Hide(id),
                });
                self.placements.insert(id, placement);
            }
        }

        if let Some(id) = self.focus_request.take() {
            commands.push(Command::Focus(id));
        }
        if let Some(id) = self.warp_request.take()
            && let Some(Placement::At(rect)) = self.placements.get(&id)
        {
            let center = rect.center();
            self.pending_warp = Some(center);
            commands.push(Command::WarpPointer(center));
        }
        commands
    }

    // ----- invariants (spec 4.8) -----

    #[cfg(debug_assertions)]
    fn debug_assert_invariants(&self) {
        let mut seen_visible = Vec::new();
        for m in &self.monitors {
            // 1. the visible workspace exists and lives on this monitor
            assert!(
                self.workspaces.contains_key(&m.visible),
                "monitor {:?} shows missing workspace {}",
                m.id,
                m.visible
            );
            assert_eq!(
                self.workspace_location(m.visible),
                Some(&m.id),
                "workspace {} is visible on a monitor it doesn't live on",
                m.visible
            );
            // 2. no workspace is visible twice
            assert!(
                !seen_visible.contains(&m.visible),
                "workspace {} visible on two monitors",
                m.visible
            );
            seen_visible.push(m.visible);
        }
        for (n, ws) in &self.workspaces {
            // 3. visible or non-empty
            assert!(
                seen_visible.contains(n) || !ws.is_empty(),
                "workspace {n} is empty and hidden"
            );
        }
        for w in self.windows.values() {
            if w.kind == WindowKind::Popup {
                continue;
            }
            // 4. exactly one place, in its own workspace
            let places = self
                .workspaces
                .values()
                .filter(|ws| ws.contains(w.id))
                .count();
            assert_eq!(places, 1, "window {:?} is in {places} workspaces", w.id);
            assert!(
                self.workspaces
                    .get(&w.workspace)
                    .is_some_and(|ws| ws.contains(w.id)),
                "window {:?} is not in its own workspace {}",
                w.id,
                w.workspace
            );
            let in_floating_list = self.workspaces[&w.workspace].floating.contains(&w.id);
            assert_eq!(
                in_floating_list, w.floating,
                "window {:?} floating flag mismatch",
                w.id
            );
        }
        // 5. the focused window is on the workspace visible on the focused monitor
        if let Some(id) = self.focused_window {
            let w = &self.windows[&id];
            assert_ne!(w.kind, WindowKind::Popup, "a popup is focused");
            let focused_ws = self
                .focused_monitor
                .as_ref()
                .and_then(|m| self.visible_workspace(m));
            assert_eq!(
                Some(w.workspace),
                focused_ws,
                "focused window {id:?} is not on the focused monitor's workspace"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::*;
    use crate::*;

    #[test]
    fn events_before_monitors_are_ignored() {
        let mut wm = Wm::new();
        assert_eq!(wm.handle(Event::WindowCreated(normal(1))), vec![]);
        assert!(wm.window(w(1)).is_none());
    }

    #[test]
    fn startup_numbers_main_monitor_first() {
        let wm = started(vec![ext_monitor(), main_monitor()], vec![]);
        assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(1)));
        assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(2)));
        assert_eq!(wm.focused_monitor(), Some(&"main".into()));
    }

    #[test]
    fn startup_places_discovered_windows_by_center() {
        let mut wm = Wm::new();
        wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
        let cmds = wm.handle(Event::WindowsDiscovered(vec![
            window(1, 101, WindowKind::Normal, r(100., 100., 200., 200.)),
            window(2, 102, WindowKind::Normal, r(1200., 100., 200., 200.)),
        ]));
        assert_eq!(
            cmds,
            vec![
                Command::SetFrame {
                    id: w(1),
                    frame: r(5., 5., 990., 790.)
                },
                Command::SetFrame {
                    id: w(2),
                    frame: r(1005., 5., 990., 790.)
                },
            ]
        );
        assert_eq!(wm.window(w(2)).unwrap().workspace, ws(2));
    }

    #[test]
    fn startup_window_off_every_monitor_goes_to_main() {
        let wm = started(
            vec![main_monitor(), ext_monitor()],
            vec![window(
                1,
                101,
                WindowKind::Normal,
                r(5000., 5000., 100., 100.),
            )],
        );
        assert_eq!(wm.window(w(1)).unwrap().workspace, ws(1));
    }

    #[test]
    fn startup_window_already_in_place_emits_nothing() {
        let mut wm = Wm::new();
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        let cmds = wm.handle(Event::WindowsDiscovered(vec![window(
            1,
            101,
            WindowKind::Normal,
            r(5., 5., 990., 790.),
        )]));
        assert_eq!(cmds, vec![]);
    }

    #[test]
    fn startup_popups_are_stored_but_never_commanded() {
        let mut wm = Wm::new();
        wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
        assert_eq!(
            wm.handle(Event::WindowsDiscovered(vec![popup_of(9, 101)])),
            vec![]
        );
        assert!(wm.window(w(9)).is_some());
        assert!(wm.workspace(ws(1)).unwrap().is_empty());
    }
}
