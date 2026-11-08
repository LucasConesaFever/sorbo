//! The resize mode (MODE-1 to MODE-3): which keys wm-macos registers is its business; core
//! decides when the mode is on and what the mode's actions do.

use super::Wm;
use crate::{Command, Direction, Mode, tree};

impl Wm {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switches to `mode`, telling wm-macos only when it actually changes.
    pub(super) fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.out.push(Command::ModeChanged(mode));
        }
    }

    /// MODE-1: the `resize-mode` binding turns the mode on, or off when it's on.
    pub(super) fn toggle_resize_mode(&mut self) {
        let next = match self.mode {
            Mode::Normal => Mode::Resize,
            Mode::Resize => Mode::Normal,
        };
        self.set_mode(next);
    }

    /// MODE-2: nothing happens without a focused tiled window or a divider on that axis;
    /// either way the mode stays on. Reconcile re-lays out and emits the frames that changed.
    pub(super) fn resize_toward_focused(&mut self, direction: Direction) {
        if let Some((id, n)) = self.focused_tiled()
            && let Some(ws) = self.workspaces.get_mut(&n)
        {
            tree::resize_toward(&mut ws.tree, id, direction);
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

    /// Windows 1 (left) and 2 (right), 2 focused.
    fn two() -> Wm {
        let mut wm = started(vec![main_monitor()], vec![]);
        wm.handle(Event::WindowCreated(normal(1)));
        wm.handle(Event::WindowCreated(normal(2)));
        wm
    }

    #[test]
    fn mode_1_resize_mode_toggles() {
        let mut wm = two();
        assert!(act(&mut wm, Action::ResizeMode).contains(&Command::ModeChanged(Mode::Resize)));
        assert_eq!(wm.mode(), Mode::Resize);
        assert!(act(&mut wm, Action::ResizeMode).contains(&Command::ModeChanged(Mode::Normal)));
        assert_eq!(wm.mode(), Mode::Normal);
    }

    #[test]
    fn mode_2_arrow_resizes_in_the_mode() {
        let mut wm = two();
        act(&mut wm, Action::ResizeMode);
        let cmds = act(&mut wm, Action::ResizeToward(Direction::Left));
        let frames = cmds
            .iter()
            .filter(|c| matches!(c, Command::SetFrame { .. }))
            .count();
        assert_eq!(frames, 2);
        assert_eq!(wm.mode(), Mode::Resize);
    }

    #[test]
    fn mode_2_arrow_without_tiled_focus_keeps_the_mode() {
        let mut wm = two();
        let mut float = window(3, 103, WindowKind::Normal, r(100., 100., 200., 200.));
        float.rule = RuleEffect {
            floating: Some(true),
        };
        wm.handle(Event::WindowCreated(float));
        assert_eq!(wm.focused_window(), Some(w(3)));
        act(&mut wm, Action::ResizeMode);
        let cmds = act(&mut wm, Action::ResizeToward(Direction::Right));
        assert!(!cmds.iter().any(|c| matches!(c, Command::SetFrame { .. })));
        assert_eq!(wm.mode(), Mode::Resize);
    }

    #[test]
    fn mode_3_exit_mode_leaves() {
        let mut wm = two();
        act(&mut wm, Action::ResizeMode);
        assert_eq!(
            act(&mut wm, Action::ExitMode),
            vec![Command::ModeChanged(Mode::Normal)]
        );
        assert_eq!(act(&mut wm, Action::ExitMode), vec![]);
    }

    #[test]
    fn mode_3_other_action_leaves_and_runs() {
        let mut wm = two();
        act(&mut wm, Action::ResizeMode);
        let cmds = act(&mut wm, Action::GoToWorkspace(ws(2)));
        assert!(cmds.contains(&Command::ModeChanged(Mode::Normal)));
        assert!(cmds.contains(&Command::Hide(w(1))));
        assert_eq!(wm.mode(), Mode::Normal);
    }
}
