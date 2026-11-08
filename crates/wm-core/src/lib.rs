//! Pure window-manager logic: events in, commands out. No macOS dependencies.

// Platform code (and its `unsafe`) lives in wm-macos; this crate stays safe Rust.
#![forbid(unsafe_code)]

pub mod config;
pub mod event;
pub mod geometry;
pub mod ids;
pub mod keymap;
pub mod model;
pub mod platform;
pub mod testing;
pub mod tree;
mod wm;

pub use config::*;
pub use event::*;
pub use geometry::*;
pub use ids::*;
pub use keymap::*;
pub use model::*;
pub use platform::{Platform, run};
pub use tree::{Gaps, Node, Split};
pub use wm::Wm;
