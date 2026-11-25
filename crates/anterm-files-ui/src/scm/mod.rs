//! Source control: standalone [`ScmPanelView`] entity, its virtualized file
//! rows, commit box, detail view, and commit graph.

pub mod actions;
pub mod detail;
pub mod graph;
pub mod panel;
pub mod path;
pub mod state;
pub mod status;
pub mod tree;

pub use actions::ScmIntent;
pub use state::{ScmContext, ScmEvent, ScmPanelView};
