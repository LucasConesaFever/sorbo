//! Turning core's `Key` (a character) into what Carbon hotkeys need (spec 5.5).

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use wm_core::{Action, Direction, Key, KeyCombo, Modifiers};

/// How to find the virtual key code for a `Key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    /// The same physical key on every layout (ANSI position).
    Fixed(u16),
    /// Look up which key types this character on the current layout.
    OnLayout(char),
}

/// The virtual key code (macOS `kVK_*`) for a key.
pub fn key_code(key: Key) -> KeyCode {
    // Digit n sits at index n; digit 0 at index 0.
    const DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    // 'a' at index 0 ... 'z' at index 25 (ANSI positions).
    const LETTERS: [u16; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F,
        0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    // F1 at index 0 ... F12 at index 11.
    const FUNCTION: [u16; 12] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F,
    ];
    match key {
        Key::Letter(c) => {
            let index = (c.to_ascii_lowercase() as usize).wrapping_sub('a' as usize);
            KeyCode::Fixed(LETTERS[index.min(25)])
        }
        Key::Digit(d) => KeyCode::Fixed(DIGITS[usize::from(d % 10)]),
        Key::F(n) => KeyCode::Fixed(FUNCTION[usize::from(n.clamp(1, 12) - 1)]),
        Key::Enter => KeyCode::Fixed(0x24),
        Key::Tab => KeyCode::Fixed(0x30),
        Key::Space => KeyCode::Fixed(0x31),
        Key::Backspace => KeyCode::Fixed(0x33),
        Key::Escape => KeyCode::Fixed(0x35),
        Key::Delete => KeyCode::Fixed(0x75),
        Key::Home => KeyCode::Fixed(0x73),
        Key::End => KeyCode::Fixed(0x77),
        Key::PageUp => KeyCode::Fixed(0x74),
        Key::PageDown => KeyCode::Fixed(0x79),
        Key::Left => KeyCode::Fixed(0x7B),
        Key::Right => KeyCode::Fixed(0x7C),
        Key::Down => KeyCode::Fixed(0x7D),
        Key::Up => KeyCode::Fixed(0x7E),
        Key::Punct(c) => KeyCode::OnLayout(c),
    }
}

/// Carbon modifier flags for a combo's modifiers.
pub fn carbon_modifiers(mods: Modifiers) -> u32 {
    const CMD_KEY: u32 = 0x0100;
    const SHIFT_KEY: u32 = 0x0200;
    const OPTION_KEY: u32 = 0x0800;
    const CONTROL_KEY: u32 = 0x1000;
    let mut flags = 0;
    if mods.cmd {
        flags |= CMD_KEY;
    }
    if mods.shift {
        flags |= SHIFT_KEY;
    }
    if mods.alt {
        flags |= OPTION_KEY;
    }
    if mods.ctrl {
        flags |= CONTROL_KEY;
    }
    flags
}

/// Virtual key codes of the numeric keypad. Laptops have no keypad, so a binding there
/// would silently never fire.
const KEYPAD: std::ops::RangeInclusive<u16> = 0x41..=0x5C;

/// The lowest key code (0..128, keypad excluded) whose unshifted character is `target`.
/// `translate` asks the current keyboard layout which character a key code types.
pub fn find_code(target: char, translate: impl Fn(u16) -> Option<char>) -> Option<u16> {
    (0..128u16)
        .filter(|code| !KEYPAD.contains(code))
        .find(|&code| translate(code) == Some(target))
}

/// The key that types `target` without Shift, or, if none does, the key that types it with
/// Shift (spec section 3: on a US layout `+` is Shift+`=`). `translate(code, shift)` asks the
/// current layout which character a key types.
pub fn find_code_with_shift(
    target: char,
    translate: impl Fn(u16, bool) -> Option<char>,
) -> Option<u16> {
    find_code(target, |code| translate(code, false))
        .or_else(|| find_code(target, |code| translate(code, true)))
}

/// The resize mode's fixed keys (MODE-2, MODE-3), registered only while the mode is on
/// (MODE-5): arrows and `h j k l` without modifiers resize, Escape and Enter leave.
pub fn mode_keys() -> Vec<(KeyCombo, Action)> {
    let plain = |key| KeyCombo {
        key,
        mods: Modifiers::default(),
    };
    let toward = |direction| Action::ResizeToward(direction);
    vec![
        (plain(Key::Left), toward(Direction::Left)),
        (plain(Key::Right), toward(Direction::Right)),
        (plain(Key::Up), toward(Direction::Up)),
        (plain(Key::Down), toward(Direction::Down)),
        (plain(Key::Letter('h')), toward(Direction::Left)),
        (plain(Key::Letter('l')), toward(Direction::Right)),
        (plain(Key::Letter('k')), toward(Direction::Up)),
        (plain(Key::Letter('j')), toward(Direction::Down)),
        (plain(Key::Escape), Action::ExitMode),
        (plain(Key::Enter), Action::ExitMode),
    ]
}

/// The keys already bound while registering a config, by key code and Carbon modifiers.
/// After the layout lookup two combos can be the same physical key (US layout: `plus` is
/// Shift+`=`, so `ctrl-alt-plus` and `ctrl-alt-equal` are both the `=` key).
#[derive(Debug, Default)]
pub struct TakenKeys {
    by_key: HashMap<(u16, u32), String>,
}

impl TakenKeys {
    /// Records `name` on this key, or returns the name of the binding that already has it.
    pub fn claim(&mut self, code: u16, modifiers: u32, name: &str) -> Result<(), String> {
        match self.by_key.entry((code, modifiers)) {
            Entry::Occupied(first) => Err(first.get().clone()),
            Entry::Vacant(slot) => {
                slot.insert(name.to_string());
                Ok(())
            }
        }
    }
}

/// What each registered hotkey does, by its Carbon hotkey id (CFG-4). Ids keep counting up
/// across reloads, so a key press queued before a reload, carrying an old id, finds nothing
/// instead of running whatever the new config put at the same position.
#[derive(Debug, Default)]
pub struct HotkeyActions {
    next_id: u32,
    by_id: HashMap<u32, Action>,
}

impl HotkeyActions {
    /// Forgets every action; ids already handed out are never given again.
    pub fn clear(&mut self) {
        self.by_id.clear();
    }

    /// Stores `action` and returns the id to register its hotkey with.
    pub fn add(&mut self, action: Action) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.by_id.insert(id, action);
        id
    }

    /// Forgets one id (its hotkey wasn't registered after all).
    pub fn remove(&mut self, id: u32) {
        self.by_id.remove(&id);
    }

    pub fn get(&self, id: u32) -> Option<&Action> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_core::{Action, Direction, Modifiers};

    #[test]
    fn key_digits_use_fixed_codes() {
        assert_eq!(key_code(Key::Digit(1)), KeyCode::Fixed(0x12));
        assert_eq!(key_code(Key::Digit(0)), KeyCode::Fixed(0x1D));
    }

    #[test]
    fn key_punctuation_is_looked_up_on_layout() {
        assert_eq!(key_code(Key::Punct('+')), KeyCode::OnLayout('+'));
        assert_eq!(key_code(Key::Punct('-')), KeyCode::OnLayout('-'));
    }

    #[test]
    fn cfg_2_letters_and_named_keys_have_fixed_codes() {
        assert_eq!(key_code(Key::Letter('a')), KeyCode::Fixed(0x00));
        assert_eq!(key_code(Key::Letter('q')), KeyCode::Fixed(0x0C));
        assert_eq!(key_code(Key::Enter), KeyCode::Fixed(0x24));
        assert_eq!(key_code(Key::Space), KeyCode::Fixed(0x31));
        assert_eq!(key_code(Key::Escape), KeyCode::Fixed(0x35));
        assert_eq!(key_code(Key::F(1)), KeyCode::Fixed(0x7A));
        assert_eq!(key_code(Key::F(12)), KeyCode::Fixed(0x6F));
        assert_eq!(key_code(Key::Home), KeyCode::Fixed(0x73));
    }

    #[test]
    fn cfg_2_modifiers_map_to_carbon_flags() {
        let cmd_alt = Modifiers {
            cmd: true,
            alt: true,
            ..Modifiers::default()
        };
        let ctrl_alt_shift = Modifiers {
            ctrl: true,
            alt: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(carbon_modifiers(cmd_alt), 0x0900);
        assert_eq!(carbon_modifiers(ctrl_alt_shift), 0x1A00);
    }

    #[test]
    fn cfg_2_plus_falls_back_to_shifted_key() {
        // US layout: 0x18 types '=' and, with Shift, '+'. No key types '+' unshifted.
        let us = |code: u16, shift: bool| match (code, shift) {
            (0x18, false) => Some('='),
            (0x18, true) => Some('+'),
            _ => None,
        };
        assert_eq!(find_code_with_shift('+', us), Some(0x18));
    }

    #[test]
    fn cfg_2_unshifted_match_wins() {
        // 0x10 types '+' with Shift, 0x1E types it without: the unshifted key wins.
        let layout = |code: u16, shift: bool| match (code, shift) {
            (0x10, true) => Some('+'),
            (0x1E, false) => Some('+'),
            _ => None,
        };
        assert_eq!(find_code_with_shift('+', layout), Some(0x1E));
    }

    #[test]
    fn cfg_5_second_binding_on_the_same_key_is_refused() {
        let mut taken = TakenKeys::default();
        assert_eq!(taken.claim(0x18, 0x1800, "ctrl-alt-plus"), Ok(()));
        assert_eq!(
            taken.claim(0x18, 0x1800, "ctrl-alt-equal"),
            Err("ctrl-alt-plus".to_string())
        );
    }

    #[test]
    fn cfg_5_same_key_with_other_modifiers_is_fine() {
        let mut taken = TakenKeys::default();
        assert_eq!(taken.claim(0x18, 0x1800, "ctrl-alt-plus"), Ok(()));
        assert_eq!(taken.claim(0x18, 0x1A00, "ctrl-alt-shift-plus"), Ok(()));
        assert_eq!(taken.claim(0x1B, 0x1800, "ctrl-alt-minus"), Ok(()));
    }

    #[test]
    fn mode_2_mode_keys_are_arrows_hjkl_escape_enter() {
        let keys = mode_keys();
        assert_eq!(keys.len(), 10);
        assert!(
            keys.iter()
                .all(|(combo, _)| combo.mods == Modifiers::default())
        );
        let action_of = |key: Key| {
            keys.iter()
                .find(|(combo, _)| combo.key == key)
                .map(|(_, action)| action.clone())
        };
        assert_eq!(
            action_of(Key::Letter('h')),
            Some(Action::ResizeToward(Direction::Left))
        );
        assert_eq!(
            action_of(Key::Letter('j')),
            Some(Action::ResizeToward(Direction::Down))
        );
        assert_eq!(
            action_of(Key::Right),
            Some(Action::ResizeToward(Direction::Right))
        );
        assert_eq!(
            action_of(Key::Up),
            Some(Action::ResizeToward(Direction::Up))
        );
        assert_eq!(action_of(Key::Escape), Some(Action::ExitMode));
        assert_eq!(action_of(Key::Enter), Some(Action::ExitMode));
    }

    #[test]
    fn cfg_4_hotkey_ids_are_not_reused_after_a_reload() {
        let mut actions = HotkeyActions::default();
        let old = actions.add(Action::Close);
        actions.clear();
        let new = actions.add(Action::ToggleFloating);
        assert_ne!(old, new);
        assert_eq!(actions.get(old), None);
        assert_eq!(actions.get(new), Some(&Action::ToggleFloating));
    }

    #[test]
    fn key_find_code_scans_layout() {
        assert_eq!(
            find_code('+', |c| if c == 0x1E { Some('+') } else { None }),
            Some(0x1E)
        );
    }

    #[test]
    fn key_find_code_skips_keypad() {
        // US layout: only the keypad's + key (0x45) types '+' without Shift. Binding a
        // laptop's missing keypad would silently never fire, so it must not match.
        assert_eq!(
            find_code('+', |c| if c == 0x45 { Some('+') } else { None }),
            None
        );
    }

    #[test]
    fn key_find_code_returns_none_when_char_missing() {
        assert_eq!(find_code('+', |_| Some('a')), None);
    }
}
