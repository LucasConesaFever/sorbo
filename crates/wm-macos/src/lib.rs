//! The macOS side of the window manager: watches macOS, turns what happens into
//! `wm_core::Event`s, and carries out `wm_core::Command`s.
//!
//! Every decision lives in a pure module that compiles and tests on any OS. Only the
//! `macos` module talks to the operating system.

pub mod apps;
pub mod banner_state;
pub mod bar;
pub mod classify;
pub mod config_load;
pub mod confirm;
pub mod echo;
pub mod focus_guard;
pub mod guard;
pub mod hide;
pub mod keys;
pub mod limits;
pub mod logging;
pub mod lost;
pub mod monitors;
pub mod place;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::logfile::{init_log_file, log_line};
#[cfg(target_os = "macos")]
pub use macos::{MacPlatform, accessibility_trusted};

/// Writes a log line to stderr (the log file once `init_log_file` was called), prefixed
/// with the local time: `[2026-11-08 14:03:27.512] message`.
#[macro_export]
macro_rules! wm_log {
    ($($arg:tt)*) => {
        $crate::log_line(format_args!($($arg)*))
    };
}
