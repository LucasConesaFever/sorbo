//! Key combos, actions and the default keybindings.

use crate::{App, WorkspaceNumber};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeStep {
    Grow,
    Shrink,
}

/// A key. Letters, digits and named keys are physical keys; punctuation is named by the
/// character it types, and translating it to the current keyboard layout is wm-macos's job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// `'a'..='z'`.
    Letter(char),
    /// `0..=9`.
    Digit(u8),
    Enter,
    Tab,
    Space,
    Escape,
    Backspace,
    /// Forward delete.
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 to F12.
    F(u8),
    /// A punctuation character: `+ - = , . / \ ; ' ` [ ]`.
    Punct(char),
}

/// Which modifier keys a combo needs. A combo always has at least one of cmd, alt or ctrl
/// (the config parser checks it), so a binding can't swallow normal typing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub cmd: bool,
    /// Option.
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

/// `mods` + `key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub key: Key,
    pub mods: Modifiers,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    GoToWorkspace(WorkspaceNumber),
    MoveToWorkspace(WorkspaceNumber),
    NextWorkspace,
    PrevWorkspace,
    Focus(Direction),
    Swap(Direction),
    Resize(ResizeStep),
    ToggleSplit,
    ToggleFloating,
    Close,
    Launch(App),
    /// Turns the resize mode on, or off when it's on (MODE-1).
    ResizeMode,
    /// Leaves the current mode (MODE-3). Only the mode's own keys send it.
    ExitMode,
    /// Moves the nearest divider around the focused tiled window toward a direction
    /// (MODE-2). Only the resize mode's keys send it.
    ResizeToward(Direction),
}

/// The default keybindings (KEY-7): those of the built-in default config file.
pub fn default_keymap() -> Vec<(KeyCombo, Action)> {
    crate::default_config().bindings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::App;
    use crate::testing::ws;

    fn ctrl_alt(key: Key, shift: bool) -> KeyCombo {
        KeyCombo {
            key,
            mods: Modifiers {
                ctrl: true,
                alt: true,
                shift,
                cmd: false,
            },
        }
    }

    #[test]
    fn key_7_keymap_has_39_unique_bindings() {
        let km = default_keymap();
        assert_eq!(km.len(), 39);
        let combos: std::collections::HashSet<_> = km.iter().map(|(c, _)| *c).collect();
        assert_eq!(combos.len(), 39);
    }

    #[test]
    fn key_1_digit_0_goes_to_workspace_10() {
        assert!(default_keymap().contains(&(
            ctrl_alt(Key::Digit(0), false),
            Action::GoToWorkspace(ws(10))
        )));
    }

    #[test]
    fn key_2_shift_digit_moves_window() {
        assert!(default_keymap().contains(&(
            ctrl_alt(Key::Digit(3), true),
            Action::MoveToWorkspace(ws(3))
        )));
    }

    #[test]
    fn key_3_tab_cycles_workspaces() {
        let km = default_keymap();
        assert!(km.contains(&(ctrl_alt(Key::Tab, false), Action::NextWorkspace)));
        assert!(km.contains(&(ctrl_alt(Key::Tab, true), Action::PrevWorkspace)));
    }

    #[test]
    fn key_6_launch_bindings() {
        let km = default_keymap();
        let terminal = App::Bundle("com.apple.Terminal".to_string());
        let finder = App::Bundle("com.apple.finder".to_string());
        assert!(km.contains(&(ctrl_alt(Key::Enter, false), Action::Launch(terminal))));
        assert!(km.contains(&(
            ctrl_alt(Key::Letter('b'), false),
            Action::Launch(App::Browser)
        )));
        assert!(km.contains(&(ctrl_alt(Key::Letter('e'), false), Action::Launch(finder))));
    }

    #[test]
    fn key_4_resize_uses_plus_and_minus() {
        let km = default_keymap();
        let grow = Action::Resize(ResizeStep::Grow);
        assert!(km.contains(&(ctrl_alt(Key::Punct('+'), false), grow)));
    }
}
