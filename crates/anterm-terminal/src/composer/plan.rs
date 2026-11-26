//! Re-exports the shared pure-Rust agent submission plan & `@` mention logic
//! from [`anterm_client_core::composer`] so desktop and mobile share one source
//! of truth.

#[allow(unused_imports)]
pub(crate) use anterm_client_core::composer::{
    KEY_GAP, OPENCODE_LIST_WAIT, Piece, SETTLE, SETTLE_AFTER_PASTE_SLOW, Step, clear_input,
    compose_message, interrupt_keys, mention, mention_pieces, opencode_mention_plan,
    submit_message, submit_plan,
};
