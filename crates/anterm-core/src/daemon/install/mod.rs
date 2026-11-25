#[cfg(test)]
use std::io;
#[cfg(test)]
use std::sync::Arc;

pub mod asset;
pub use anterm_protocol::updater::checksums;
#[cfg(feature = "remote-install")]
pub mod download;
mod installer;
mod launch;
pub use anterm_protocol::updater::outcome;
#[cfg(feature = "remote-install")]
pub mod proxy;
pub mod ssh_ops;
pub use anterm_protocol::updater::contract as updater_contract;
pub mod windows_host;
pub mod wsl;

pub use asset::{RemotePaths, RemotePlatform, UnsupportedTarget};
pub use checksums::ChecksumError;
pub use installer::{
    AssetFetcher, BundledOrRelease, DenyInstall, ExecOutput, InstallConfirm, InstallDecision,
    InstallError, InstallPhase, InstallProgress, InstallReport, InstallRequest, Installer,
    LoadedBinary, MismatchedRemoteDaemon, PROTOCOL_FLAG, ProvedServer, ReleaseDownload, RemoteOps,
    RemoteProtocol, RemoteStat, ServerBinarySource, SilentProgress, client_version,
    ensure_remote_server, ensure_remote_server_labeled, forget_remote_mismatch,
    forget_remote_server, install_confirm, install_progress, record_remote_mismatches,
    replace_remote_server, restart_remote_daemon, set_install_confirm, set_install_progress,
    take_mismatched_remote_daemons, update_remote_server, with_install_confirm,
    with_install_progress, with_mismatch_sink,
};
pub use launch::server_stdio_command;
pub(crate) use launch::shell_quote;

#[cfg(test)]
use installer::{proved_or_prove, while_changing_the_server};
#[cfg(test)]
use launch::{
    QUICK_PROVE_MARK, RUNNING_EXE_COMMAND, StartupLog, TERMINATE_RUNNING_COMMAND, launch_command,
    launch_script, under_sh, unique_temp,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod windows_tests;
