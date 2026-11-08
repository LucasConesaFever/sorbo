//! What the config error banner and the menu bar dot show (CFG-6, spec section 6). Pure:
//! the panel in `macos::banner` only draws `lines()`.

use std::time::{Duration, Instant};

use wm_core::{ConfigError, format_error};

/// At most this many errors are listed; the rest are counted.
const MAX_LISTED: usize = 5;
/// How long a runtime error (an app that isn't installed) stays.
const RUNTIME_ERROR_TIME: Duration = Duration::from_secs(5);

/// The config's errors, whether the author dismissed them, and a passing runtime error.
#[derive(Debug)]
pub struct BannerState {
    /// Names the file in each line (`sorbo.toml line 3: ...`).
    file_name: String,
    errors: Vec<ConfigError>,
    /// The error set that was showing when the banner was clicked away.
    dismissed: Option<Vec<ConfigError>>,
    /// A runtime error and when it happened.
    transient: Option<(String, Instant)>,
}

impl BannerState {
    pub fn new(file_name: &str) -> BannerState {
        BannerState {
            file_name: file_name.to_string(),
            errors: Vec::new(),
            dismissed: None,
            transient: None,
        }
    }

    /// The config's error set after a load. A dismissal holds only while the set is the
    /// one that was dismissed: a different set shows again.
    pub fn set_config_errors(&mut self, errors: Vec<ConfigError>) {
        // Any other set (an empty one included) ends the dismissal, so the same mistake
        // made again after a fix shows again.
        if self.dismissed.as_ref() != Some(&errors) {
            self.dismissed = None;
        }
        self.errors = errors;
    }

    /// The banner was clicked: hide it until the error set changes or the dot is clicked.
    pub fn dismiss(&mut self) {
        self.dismissed = Some(self.errors.clone());
        self.transient = None;
    }

    /// The dot was clicked.
    pub fn show_again(&mut self) {
        self.dismissed = None;
    }

    /// Something a keypress couldn't do (spec section 6): shown for 5 seconds, even when
    /// the config errors are dismissed, and never lights the dot.
    pub fn runtime_error(&mut self, message: String, now: Instant) {
        self.transient = Some((message, now));
    }

    /// The banner's lines; empty means hidden.
    pub fn lines(&self, now: Instant) -> Vec<String> {
        let mut lines = Vec::new();
        let dismissed = self.dismissed.as_ref() == Some(&self.errors);
        if !self.errors.is_empty() && !dismissed {
            lines.push(format!(
                "{} has errors — fix and save, details in the log",
                self.file_name
            ));
            lines.extend(
                self.errors
                    .iter()
                    .take(MAX_LISTED)
                    .map(|e| format_error(&self.file_name, e)),
            );
            if self.errors.len() > MAX_LISTED {
                let more = self.errors.len() - MAX_LISTED;
                lines.push(format!("…and {more} more (see the log)"));
            }
        }
        if let Some((message, at)) = &self.transient
            && now.duration_since(*at) < RUNTIME_ERROR_TIME
        {
            lines.push(message.clone());
        }
        lines
    }

    /// True while the config has errors, shown or dismissed.
    pub fn dot(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Splits `line` into lines no wider than `max_width` (CFG-6: a long error stays readable
/// instead of running off the screen). Breaks at spaces; a word wider than the banner is
/// split between characters. `measure` gives a text's drawn width.
pub fn wrap_line(line: &str, max_width: f64, measure: impl Fn(&str) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in line.split(' ') {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= max_width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        // The word starts a new line; split it if it's too wide even alone.
        for c in word.chars() {
            let mut longer = current.clone();
            longer.push(c);
            if measure(&longer) > max_width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current.push(c);
            } else {
                current = longer;
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use wm_core::ConfigError;

    const HEADER: &str = "sorbo.toml has errors — fix and save, details in the log";

    fn error(line: usize, message: &str) -> ConfigError {
        ConfigError {
            line: Some(line),
            message: message.into(),
        }
    }

    fn with_errors(errors: Vec<ConfigError>) -> BannerState {
        let mut state = BannerState::new("sorbo.toml");
        state.set_config_errors(errors);
        state
    }

    /// Every character is one unit wide.
    fn chars(text: &str) -> f64 {
        text.chars().count() as f64
    }

    #[test]
    fn cfg_6_short_line_is_kept_whole() {
        assert_eq!(
            wrap_line("line 3: bad key", 40.0, chars),
            vec!["line 3: bad key"]
        );
    }

    #[test]
    fn cfg_6_long_line_wraps_at_spaces() {
        assert_eq!(
            wrap_line("line 12: unknown action \"focs left\"", 20.0, chars),
            vec!["line 12: unknown", "action \"focs left\""]
        );
    }

    #[test]
    fn cfg_6_word_wider_than_the_banner_is_split() {
        assert_eq!(
            wrap_line("ctrl-alt-shift-cmd-backslash", 10.0, chars),
            vec!["ctrl-alt-s", "hift-cmd-b", "ackslash"]
        );
    }

    #[test]
    fn cfg_6_no_room_still_gives_one_character_per_line() {
        assert_eq!(wrap_line("ab", 0.0, chars), vec!["a", "b"]);
    }

    #[test]
    fn cfg_6_errors_show_lines_and_dot() {
        let refused = ConfigError {
            line: None,
            message: "ctrl-alt-plus: no \"+\" key".into(),
        };
        let state = with_errors(vec![error(3, "bad"), refused]);
        assert_eq!(
            state.lines(Instant::now()),
            vec![
                HEADER.to_string(),
                "sorbo.toml line 3: bad".to_string(),
                "sorbo.toml: ctrl-alt-plus: no \"+\" key".to_string(),
            ]
        );
        assert!(state.dot());
    }

    #[test]
    fn cfg_6_no_errors_hidden_no_dot() {
        let state = with_errors(vec![]);
        assert!(state.lines(Instant::now()).is_empty());
        assert!(!state.dot());
    }

    #[test]
    fn cfg_6_dismiss_hides_but_keeps_dot() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.dismiss();
        assert!(state.lines(Instant::now()).is_empty());
        assert!(state.dot());
    }

    #[test]
    fn cfg_6_same_errors_after_reload_stay_dismissed() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.dismiss();
        state.set_config_errors(vec![error(3, "bad")]);
        assert!(state.lines(Instant::now()).is_empty());
    }

    #[test]
    fn cfg_6_different_errors_show_again() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.dismiss();
        state.set_config_errors(vec![error(4, "worse")]);
        assert_eq!(state.lines(Instant::now()).len(), 2);
    }

    #[test]
    fn cfg_6_fixing_the_file_clears_banner_and_dot() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.set_config_errors(vec![]);
        assert!(state.lines(Instant::now()).is_empty());
        assert!(!state.dot());
    }

    #[test]
    fn cfg_6_dismissal_ends_when_errors_clear() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.dismiss();
        state.set_config_errors(vec![]);
        state.set_config_errors(vec![error(3, "bad")]);
        assert_eq!(state.lines(Instant::now()).len(), 2);
    }

    #[test]
    fn cfg_6_show_again_after_dismiss() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.dismiss();
        state.show_again();
        assert_eq!(state.lines(Instant::now()).len(), 2);
    }

    #[test]
    fn cfg_6_more_than_five_errors_are_summarised() {
        let errors = (1..=7).map(|n| error(n, "bad")).collect();
        let lines = with_errors(errors).lines(Instant::now());
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[5], "sorbo.toml line 5: bad");
        assert_eq!(lines[6], "…and 2 more (see the log)");
    }

    #[test]
    fn cfg_6_runtime_error_expires_after_5s_without_dot() {
        let mut state = BannerState::new("sorbo.toml");
        let now = Instant::now();
        state.runtime_error("no app named \"kitty\" is installed".into(), now);
        assert_eq!(
            state.lines(now + Duration::from_secs(4)),
            vec!["no app named \"kitty\" is installed".to_string()]
        );
        assert!(state.lines(now + Duration::from_secs(5)).is_empty());
        assert!(!state.dot());
    }

    #[test]
    fn cfg_6_runtime_error_shows_even_when_dismissed() {
        let mut state = with_errors(vec![error(3, "bad")]);
        state.dismiss();
        let now = Instant::now();
        state.runtime_error("no app named \"x\" is installed".into(), now);
        assert_eq!(
            state.lines(now),
            vec!["no app named \"x\" is installed".to_string()]
        );
    }

    #[test]
    fn cfg_6_runtime_error_is_added_after_config_errors() {
        let mut state = with_errors(vec![error(3, "bad")]);
        let now = Instant::now();
        state.runtime_error("oops".into(), now);
        assert_eq!(state.lines(now).len(), 3);
        assert_eq!(state.lines(now)[2], "oops");
    }
}
