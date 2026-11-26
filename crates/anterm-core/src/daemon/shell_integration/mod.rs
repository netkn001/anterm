#[cfg_attr(not(feature = "local-daemon"), allow(dead_code))]
mod local;
pub mod remote;
#[cfg(all(test, feature = "local-daemon"))]
mod tests;

const ZSH_INTEGRATION: &str = include_str!("scripts/zsh.zsh");
const FISH_INTEGRATION: &str = include_str!("scripts/fish.fish");
const BASH_INTEGRATION: &str = include_str!("scripts/bash.bash");
const POWERSHELL_INTEGRATION: &str = include_str!("scripts/powershell.ps1");
#[cfg_attr(not(feature = "local-daemon"), allow(dead_code))]
const NUSHELL_INTEGRATION: &str = include_str!("scripts/nushell.nu");

#[cfg(feature = "local-daemon")]
pub use local::{Injection, setup};
#[cfg(feature = "local-daemon")]
pub(crate) use local::{is_our_zdotdir, wsl_distro};
