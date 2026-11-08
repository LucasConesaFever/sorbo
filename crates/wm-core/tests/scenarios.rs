//! Acceptance scenarios from docs/requirements.md, driven through the public API only.

use wm_core::testing::*;
use wm_core::*;

fn act(wm: &mut Wm, a: Action) -> Vec<Command> {
    wm.handle(Event::Action(a))
}

fn moved(wm: &mut Wm, x: f64, y: f64, n: u64) -> Vec<Command> {
    wm.handle(Event::PointerMoved {
        position: p(x, y),
        target: PointerTarget::Window(w(n)),
    })
}

fn create(wm: &mut Wm, id: u64, pid: i32) {
    wm.handle(Event::WindowCreated(window(
        id,
        pid,
        WindowKind::Normal,
        r(0., 0., 100., 100.),
    )));
}

/// windows 1 (left) and 2 (right) on workspace 1, 2 focused
fn two_windows() -> Wm {
    let mut wm = started(vec![main_monitor()], vec![]);
    wm.handle(Event::WindowCreated(normal(1)));
    wm.handle(Event::WindowCreated(normal(2)));
    wm
}

/// main shows 1 (empty); ext has 2 (window 2, hidden) and 3 (window 3, visible), then ext is
/// disconnected.
fn ext_with_two_workspaces_disconnected() -> Wm {
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
    wm.handle(Event::MonitorsChanged(vec![main_monitor()]));
    wm
}

#[test]
fn scenario_01_second_monitor_shows_workspace_2() {
    let mut wm = started(vec![main_monitor()], vec![]);
    wm.handle(Event::WindowCreated(normal(1)));
    wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
    assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(2)));
}

#[test]
fn scenario_02_monitor_after_ten_gets_11_and_is_reachable() {
    let mut wm = started(vec![main_monitor()], vec![]);
    for n in 1..=10 {
        act(&mut wm, Action::GoToWorkspace(ws(n)));
        wm.handle(Event::WindowCreated(normal(u64::from(n))));
    }
    wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
    assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(11)));
    act(&mut wm, Action::Focus(Direction::Right));
    assert_eq!(wm.focused_monitor(), Some(&"ext".into()));
    act(&mut wm, Action::NextWorkspace);
    assert_eq!(wm.visible_workspace(&"ext".into()), Some(ws(11)));
    assert!(wm.workspace_numbers().contains(&ws(11)));
}

#[test]
fn scenario_03_disconnect_moves_workspaces_with_windows() {
    let wm = ext_with_two_workspaces_disconnected();
    assert_eq!(wm.workspace_location(ws(2)), Some(&"main".into()));
    assert_eq!(wm.workspace_location(ws(3)), Some(&"main".into()));
    assert_eq!(wm.window(w(2)).unwrap().workspace, ws(2));
    assert_eq!(wm.window(w(3)).unwrap().workspace, ws(3));
}

#[test]
fn scenario_04a_reconnect_returns_workspaces() {
    let mut wm = ext_with_two_workspaces_disconnected();
    wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
    assert_eq!(wm.workspace_location(ws(2)), Some(&"ext".into()));
    assert_eq!(wm.workspace_location(ws(3)), Some(&"ext".into()));
}

#[test]
fn scenario_04b_visible_workspace_stays_on_reconnect() {
    let mut wm = ext_with_two_workspaces_disconnected();
    act(&mut wm, Action::GoToWorkspace(ws(3)));
    wm.handle(Event::MonitorsChanged(vec![main_monitor(), ext_monitor()]));
    assert_eq!(wm.workspace_location(ws(3)), Some(&"main".into()));
    assert_eq!(wm.workspace_location(ws(2)), Some(&"ext".into()));
}

#[test]
fn scenario_05_switching_away_removes_empty_workspace() {
    let mut wm = started(vec![main_monitor()], vec![]);
    act(&mut wm, Action::GoToWorkspace(ws(2)));
    assert_eq!(nums(&wm), vec![2]);
}

#[test]
fn scenario_06_focus_to_other_monitor_keeps_empty_workspace() {
    let mut wm = started(vec![main_monitor(), ext_monitor()], vec![normal(1)]);
    act(&mut wm, Action::GoToWorkspace(ws(1)));
    act(&mut wm, Action::Focus(Direction::Right));
    assert_eq!(nums(&wm), vec![1, 2]);
}

fn popup_survives_moves_within_app(pid: i32) {
    let mut wm = started(vec![main_monitor()], vec![]);
    create(&mut wm, 1, pid);
    wm.handle(Event::WindowCreated(popup_of(10, pid)));
    assert_eq!(moved(&mut wm, 100., 100., 1), vec![]);
    assert_eq!(moved(&mut wm, 110., 110., 10), vec![]);
    assert_eq!(moved(&mut wm, 120., 120., 1), vec![]);
    assert_eq!(moved(&mut wm, 130., 120., 1), vec![]);
    assert_eq!(wm.focused_window(), Some(w(1)));
}

#[test]
fn scenario_07_popup_survives_moves_within_brave() {
    popup_survives_moves_within_app(101);
}

#[test]
fn scenario_08_same_for_chrome() {
    popup_survives_moves_within_app(201);
}

#[test]
fn scenario_09_moving_within_focused_window_keeps_focus() {
    let mut wm = two_windows();
    for i in 0..5 {
        assert_eq!(moved(&mut wm, 600. + f64::from(i), 100., 2), vec![]);
    }
    assert_eq!(wm.focused_window(), Some(w(2)));
}

#[test]
fn scenario_10_keyboard_focus_warps_once() {
    let mut wm = two_windows();
    assert_eq!(
        act(&mut wm, Action::Focus(Direction::Left)),
        vec![Command::Focus(w(1)), Command::WarpPointer(p(251.5, 400.))]
    );
    assert_eq!(moved(&mut wm, 251.5, 400., 1), vec![]);
}

#[test]
fn scenario_11_same_app_windows_stay_in_their_workspaces() {
    let mut wm = started(vec![main_monitor()], vec![]);
    create(&mut wm, 1, 300);
    act(&mut wm, Action::GoToWorkspace(ws(2)));
    create(&mut wm, 2, 300);
    wm.handle(Event::WindowFocused(w(1)));
    assert_eq!(wm.window(w(1)).unwrap().workspace, ws(1));
    assert_eq!(wm.window(w(2)).unwrap().workspace, ws(2));
    assert_eq!(wm.visible_workspace(&"main".into()), Some(ws(1)));
    assert_eq!(wm.placement(w(2)), Some(Placement::Hidden));
}

#[test]
fn scenario_12_new_terminal_opens_second_window_in_focused_workspace() {
    let mut wm = started(vec![main_monitor()], vec![]);
    create(&mut wm, 1, 400);
    assert_eq!(
        act(
            &mut wm,
            Action::Launch(App::Bundle("com.apple.Terminal".into()))
        ),
        vec![Command::LaunchNewWindow(App::Bundle(
            "com.apple.Terminal".into()
        ))]
    );
    create(&mut wm, 2, 400);
    assert_eq!(wm.window(w(1)).unwrap().workspace, ws(1));
    assert_eq!(wm.window(w(2)).unwrap().workspace, ws(1));
    assert_eq!(wm.focused_window(), Some(w(2)));
}
