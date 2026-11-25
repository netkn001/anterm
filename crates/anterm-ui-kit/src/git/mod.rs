//! GUI Git/SCM data caches, repository status probing, and diff re-exports.
//!
//! Relocated out of `crate::terminal` because 95%+ of callers are UI panels
//! (SCM, Diff Overlay, File Tree, Code Editor gutter, Sidebar, Switcher).

pub mod data;
pub mod diff;
pub mod status;
