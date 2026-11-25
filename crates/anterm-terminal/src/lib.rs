extern crate gpui_kit as gpui;

pub mod backend;
mod composer;
mod element;
pub mod input;
mod links;
pub mod prompt;
mod remote;
pub mod render;
mod search;
pub mod view;

pub use backend::size::TermSize;
pub use remote::{
    PaneRoute, RemoteTerminal, attach_refused, attach_unanswered,
};
