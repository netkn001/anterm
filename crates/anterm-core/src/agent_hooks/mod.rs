mod runner;
mod strategies;
mod target;
#[cfg(test)]
mod tests;

pub use runner::{ANTERM_ENV_MARKER, run_agent_hook};
pub use target::{
    HookAgent, HookOutcome, HookTarget, HooksState, hooks_state, install_hooks, refresh_hooks,
    refresh_hooks_at_launch, refresh_remote_hooks, uninstall_hooks,
};
