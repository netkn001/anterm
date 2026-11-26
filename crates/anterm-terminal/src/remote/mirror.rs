//! Re-exports the framework-free atomic terminal state mirror from
//! [`anterm_client_core::mirror`] so desktop and mobile share one implementation.

pub use anterm_client_core::mirror::{EventReceiver, TermStateMirror};
