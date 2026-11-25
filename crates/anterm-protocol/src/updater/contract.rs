//! Shared CLI protocol constants between the GUI auto-updater (`anterm::core::update`)
//! and the standalone `anterm-updater` helper binary.
//!
//! Keeping these tokens in one shared module ensures the spawning GUI and the
//! updater binary cannot silently drift on verb names, named flags, portable
//! install markers, or UAC elevation capability names.

/// Updater subcommands.
pub const CMD_VERIFY: &str = "verify";
pub const CMD_INSTALL: &str = "install";
pub const CMD_VERIFY_PORTABLE: &str = "verify-portable";
pub const CMD_INSTALL_PORTABLE: &str = "install-portable";
pub const CMD_CLEANUP: &str = "cleanup";
pub const CMD_CAPABILITIES: &str = "capabilities";
pub const CMD_ELEVATED_STAGE: &str = "elevated-stage";
pub const CMD_INSTALL_ELEVATED: &str = "install-elevated";
pub const CMD_RELAUNCH_WATCHER: &str = "relaunch-watcher";

/// Named tail flags shared by the GUI and updater.
pub const FLAG_CONFIG_DIR: &str = "--config-dir";
pub const FLAG_RESULT_FILE: &str = "--result-file";
pub const FLAG_EXPECTED_SHA256: &str = "--expected-sha256";
pub const FLAG_STATUS_FILE: &str = "--status-file";
pub const FLAG_APP_PATH: &str = "--app-path";
pub const FLAG_LOG: &str = "--log";
pub const FLAG_EXPECTED_VERSION: &str = "--expected-version";
pub const FLAG_GUI_PID: &str = "--gui-pid";

/// Capability tokens printed by `anterm-updater capabilities` (one per line)
/// and checked by the GUI before triggering a UAC elevation prompt.
pub const ELEVATION_CAPABILITIES: [&str; 3] = [
    CMD_ELEVATED_STAGE,
    CMD_INSTALL_ELEVATED,
    CMD_RELAUNCH_WATCHER,
];

/// Marker file placed inside a Windows portable installation directory.
pub const PORTABLE_MARKER: &str = ".anterm-portable";
/// Expected payload of [`PORTABLE_MARKER`].
pub const PORTABLE_MARKER_CONTENT: &[u8] = b"portable-v1";
/// Prefix of temporary backup directories created during portable replacement.
pub const PORTABLE_BACKUP_PREFIX: &str = ".anterm-update-backup-";
/// Sentinel inside a portable backup directory indicating an interrupted swap.
pub const PORTABLE_BACKUP_INCOMPLETE: &str = ".anterm-replace-incomplete";
