//! Shell prompt UX: inline editor, completions, command signatures, history,
//! reverse search, and prompt reflow.

pub mod cmd_editor;
pub mod command_cursor;
pub mod completion;
pub mod fuzzy;
pub mod generator;
pub mod history;
pub mod parked_cursor;
pub mod prompt_reflow;
pub mod reverse_search;
pub mod signature;
pub mod store;
