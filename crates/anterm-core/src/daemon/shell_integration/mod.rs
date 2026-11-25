mod local;
pub mod remote;
#[cfg(test)]
mod tests;

const ZSH_INTEGRATION: &str = include_str!("scripts/zsh.zsh");
const FISH_INTEGRATION: &str = include_str!("scripts/fish.fish");
const BASH_INTEGRATION: &str = include_str!("scripts/bash.bash");
const POWERSHELL_INTEGRATION: &str = include_str!("scripts/powershell.ps1");
const NUSHELL_INTEGRATION: &str = include_str!("scripts/nushell.nu");

pub use local::{Injection, setup};
pub(crate) use local::{is_our_zdotdir, wsl_distro};
