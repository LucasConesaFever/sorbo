//! The configuration file (CFG-1 to CFG-10, spec `2026-11-08-config-file-design.md`).
//!
//! Text in, `Config` and errors out; no file access here (that's wm-macos's job), so it all
//! runs in unit tests on any OS. The TOML is read with `toml`'s own parser, which keeps the
//! position of every key and value, so every error can name its line.

use std::collections::HashMap;
use std::ops::{Range, RangeInclusive};

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::{
    Action, App, Direction, Gaps, Key, KeyCombo, Modifiers, ResizeStep, RuleEffect, WorkspaceNumber,
};

/// The default file shipped in the repo (`config/sorbo.toml`), built in so that "no file"
/// and "the shipped file" are the same thing (CFG-1, CFG-10).
pub const DEFAULT_CONFIG_TEXT: &str = include_str!("../../../config/sorbo.toml");

/// Allowed gap values, in points.
const GAP_RANGE: RangeInclusive<f64> = 0.0..=100.0;

/// What a config file sets.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// In file order.
    pub bindings: Vec<(KeyCombo, Action)>,
    pub gaps: Gaps,
    /// Window rules, in file order (RULE-1).
    pub rules: Vec<Rule>,
}

/// `[[rules]]`: windows of `app` start floating or tiled (RULE-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// An app name (`Calculator`) or bundle id (`com.apple.calculator`), any case.
    pub app: String,
    pub floating: bool,
}

/// What `rules` say about a window of the app with this bundle id and name: every
/// matching rule applies in order, so a later one wins (RULE-1).
pub fn rule_effect(rules: &[Rule], bundle_id: Option<&str>, name: Option<&str>) -> RuleEffect {
    let matches = |rule: &Rule| {
        [bundle_id, name]
            .into_iter()
            .flatten()
            .any(|id| id.eq_ignore_ascii_case(&rule.app))
    };
    RuleEffect {
        // The last matching rule wins: search from the end.
        floating: rules.iter().rev().find(|r| matches(r)).map(|r| r.floating),
    }
}

/// One problem with the file. `line` is 1-based; `None` when it isn't about one line.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConfigError {
    pub line: Option<usize>,
    pub message: String,
}

/// The result of reading a file's text.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseOutcome {
    /// `None` when the file can't be used at all (not valid TOML, or no keybinding in it);
    /// `errors` then says why.
    pub config: Option<Config>,
    pub errors: Vec<ConfigError>,
}

/// The built-in configuration: the shipped default file.
pub fn default_config() -> Config {
    let outcome = parse_config(DEFAULT_CONFIG_TEXT);
    match outcome.config {
        Some(config) if outcome.errors.is_empty() => config,
        // A test checks the shipped file parses cleanly, so this can't happen in a build.
        _ => panic!("the built-in config has errors: {:?}", outcome.errors),
    }
}

/// `sorbo.toml line 12: <message>`, or `sorbo.toml: <message>` without a line.
pub fn format_error(file_name: &str, e: &ConfigError) -> String {
    match e.line {
        Some(line) => format!("{file_name} line {line}: {}", e.message),
        None => format!("{file_name}: {}", e.message),
    }
}

/// Reads a config file's text (CFG-5): a bad line is reported and skipped, the rest
/// applies; invalid TOML or a file without any valid binding can't be used at all.
pub fn parse_config(text: &str) -> ParseOutcome {
    let table = match DeTable::parse(text) {
        Ok(table) => table.into_inner(),
        Err(e) => {
            return ParseOutcome {
                config: None,
                errors: vec![ConfigError {
                    line: e.span().map(|span| line_of(text, span.start)),
                    message: format!("not valid TOML: {}", e.message().trim()),
                }],
            };
        }
    };
    let mut parser = Parser {
        text,
        errors: Vec::new(),
    };
    let mut gaps = Gaps::DEFAULT;
    let mut bindings = Vec::new();
    let mut rules = Vec::new();
    for (key, value) in in_file_order(&table) {
        match (key.get_ref().as_ref(), value.get_ref()) {
            ("gaps", DeValue::Table(t)) => gaps = parser.gaps(t),
            ("bindings", DeValue::Table(t)) => bindings = parser.bindings(t),
            ("rules", DeValue::Array(a)) => rules = parser.rules(a),
            ("rules", _) => parser.error(
                key.span(),
                "write each rule as its own [[rules]] block (two brackets)".to_string(),
            ),
            ("gaps" | "bindings", _) => {
                let name = key.get_ref();
                parser.error(
                    key.span(),
                    format!("{name} must be a section, written [{name}]"),
                );
            }
            (name, _) => parser.error(
                key.span(),
                format!("unknown section \"{name}\" (expected [gaps], [bindings] or [[rules]])"),
            ),
        }
    }
    let mut errors = parser.errors;
    if bindings.is_empty() {
        errors.push(ConfigError {
            line: None,
            message: "no keybindings in the file (expected a [bindings] section with at least \
                      one valid line)"
                .to_string(),
        });
        return ParseOutcome {
            config: None,
            errors,
        };
    }
    ParseOutcome {
        config: Some(Config {
            bindings,
            gaps,
            rules,
        }),
        errors,
    }
}

/// A table's entries sorted by where they are in the file (the map itself is sorted by key).
fn in_file_order<'a, 'i>(
    table: &'a DeTable<'i>,
) -> Vec<(
    &'a Spanned<std::borrow::Cow<'i, str>>,
    &'a Spanned<DeValue<'i>>,
)> {
    let mut entries: Vec<_> = table.iter().collect();
    entries.sort_by_key(|(key, _)| key.span().start);
    entries
}

/// The 1-based line of byte `offset` in `text`.
fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Collects errors while reading the sections.
struct Parser<'t> {
    text: &'t str,
    errors: Vec<ConfigError>,
}

impl Parser<'_> {
    fn error(&mut self, span: Range<usize>, message: String) {
        self.errors.push(ConfigError {
            line: Some(line_of(self.text, span.start)),
            message,
        });
    }

    /// `[gaps]`: a missing or bad value keeps its default.
    fn gaps(&mut self, table: &DeTable) -> Gaps {
        let mut gaps = Gaps::DEFAULT;
        for (key, value) in in_file_order(table) {
            let name = key.get_ref().as_ref();
            let (slot, default) = match name {
                "outer" => (&mut gaps.outer, Gaps::DEFAULT.outer),
                "inner" => (&mut gaps.inner, Gaps::DEFAULT.inner),
                _ => {
                    self.error(
                        key.span(),
                        format!("unknown gap setting \"{name}\" (expected outer or inner)"),
                    );
                    continue;
                }
            };
            match number(value.get_ref()) {
                Some(n) if GAP_RANGE.contains(&n) => *slot = n,
                _ => self.error(
                    value.span(),
                    format!("gap \"{name}\" must be a number between 0 and 100; using {default}"),
                ),
            }
        }
        gaps
    }

    /// `[bindings]`, in file order. A bad line is reported and skipped; the same combo
    /// written a second time (in another spelling) is reported and the first one kept.
    fn bindings(&mut self, table: &DeTable) -> Vec<(KeyCombo, Action)> {
        let mut out = Vec::new();
        // Where each combo was first bound: its spelling and line.
        let mut seen: HashMap<KeyCombo, (String, usize)> = HashMap::new();
        for (key, value) in in_file_order(table) {
            let spelling = key.get_ref().as_ref();
            let combo = match parse_combo(spelling) {
                Ok(combo) => combo,
                Err(message) => {
                    self.error(key.span(), message);
                    continue;
                }
            };
            let action = match value.get_ref() {
                DeValue::String(s) => parse_action(s),
                _ => Err(format!(
                    "the action for \"{spelling}\" must be text in quotes, like \"close\""
                )),
            };
            let action = match action {
                Ok(action) => action,
                Err(message) => {
                    self.error(value.span(), message);
                    continue;
                }
            };
            if let Some((first, line)) = seen.get(&combo) {
                let message = format!(
                    "\"{spelling}\" is the same keys as \"{first}\" on line {line}; the first \
                     one is used"
                );
                self.error(key.span(), message);
                continue;
            }
            seen.insert(
                combo,
                (spelling.to_string(), line_of(self.text, key.span().start)),
            );
            out.push((combo, action));
        }
        out
    }
}

impl Parser<'_> {
    /// `[[rules]]` blocks, in file order. A rule with any mistake is reported and skipped.
    fn rules(&mut self, blocks: &toml::de::DeArray) -> Vec<Rule> {
        let mut out = Vec::new();
        for block in blocks {
            let DeValue::Table(table) = block.get_ref() else {
                self.error(
                    block.span(),
                    "each rule must be a [[rules]] block".to_string(),
                );
                continue;
            };
            let errors_before = self.errors.len();
            let (mut app, mut floating, mut floating_seen) = (None, None, false);
            for (key, value) in in_file_order(table) {
                match (key.get_ref().as_ref(), value.get_ref()) {
                    ("app", DeValue::String(name)) if !name.trim().is_empty() => {
                        app = Some(name.trim().to_string());
                    }
                    ("app", _) => self.error(
                        value.span(),
                        "app must be an app name or bundle id in quotes, like \"Calculator\""
                            .to_string(),
                    ),
                    ("floating", value_ref) => {
                        floating_seen = true;
                        match value_ref {
                            DeValue::Boolean(b) => floating = Some(*b),
                            _ => self.error(
                                value.span(),
                                "floating must be true or false (without quotes)".to_string(),
                            ),
                        }
                    }
                    (other, _) => self.error(
                        key.span(),
                        format!("unknown rule setting \"{other}\" (expected app or floating)"),
                    ),
                }
            }
            if app.is_none() && self.errors.len() == errors_before {
                self.error(
                    block.span(),
                    "this rule needs app = \"<app name or bundle id>\"".to_string(),
                );
            }
            if !floating_seen && self.errors.len() == errors_before {
                self.error(
                    block.span(),
                    "this rule needs floating = true or false".to_string(),
                );
            }
            if self.errors.len() > errors_before {
                continue;
            }
            if let (Some(app), Some(floating)) = (app, floating) {
                out.push(Rule { app, floating });
            }
        }
        out
    }
}

/// A TOML integer or float as `f64`.
fn number(value: &DeValue) -> Option<f64> {
    match value {
        DeValue::Integer(i) => {
            // toml has already dropped the 0x/0o/0b prefix here and gives the base as
            // `radix()`. Stripping it again turned `0x0b4` into `4` (HARD-4).
            let digits = i.as_str().replace('_', "");
            i64::from_str_radix(&digits, i.radix())
                .ok()
                .map(|n| n as f64)
        }
        DeValue::Float(f) => f.as_str().replace('_', "").parse().ok(),
        _ => None,
    }
}

const KEYS_EXPECTED: &str = "expected a-z, 0-9, enter, tab, space, escape, backspace, delete, \
                             left, right, up, down, home, end, pageup, pagedown, f1-f12, or a \
                             punctuation name: plus, minus, equal, comma, period, slash, \
                             backslash, semicolon, quote, backtick, leftbracket, rightbracket";

/// `"ctrl-alt-shift-q"` → the combo (CFG-2).
pub fn parse_combo(spelling: &str) -> Result<KeyCombo, String> {
    let lower = spelling.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('-').collect();
    let key_name = parts.pop().unwrap_or_default();
    let mut mods = Modifiers::default();
    for part in parts {
        match part {
            "cmd" => mods.cmd = true,
            "alt" => mods.alt = true,
            "ctrl" => mods.ctrl = true,
            "shift" => mods.shift = true,
            other => {
                return Err(format!(
                    "unknown modifier \"{other}\" in \"{spelling}\" (expected cmd, alt, ctrl or \
                     shift)"
                ));
            }
        }
    }
    let key = parse_key(key_name)
        .ok_or_else(|| format!("unknown key \"{key_name}\" in \"{spelling}\" ({KEYS_EXPECTED})"))?;
    if !(mods.cmd || mods.alt || mods.ctrl) {
        return Err(format!(
            "\"{spelling}\" needs cmd, alt or ctrl (without them it would swallow normal typing)"
        ));
    }
    Ok(KeyCombo { key, mods })
}

/// Keys spelled with a name (letters, digits and f1-f12 are handled apart).
const NAMED_KEYS: [(&str, Key); 26] = [
    ("enter", Key::Enter),
    ("tab", Key::Tab),
    ("space", Key::Space),
    ("escape", Key::Escape),
    ("backspace", Key::Backspace),
    ("delete", Key::Delete),
    ("left", Key::Left),
    ("right", Key::Right),
    ("up", Key::Up),
    ("down", Key::Down),
    ("home", Key::Home),
    ("end", Key::End),
    ("pageup", Key::PageUp),
    ("pagedown", Key::PageDown),
    ("plus", Key::Punct('+')),
    ("minus", Key::Punct('-')),
    ("equal", Key::Punct('=')),
    ("comma", Key::Punct(',')),
    ("period", Key::Punct('.')),
    ("slash", Key::Punct('/')),
    ("backslash", Key::Punct('\\')),
    ("semicolon", Key::Punct(';')),
    ("quote", Key::Punct('\'')),
    ("backtick", Key::Punct('`')),
    ("leftbracket", Key::Punct('[')),
    ("rightbracket", Key::Punct(']')),
];

fn parse_key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return match c {
            'a'..='z' => Some(Key::Letter(c)),
            '0'..='9' => Some(Key::Digit(c as u8 - b'0')),
            _ => None,
        };
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=12).contains(&n).then_some(Key::F(n));
    }
    NAMED_KEYS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, key)| *key)
}

/// How a key is written in the file (`plus`, `f7`, `q`).
fn key_name(key: Key) -> String {
    match key {
        Key::Letter(c) => c.to_string(),
        Key::Digit(d) => d.to_string(),
        Key::F(n) => format!("f{n}"),
        other => NAMED_KEYS
            .iter()
            .find(|(_, known)| *known == other)
            .map_or_else(|| format!("{other:?}"), |(name, _)| name.to_string()),
    }
}

/// How a combo is written in the file, modifiers in a fixed order: `cmd-ctrl-alt-shift-q`.
/// Used to name a binding in messages that have no line (macOS refused it).
pub fn combo_name(combo: &KeyCombo) -> String {
    let m = combo.mods;
    let mods = [
        (m.cmd, "cmd"),
        (m.ctrl, "ctrl"),
        (m.alt, "alt"),
        (m.shift, "shift"),
    ];
    let mut parts: Vec<String> = mods
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name.to_string())
        .collect();
    parts.push(key_name(combo.key));
    parts.join("-")
}

const ACTIONS_EXPECTED: &str = "expected workspace, move-to-workspace, focus, swap, resize, \
                                resize-mode, toggle-split, toggle-floating, close or new-window";

/// `"focus left"` → the action (CFG-3, spec section 3's table).
pub fn parse_action(text: &str) -> Result<Action, String> {
    let text = text.trim();
    let (verb, rest) = match text.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (text, ""),
    };
    let no_argument = |action: Action| {
        if rest.is_empty() {
            Ok(action)
        } else {
            Err(format!("\"{verb}\" takes nothing after it, got \"{text}\""))
        }
    };
    match verb {
        "workspace" => match rest {
            "next" => Ok(Action::NextWorkspace),
            "prev" => Ok(Action::PrevWorkspace),
            n => workspace_number(text, n, "a number, next or prev").map(Action::GoToWorkspace),
        },
        "move-to-workspace" => {
            workspace_number(text, rest, "a number").map(Action::MoveToWorkspace)
        }
        "focus" => direction(text, rest).map(Action::Focus),
        "swap" => direction(text, rest).map(Action::Swap),
        "resize" => match rest {
            "grow" => Ok(Action::Resize(ResizeStep::Grow)),
            "shrink" => Ok(Action::Resize(ResizeStep::Shrink)),
            _ => Err(format!(
                "unknown resize in \"{text}\" (expected grow or shrink)"
            )),
        },
        "resize-mode" => no_argument(Action::ResizeMode),
        "toggle-split" => no_argument(Action::ToggleSplit),
        "toggle-floating" => no_argument(Action::ToggleFloating),
        "close" => no_argument(Action::Close),
        "new-window" => match rest {
            "" => Err(format!(
                "\"{text}\" needs an app: a bundle id, an app name or browser"
            )),
            // Names become `<folder>/<name>.app`; a "/" could point anywhere (SAFE-7).
            app if app.contains('/') => Err(format!("\"{text}\": app names can't contain \"/\"")),
            "browser" => Ok(Action::Launch(App::Browser)),
            app if app.contains('.') && !app.contains(char::is_whitespace) => {
                Ok(Action::Launch(App::Bundle(app.to_string())))
            }
            app => Ok(Action::Launch(App::Named(app.to_string()))),
        },
        _ => Err(format!("unknown action \"{text}\" ({ACTIONS_EXPECTED})")),
    }
}

fn workspace_number(text: &str, n: &str, expected: &str) -> Result<WorkspaceNumber, String> {
    match n.parse::<u32>().map(WorkspaceNumber::new) {
        Ok(Some(n)) => Ok(n),
        Ok(None) => Err(format!("workspace numbers start at 1, got \"{text}\"")),
        Err(_) => Err(format!("bad workspace in \"{text}\" (expected {expected})")),
    }
}

fn direction(text: &str, name: &str) -> Result<Direction, String> {
    match name {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        _ => Err(format!(
            "unknown direction in \"{text}\" (expected left, right, up or down)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::ws;
    use crate::{Action, App, Direction, Gaps, Key, KeyCombo, Modifiers, ResizeStep, RuleEffect};

    fn ctrl_alt(key: Key) -> KeyCombo {
        KeyCombo {
            key,
            mods: Modifiers {
                ctrl: true,
                alt: true,
                ..Modifiers::default()
            },
        }
    }

    fn ctrl_alt_shift(key: Key) -> KeyCombo {
        let mut combo = ctrl_alt(key);
        combo.mods.shift = true;
        combo
    }

    /// Parses a `[bindings]` section made of `lines`.
    fn bindings(lines: &str) -> ParseOutcome {
        parse_config(&format!("[bindings]\n{lines}"))
    }

    fn only_error(outcome: &ParseOutcome) -> &ConfigError {
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        &outcome.errors[0]
    }

    #[test]
    fn cfg_1_default_file_has_no_errors_and_39_bindings() {
        let outcome = parse_config(DEFAULT_CONFIG_TEXT);
        assert_eq!(outcome.errors, vec![]);
        assert_eq!(outcome.config.unwrap().bindings.len(), 39);
    }

    #[test]
    fn cfg_1_default_file_matches_key_7() {
        let config = default_config();
        let has = |combo, action| config.bindings.contains(&(combo, action));
        assert!(has(ctrl_alt(Key::Digit(0)), Action::GoToWorkspace(ws(10))));
        assert!(has(
            ctrl_alt_shift(Key::Digit(3)),
            Action::MoveToWorkspace(ws(3))
        ));
        assert!(has(
            ctrl_alt(Key::Punct('+')),
            Action::Resize(ResizeStep::Grow)
        ));
        assert!(has(
            ctrl_alt(Key::Enter),
            Action::Launch(App::Bundle("com.apple.Terminal".into()))
        ));
        assert!(has(
            ctrl_alt(Key::Letter('b')),
            Action::Launch(App::Browser)
        ));
        assert_eq!(config.gaps, Gaps::DEFAULT);
    }

    #[test]
    fn mode_1_parses_resize_mode() {
        let config = bindings("ctrl-alt-r = \"resize-mode\"").config.unwrap();
        assert_eq!(
            config.bindings,
            vec![(ctrl_alt(Key::Letter('r')), Action::ResizeMode)]
        );
        let outcome = bindings("ctrl-alt-r = \"resize-mode now\"\nctrl-alt-q = \"close\"");
        assert!(
            only_error(&outcome)
                .message
                .contains("takes nothing after it")
        );
    }

    #[test]
    fn mode_1_default_file_binds_ctrl_alt_r() {
        let outcome = parse_config(DEFAULT_CONFIG_TEXT);
        assert!(outcome.errors.is_empty());
        assert!(
            outcome
                .config
                .unwrap()
                .bindings
                .contains(&(ctrl_alt(Key::Letter('r')), Action::ResizeMode))
        );
    }

    #[test]
    fn cfg_2_modifiers_in_any_order() {
        let a = bindings("alt-ctrl-x = \"close\"").config.unwrap();
        let b = bindings("ctrl-alt-x = \"close\"").config.unwrap();
        assert_eq!(a.bindings, b.bindings);
        assert_eq!(a.bindings[0].0, ctrl_alt(Key::Letter('x')));
    }

    #[test]
    fn cfg_2_combo_needs_cmd_alt_or_ctrl() {
        let outcome = bindings("shift-x = \"close\"\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(2));
        assert!(
            e.message.contains("needs cmd, alt or ctrl"),
            "{}",
            e.message
        );
        assert_eq!(outcome.config.unwrap().bindings.len(), 1);
    }

    #[test]
    fn cfg_2_unknown_key() {
        let outcome = bindings("ctrl-alt-foo = \"close\"\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("unknown key \"foo\""), "{}", e.message);
    }

    #[test]
    fn cfg_2_unknown_modifier() {
        let outcome = bindings("hyper-alt-x = \"close\"\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert!(
            e.message.contains("unknown modifier \"hyper\""),
            "{}",
            e.message
        );
    }

    #[test]
    fn cfg_2_named_and_punctuation_keys() {
        let config =
            bindings("cmd-f12 = \"close\"\nctrl-pagedown = \"close\"\nalt-backslash = \"close\"")
                .config
                .unwrap();
        let keys: Vec<Key> = config.bindings.iter().map(|(c, _)| c.key).collect();
        assert_eq!(keys, vec![Key::F(12), Key::PageDown, Key::Punct('\\')]);
    }

    #[test]
    fn safe_7_new_window_name_with_slash_is_an_error() {
        let outcome = bindings(concat!(
            "ctrl-alt-a = \"new-window com.apple.Terminal\"\n",
            "ctrl-alt-x = \"new-window ../../tmp/evil\"\n",
        ));
        let error = only_error(&outcome);
        assert_eq!(error.line, Some(3));
        assert!(
            error.message.contains("app names can't contain \"/\""),
            "{}",
            error.message
        );
    }

    #[test]
    fn cfg_3_new_window_forms() {
        let config = bindings(concat!(
            "ctrl-alt-a = \"new-window browser\"\n",
            "ctrl-alt-b = \"new-window com.mitchellh.ghostty\"\n",
            "ctrl-alt-c = \"new-window kitty\"\n",
            "ctrl-alt-d = \"new-window Google Chrome\"\n",
        ))
        .config
        .unwrap();
        let apps: Vec<Action> = config.bindings.into_iter().map(|(_, a)| a).collect();
        assert_eq!(
            apps,
            vec![
                Action::Launch(App::Browser),
                Action::Launch(App::Bundle("com.mitchellh.ghostty".into())),
                Action::Launch(App::Named("kitty".into())),
                Action::Launch(App::Named("Google Chrome".into())),
            ]
        );
    }

    #[test]
    fn cfg_5_bad_action_line_is_skipped() {
        let outcome = bindings(concat!(
            "ctrl-alt-q = \"close\"\n",
            "ctrl-alt-left = \"focus lft\"\n",
            "ctrl-alt-j = \"toggle-split\"\n",
        ));
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(3));
        assert!(
            e.message.contains("expected left, right, up or down"),
            "{}",
            e.message
        );
        let config = outcome.config.unwrap();
        assert_eq!(
            config.bindings,
            vec![
                (ctrl_alt(Key::Letter('q')), Action::Close),
                (ctrl_alt(Key::Letter('j')), Action::ToggleSplit),
            ]
        );
    }

    #[test]
    fn cfg_5_workspace_zero_is_rejected() {
        let outcome = bindings("ctrl-alt-0 = \"workspace 0\"\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert!(
            e.message.contains("workspace numbers start at 1"),
            "{}",
            e.message
        );
    }

    #[test]
    fn cfg_5_workspace_forms() {
        let config = bindings(concat!(
            "ctrl-alt-1 = \"workspace 12\"\n",
            "ctrl-alt-2 = \"workspace next\"\n",
            "ctrl-alt-3 = \"workspace prev\"\n",
            "ctrl-alt-4 = \"move-to-workspace 4\"\n",
            "ctrl-alt-5 = \"swap up\"\n",
            "ctrl-alt-6 = \"resize shrink\"\n",
            "ctrl-alt-7 = \"toggle-floating\"\n",
        ))
        .config
        .unwrap();
        let actions: Vec<Action> = config.bindings.into_iter().map(|(_, a)| a).collect();
        assert_eq!(
            actions,
            vec![
                Action::GoToWorkspace(ws(12)),
                Action::NextWorkspace,
                Action::PrevWorkspace,
                Action::MoveToWorkspace(ws(4)),
                Action::Swap(Direction::Up),
                Action::Resize(ResizeStep::Shrink),
                Action::ToggleFloating,
            ]
        );
    }

    #[test]
    fn cfg_5_same_combo_spelled_twice_first_wins() {
        let outcome = bindings("ctrl-alt-q = \"close\"\nalt-ctrl-q = \"toggle-split\"");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("alt-ctrl-q"), "{}", e.message);
        assert_eq!(
            outcome.config.unwrap().bindings,
            vec![(ctrl_alt(Key::Letter('q')), Action::Close)]
        );
    }

    #[test]
    fn cfg_5_unknown_section_is_reported() {
        let outcome = parse_config("[bindigs]\nctrl-alt-q = \"close\"\n");
        assert!(outcome.config.is_none());
        let unknown = outcome
            .errors
            .iter()
            .find(|e| e.message.contains("unknown section \"bindigs\""))
            .expect("unknown section reported");
        assert_eq!(unknown.line, Some(1));
    }

    #[test]
    fn cfg_5_zero_bindings_is_unusable() {
        let outcome = parse_config("[bindings]\n");
        assert!(outcome.config.is_none());
        assert!(only_error(&outcome).message.contains("no keybindings"));
    }

    #[test]
    fn cfg_5_invalid_toml_is_unusable() {
        let outcome =
            parse_config("[bindings]\nctrl-alt-q = \"close\"\nctrl-alt-j = \"toggle-split\n");
        assert!(outcome.config.is_none());
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("not valid TOML"), "{}", e.message);
    }

    #[test]
    fn cfg_5_exact_duplicate_key_is_invalid_toml() {
        let outcome = bindings("ctrl-alt-q = \"close\"\nctrl-alt-q = \"toggle-split\"");
        assert!(outcome.config.is_none());
        assert_eq!(only_error(&outcome).line, Some(3));
    }

    #[test]
    fn cfg_5_action_must_be_a_string() {
        let outcome = bindings("ctrl-alt-1 = 1\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn cfg_1_missing_gaps_use_defaults() {
        let config = bindings("ctrl-alt-q = \"close\"").config.unwrap();
        assert_eq!(config.gaps, Gaps::DEFAULT);
        let config = parse_config("[gaps]\nouter = 10\n[bindings]\nctrl-alt-q = \"close\"")
            .config
            .unwrap();
        assert_eq!(
            config.gaps,
            Gaps {
                outer: 10.0,
                inner: 4.0
            }
        );
    }

    #[test]
    fn hard_4_hex_octal_binary_gaps() {
        let c =
            parse_config("[gaps]\nouter = 0x0b\ninner = 0o17\n[bindings]\nctrl-alt-q = \"close\"")
                .config
                .unwrap();
        assert_eq!(
            c.gaps,
            Gaps {
                outer: 11.0,
                inner: 15.0
            }
        );
        let c =
            parse_config("[gaps]\nouter = 0b101\ninner = 1_0\n[bindings]\nctrl-alt-q = \"close\"")
                .config
                .unwrap();
        assert_eq!(
            c.gaps,
            Gaps {
                outer: 5.0,
                inner: 10.0
            }
        );
    }

    #[test]
    fn hard_4_hex_out_of_range_is_an_error() {
        let outcome = parse_config("[gaps]\nouter = 0x0b4\n[bindings]\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert!(e.message.contains("between 0 and 100"), "{}", e.message);
    }

    #[test]
    fn cfg_5_gap_out_of_range_uses_default() {
        let outcome =
            parse_config("[gaps]\nouter = 8.5\ninner = 5000\n[bindings]\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("between 0 and 100"), "{}", e.message);
        assert_eq!(
            outcome.config.unwrap().gaps,
            Gaps {
                outer: 8.5,
                inner: 4.0
            }
        );
        let negative = parse_config("[gaps]\nouter = -3\n[bindings]\nctrl-alt-q = \"close\"");
        assert_eq!(negative.errors.len(), 1);
        assert_eq!(negative.config.unwrap().gaps, Gaps::DEFAULT);
    }

    #[test]
    fn cfg_5_gap_not_a_number_uses_default() {
        let outcome = parse_config("[gaps]\nouter = \"x\"\n[bindings]\nctrl-alt-q = \"close\"");
        assert_eq!(only_error(&outcome).line, Some(2));
        assert_eq!(outcome.config.unwrap().gaps, Gaps::DEFAULT);
    }

    #[test]
    fn cfg_5_unknown_gap_setting_is_reported() {
        let outcome = parse_config("[gaps]\nmiddle = 3\n[bindings]\nctrl-alt-q = \"close\"");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(2));
        assert!(
            e.message.contains("unknown gap setting \"middle\""),
            "{}",
            e.message
        );
        assert!(outcome.config.is_some());
    }

    fn rule(app: &str, floating: bool) -> Rule {
        Rule {
            app: app.into(),
            floating,
        }
    }

    #[test]
    fn rule_1_matches_name_or_bundle_ignoring_case() {
        let by_name = [rule("calculator", true)];
        let by_bundle = [rule("COM.APPLE.Calculator", true)];
        let calc = (Some("com.apple.calculator"), Some("Calculator"));
        for rules in [&by_name[..], &by_bundle[..]] {
            assert_eq!(
                rule_effect(rules, calc.0, calc.1),
                RuleEffect {
                    floating: Some(true)
                }
            );
        }
    }

    #[test]
    fn rule_1_later_rules_win() {
        let rules = [rule("Calculator", true), rule("calculator", false)];
        assert_eq!(
            rule_effect(&rules, None, Some("Calculator")).floating,
            Some(false)
        );
    }

    #[test]
    fn rule_1_no_match_is_no_effect() {
        let rules = [rule("Calculator", true)];
        assert_eq!(
            rule_effect(&rules, Some("com.tinyspeck.slackmacgap"), Some("Slack")),
            RuleEffect::default()
        );
        assert_eq!(rule_effect(&rules, None, None), RuleEffect::default());
    }

    #[test]
    fn rule_3_parses_rules() {
        let config = parse_config(concat!(
            "[bindings]\n",
            "ctrl-alt-q = \"close\"\n",
            "[[rules]]\n",
            "app = \"Calculator\"\n",
            "floating = true\n",
            "[[rules]]\n",
            "app = \"System Settings\"\n",
            "floating = true\n",
        ))
        .config
        .unwrap();
        assert_eq!(
            config.rules,
            vec![rule("Calculator", true), rule("System Settings", true)]
        );
    }

    #[test]
    fn rule_3_bad_rules_are_skipped_with_their_line() {
        let outcome = parse_config(concat!(
            "[bindings]\n",             // 1
            "ctrl-alt-q = \"close\"\n", // 2
            "[[rules]]\n",              // 3
            "floating = true\n",        // 4: no app
            "[[rules]]\n",              // 5
            "app = \"Slack\"\n",        // 6
            "floating = \"yes\"\n",     // 7: not a boolean
            "[[rules]]\n",              // 8
            "app = \"Slack\"\n",        // 9
            "workspace = 3\n",          // 10: unknown setting
            "floating = true\n",        // 11
            "[[rules]]\n",              // 12
            "app = \"Discord\"\n",      // 13: no floating
            "[[rules]]\n",              // 14
            "app = \"Calculator\"\n",   // 15
            "floating = true\n",        // 16
        ));
        let config = outcome.config.unwrap();
        assert_eq!(config.rules, vec![rule("Calculator", true)]);
        let lines: Vec<Option<usize>> = outcome.errors.iter().map(|e| e.line).collect();
        assert_eq!(
            lines,
            vec![Some(3), Some(7), Some(10), Some(12)],
            "{:?}",
            outcome.errors
        );
        assert!(
            outcome.errors[0].message.contains("needs app"),
            "{}",
            outcome.errors[0].message
        );
        assert!(
            outcome.errors[1].message.contains("true or false"),
            "{}",
            outcome.errors[1].message
        );
        assert!(
            outcome.errors[2]
                .message
                .contains("unknown rule setting \"workspace\""),
            "{}",
            outcome.errors[2].message
        );
        assert!(
            outcome.errors[3].message.contains("needs floating"),
            "{}",
            outcome.errors[3].message
        );
    }

    #[test]
    fn rule_3_rules_as_a_table_is_reported() {
        let outcome =
            parse_config("[bindings]\nctrl-alt-q = \"close\"\n[rules]\napp = \"Calculator\"\n");
        let e = only_error(&outcome);
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("[[rules]]"), "{}", e.message);
    }

    #[test]
    fn rule_3_default_file_has_no_rules() {
        assert!(default_config().rules.is_empty());
    }

    #[test]
    fn cfg_7_combo_name_round_trips() {
        assert_eq!(combo_name(&ctrl_alt(Key::Punct('+'))), "ctrl-alt-plus");
        assert_eq!(combo_name(&ctrl_alt_shift(Key::Tab)), "ctrl-alt-shift-tab");
        for (combo, _) in default_config().bindings {
            assert_eq!(parse_combo(&combo_name(&combo)), Ok(combo));
        }
        for name in [
            "cmd-f7",
            "alt-pagedown",
            "ctrl-backtick",
            "cmd-quote",
            "ctrl-z",
        ] {
            assert_eq!(combo_name(&parse_combo(name).unwrap()), name);
        }
    }

    #[test]
    fn cfg_7_format_error_with_and_without_line() {
        let with = ConfigError {
            line: Some(12),
            message: "unknown action \"x\"".into(),
        };
        let without = ConfigError {
            line: None,
            message: "no keybindings in the file".into(),
        };
        assert_eq!(
            format_error("sorbo.toml", &with),
            "sorbo.toml line 12: unknown action \"x\""
        );
        assert_eq!(
            format_error("sorbo.toml", &without),
            "sorbo.toml: no keybindings in the file"
        );
    }
}
