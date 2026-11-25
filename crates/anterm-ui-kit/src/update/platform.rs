//! Platform-specific installation layout detection, staging, verification,
//! and updater invocation (macOS `.app`, Linux AppImage, Windows Inno/Portable).

use anyhow::{Context as _, Result};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use anterm_core::daemon::install::AssetFetcher as _;
#[cfg(target_os = "windows")]
use anterm_core::daemon::install::updater_contract::{
    CMD_CAPABILITIES, ELEVATION_CAPABILITIES, FLAG_CONFIG_DIR, FLAG_RESULT_FILE,
};
#[cfg(not(target_os = "windows"))]
use anterm_core::daemon::install::updater_contract::{FLAG_CONFIG_DIR, FLAG_RESULT_FILE};

use crate::config::Config;

use super::{DOWNLOAD_VERIFYING, PLAN_VERSION, PendingUpdate, ReleaseAsset, UpdateInstallHint};
#[cfg(target_os = "windows")]
use super::{
    WINDOWS_INNO_INSTALL_MARKER, WINDOWS_PORTABLE_MARKER, WINDOWS_PORTABLE_MARKER_CONTENT,
    localized_update_install_hint,
};

/// The release package this installation can replace itself with. Split from
/// the bare filename so the Windows Inno layout can carry "yes, but the
/// install needs a UAC prompt" alongside it (#504).
#[derive(Debug)]
pub(super) struct PackageOffer {
    pub(super) name: String,
    #[cfg(target_os = "windows")]
    pub(super) needs_elevation: bool,
}

impl PackageOffer {
    #[cfg(any(target_os = "macos", target_os = "linux", test))]
    pub(super) fn plain(name: String) -> Self {
        Self {
            name,
            #[cfg(target_os = "windows")]
            needs_elevation: false,
        }
    }
}

/// The release package this installation can replace itself with, or the
/// reason it cannot.
pub(super) fn package_for_current_install(version: &str) -> Result<PackageOffer, UpdateInstallHint> {
    #[cfg(target_os = "macos")]
    {
        let Some(app) = current_macos_app_bundle() else {
            return Err(UpdateInstallHint::UnsupportedMacos);
        };
        if !is_macos_update_writable(&app) || bundled_updater().is_none() {
            return Err(UpdateInstallHint::UnsupportedMacos);
        }
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            return Err(UpdateInstallHint::UnsupportedMacos);
        };
        return Ok(PackageOffer::plain(format!(
            "anterm-{version}-macos-{arch}.zip"
        )));
    }
    #[cfg(target_os = "linux")]
    {
        let arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            // The release publishes no Linux package for anything else, so
            // there is no filename worth naming.
            return Err(UpdateInstallHint::UnsupportedLinux);
        };
        let appimage = current_appimage();
        // An AppImage can replace itself only if a staging directory fits
        // beside it — the swap is two renames, which need one filesystem —
        // and only if it carries the helper that performs the swap after
        // this process exits. Images from before the helper shipped fail the
        // second check and keep the manual path they always had.
        let can_self_update = appimage.as_deref().is_some_and(|appimage| {
            is_appimage_update_writable(appimage) && bundled_updater().is_some()
        });
        return linux_package_for(version, arch, appimage.is_some(), can_self_update);
    }
    #[cfg(target_os = "windows")]
    {
        let Some(layout) = current_windows_update_layout() else {
            return Err(UpdateInstallHint::UnsupportedWindows);
        };
        let needs_elevation = match windows_layout_updatability(&layout) {
            WindowsUpdatability::Updatable => false,
            WindowsUpdatability::NeedsElevation => true,
            WindowsUpdatability::Unsupported(hint) => return Err(hint),
        };
        if !layout.directory().join("anterm-updater.exe").is_file() {
            return Err(UpdateInstallHint::UnsupportedWindows);
        }
        // UAC is pointed at the *installed* updater, so the elevated chain is
        // only as real as the verbs that binary speaks: one from a release
        // before the verbs existed keeps today's answer — the release page.
        if needs_elevation && !updater_speaks_elevation(layout.directory()) {
            return Err(UpdateInstallHint::WindowsAllUsersInstall);
        }
        return windows_package_for_layout(version, &layout)
            .map(|name| PackageOffer {
                name,
                needs_elevation,
            })
            .ok_or(UpdateInstallHint::UnsupportedWindows);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = version;
        Err(UpdateInstallHint::UnsupportedPlatform)
    }
}

/// Whether the updater installed beside this app speaks the elevation verbs
/// (`elevated-stage` / `install-elevated` / `relaunch-watcher`). Probed by
/// execution rather than version comparison: the capability list is the
/// contract, and a manually downgraded or side-loaded binary answers for
/// itself. One short-lived process per check; an updater that predates the
/// verb exits with its usage error, which is the same "no".
#[cfg(target_os = "windows")]
pub(super) fn updater_speaks_elevation(install_dir: &Path) -> bool {
    let updater = install_dir.join("anterm-updater.exe");
    let mut command = Command::new(&updater);
    command
        .arg(CMD_CAPABILITIES)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let output = anterm_core::proc::hide_console(&mut command).output();
    let Ok(output) = output else { return false };
    output.status.success() && capabilities_cover_elevation(&output.stdout)
}

/// The pure half, so the token matching is testable without spawning a real
/// helper. Mirrors `ELEVATION_CAPABILITIES` in the updater.
#[cfg(target_os = "windows")]
pub(super) fn capabilities_cover_elevation(stdout: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(stdout) else {
        return false;
    };
    let tokens: std::collections::HashSet<&str> = text.lines().map(str::trim).collect();
    ELEVATION_CAPABILITIES
        .iter()
        .all(|capability| tokens.contains(capability))
}

pub(super) fn prepare_update(
    version: &str,
    asset: &ReleaseAsset,
    on_progress: &dyn Fn(u64, Option<u64>) -> ControlFlow<()>,
) -> Result<PreparedUpdate> {
    // Runs off the main thread, so the `Config` global is out of reach.
    let cfg = Config::load();
    let fetcher =
        anterm_core::daemon::install::download::HttpsFetcher::new(cfg.http_proxy.as_deref());
    // Fetched without progress: a few hundred bytes next to a 30 MB package.
    let checksums = fetcher
        .get(&asset.checksums_url)
        .map_err(anyhow::Error::msg)
        .context("downloading checksums.txt")?;
    let archive = fetcher
        .get_cancellable(&asset.url, on_progress)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("downloading {}", asset.name))?;
    DOWNLOAD_VERIFYING.store(true, Ordering::Relaxed);
    #[cfg(target_os = "macos")]
    {
        return prepare_macos_update(version, &asset.name, &archive, &checksums);
    }
    #[cfg(target_os = "windows")]
    {
        return prepare_windows_update(version, &asset.name, &archive, &checksums);
    }
    #[cfg(target_os = "linux")]
    {
        return prepare_linux_update(version, &asset.name, &archive, &checksums);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    anyhow::bail!("automatic installation is not supported on this platform")
}

#[derive(Debug)]
pub(super) struct PreparedUpdate {
    pub(super) updater: PathBuf,
    /// The updater subcommand: `install`, or `install-portable` on Windows.
    pub(super) command: String,
    /// Every argument after the parent pid.
    pub(super) rest: Vec<PathBuf>,
    pub(super) config_dir: Option<PathBuf>,
    pub(super) stage: PathBuf,
    /// Windows, all-users Inno layout: launch through the elevated chain
    /// (#504), not by spawning the staged helper directly.
    pub(super) needs_elevation: bool,
    /// See [`PendingUpdate::expected_sha256`].
    pub(super) expected_sha256: Option<String>,
}

impl PreparedUpdate {
    /// The pid is filled in here rather than baked into `rest`, so a plan
    /// written to disk today still names the right process when a launch
    /// tomorrow runs it.
    pub(super) fn args(&self) -> Vec<PathBuf> {
        let mut args = Vec::with_capacity(self.rest.len() + 2);
        args.push(PathBuf::from(&self.command));
        args.push(std::process::id().to_string().into());
        args.extend(self.rest.iter().cloned());
        args
    }

    pub(super) fn into_pending(self, version: String, apply_on_launch: bool) -> PendingUpdate {
        PendingUpdate {
            version,
            apply_on_launch,
            plan_version: PLAN_VERSION,
            updater: self.updater,
            command: self.command,
            rest: self.rest,
            config_dir: self.config_dir,
            stage: self.stage,
            needs_elevation: self.needs_elevation,
            expected_sha256: self.expected_sha256,
        }
    }

    pub(super) fn launch(&self) -> Result<()> {
        let mut command = Command::new(&self.updater);
        command.args(self.args());
        let outcome = update_outcome_path();
        for arg in updater_tail_args(self.config_dir.as_deref(), outcome.as_deref()) {
            command.arg(arg);
        }
        if let Some(outcome) = &outcome {
            // A result from an earlier attempt has to be gone before this one
            // starts: a helper that dies before writing its own would
            // otherwise be read as having written *that* one.
            let _ = std::fs::remove_file(outcome);
        }
        anterm_core::proc::hide_console(&mut command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&self.stage);
            })
            .context("launching anterm-updater")?;
        Ok(())
    }
}

/// Where the updater records how the install ended; the next launch merges it
/// into the update state (`absorb_update_outcome_at_launch`).
pub(super) fn update_outcome_path() -> Option<PathBuf> {
    crate::config::config_path(anterm_core::daemon::install::outcome::OUTCOME_FILE_NAME)
}

/// The file the privileged first stage writes its pid into so the de-elevated
/// watcher can tell the chain apart from a UAC prompt nobody has answered
/// yet. It cannot live in the privileged staging directory (the watcher's
/// token may not read there), so it sits next to the outcome file: an
/// administrator can write the user's config directory under every elevation
/// shape, including over-the-shoulder (#504).
#[cfg(target_os = "windows")]
pub(super) fn update_elevation_status_path() -> Option<PathBuf> {
    crate::config::config_path("update-elevation.status")
}

/// The named options after the positional arguments. Everything the updater
/// must know about this process's configuration crosses as arguments, never
/// the environment: on Windows an elevated (UAC) child does not inherit the
/// spawning process's environment, so a `ANTERM_CONFIG_DIR` set there would
/// fall back to the administrator's config directory exactly in the case
/// that needs the caller's (#504). The updater re-exports the variable for
/// the children it spawns itself.
pub(super) fn updater_tail_args(config_dir: Option<&Path>, outcome: Option<&Path>) -> Vec<std::ffi::OsString> {
    let mut args = Vec::new();
    if let Some(dir) = config_dir {
        args.push(std::ffi::OsString::from(FLAG_CONFIG_DIR));
        args.push(dir.as_os_str().to_os_string());
    }
    if let Some(outcome) = outcome {
        args.push(std::ffi::OsString::from(FLAG_RESULT_FILE));
        args.push(outcome.as_os_str().to_os_string());
    }
    args
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn update_staging_dir(parent: &Path) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(".anterm-update-")
        .tempdir_in(parent)
        .context("creating update staging directory")
}

#[cfg(target_os = "windows")]
pub(super) fn system_update_staging_dir() -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix("anterm-update-")
        .tempdir()
        .context("creating the Windows update staging directory")
}

pub(super) fn write_staged_asset(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(target_os = "macos")]
pub(super) fn prepare_macos_update(
    version: &str,
    asset_name: &str,
    archive: &[u8],
    checksums: &[u8],
) -> Result<PreparedUpdate> {
    let current =
        current_macos_app_bundle().context("anterm is not running from an application bundle")?;
    let parent = current
        .parent()
        .context("anterm.app has no parent directory")?;
    let updater = bundled_updater().context("anterm-updater is not bundled with this app")?;
    let staging = update_staging_dir(parent)?;
    let dir = staging.path().to_path_buf();
    let archive = write_staged_asset(&dir, asset_name, archive)?;
    let checksums = write_staged_asset(&dir, "checksums.txt", checksums)?;
    run_updater(
        &updater,
        [
            PathBuf::from("verify"),
            current.clone(),
            archive.clone(),
            checksums.clone(),
            PathBuf::from(asset_name),
            dir.clone(),
            PathBuf::from(version),
        ],
    )?;
    let log =
        crate::config::config_path("update.log").unwrap_or_else(|| dir.join("update.log"));
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).context("creating the update log directory")?;
    }
    let dir = staging.keep();
    Ok(PreparedUpdate {
        updater,
        command: "install".to_string(),
        rest: vec![
            current,
            archive,
            checksums,
            PathBuf::from(asset_name),
            dir.clone(),
            PathBuf::from(version),
            log,
        ],
        config_dir: crate::config::config_dir_path(),
        stage: dir,
        needs_elevation: false,
        expected_sha256: None,
    })
}

/// Stages a downloaded AppImage beside the installed one and has the bundled
/// updater verify it while this process is still around to show a failure.
///
/// Staging lives in the image's own directory for the same reason macOS
/// stages beside the bundle: the swap is two renames, and renames only stay
/// atomic on one filesystem. The helper that performs them is *copied* into
/// staging rather than run from the image — the image is a FUSE mount the
/// runtime tears down when the GUI exits, which is precisely the moment the
/// installer starts working.
#[cfg(target_os = "linux")]
pub(super) fn prepare_linux_update(
    version: &str,
    asset_name: &str,
    archive: &[u8],
    checksums: &[u8],
) -> Result<PreparedUpdate> {
    let current = current_appimage().context("anterm is not running from an AppImage")?;
    let parent = current
        .parent()
        .context("the AppImage has no parent directory")?;
    let bundled = bundled_updater().context("anterm-updater is not bundled with this image")?;
    let staging = update_staging_dir(parent)?;
    let dir = staging.path().to_path_buf();
    let archive = write_staged_asset(&dir, asset_name, archive)?;
    let checksums = write_staged_asset(&dir, "checksums.txt", checksums)?;
    run_updater(
        &bundled,
        [
            PathBuf::from("verify"),
            archive.clone(),
            checksums.clone(),
            PathBuf::from(asset_name),
            dir.clone(),
            PathBuf::from(version),
        ],
    )?;
    let updater = dir.join("anterm-updater");
    std::fs::copy(&bundled, &updater)
        .with_context(|| format!("copying the updater to {}", updater.display()))?;
    let log =
        crate::config::config_path("update.log").unwrap_or_else(|| dir.join("update.log"));
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).context("creating the update log directory")?;
    }
    let dir = staging.keep();
    Ok(PreparedUpdate {
        updater,
        command: "install".to_string(),
        rest: vec![
            current,
            archive,
            checksums,
            PathBuf::from(asset_name),
            dir.clone(),
            PathBuf::from(version),
            log,
        ],
        config_dir: crate::config::config_dir_path(),
        stage: dir,
        needs_elevation: false,
        expected_sha256: None,
    })
}

#[cfg(target_os = "windows")]
pub(super) fn prepare_windows_update(
    version: &str,
    asset_name: &str,
    package: &[u8],
    checksums: &[u8],
) -> Result<PreparedUpdate> {
    let layout = current_windows_update_layout()
        .context("anterm is not running from a recognized Windows installation")?;
    // Re-checked here rather than trusting the check that produced the offer:
    // an installation can be relocated, or its privileges changed, between the
    // update check and the user pressing the button.
    let updatability = windows_layout_updatability(&layout);
    let needs_elevation = match updatability {
        WindowsUpdatability::Updatable => false,
        WindowsUpdatability::NeedsElevation => true,
        // This error surfaces in Settings as the update failure — the moment
        // the user most needs to understand it — so it is the localized hint,
        // not the English one meant for logs (#602).
        WindowsUpdatability::Unsupported(hint) => {
            anyhow::bail!("{}", localized_update_install_hint(&hint))
        }
    };
    let install_dir = layout.directory().to_path_buf();
    let bundled = bundled_updater().context("anterm-updater.exe is not bundled with this app")?;

    // The digest the elevated chain trusts, taken from the downloaded
    // manifest rather than the staged copy of it: the staged file lands in a
    // user-writable directory, so it cannot anchor its own verification.
    // Computed for both Inno modes — a plan's elevation answer is re-derived
    // at launch, and the digest costs one manifest parse.
    let expected_sha256 = if matches!(layout, WindowsUpdateLayout::Inno(_)) {
        let manifest =
            std::str::from_utf8(checksums).context("checksums.txt is not valid UTF-8")?;
        Some(
            anterm_core::daemon::install::checksums::expected_digest(manifest, asset_name)
                .map(|digest| anterm_core::daemon::install::checksums::hex(&digest))
                .context("reading the staged package's digest from checksums.txt")?,
        )
    } else {
        None
    };

    let staging = system_update_staging_dir()?;
    let dir = staging.path().to_path_buf();
    let package = write_staged_asset(&dir, asset_name, package)?;
    let checksums = write_staged_asset(&dir, "checksums.txt", checksums)?;

    // Verification runs before the GUI commits to quitting. Both Windows
    // update modes repeat their archive checks after the parent exits.
    let install_command = match &layout {
        WindowsUpdateLayout::Inno(_) => {
            run_updater(
                &bundled,
                [
                    PathBuf::from("verify"),
                    package.clone(),
                    checksums.clone(),
                    PathBuf::from(asset_name),
                    PathBuf::from(version),
                ],
            )?;
            "install"
        }
        WindowsUpdateLayout::Portable(_) => {
            run_updater(
                &bundled,
                [
                    PathBuf::from("verify-portable"),
                    package.clone(),
                    checksums.clone(),
                    PathBuf::from(asset_name),
                    PathBuf::from(version),
                    dir.clone(),
                ],
            )?;
            "install-portable"
        }
    };

    // Windows locks a running executable. Run a private copy from the staging
    // directory so Inno can replace the bundled helper in the installation.
    let updater = dir.join("anterm-updater.exe");
    std::fs::copy(&bundled, &updater)
        .with_context(|| format!("copying the Windows updater to {}", updater.display()))?;

    let log =
        crate::config::config_path("update.log").unwrap_or_else(|| dir.join("update.log"));
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).context("creating the update log directory")?;
    }
    let dir = staging.keep();
    Ok(PreparedUpdate {
        updater,
        command: install_command.to_string(),
        rest: vec![
            package,
            checksums,
            PathBuf::from(asset_name),
            install_dir,
            PathBuf::from(version),
            log,
            dir.clone(),
        ],
        config_dir: crate::config::config_dir_path(),
        stage: dir,
        needs_elevation,
        expected_sha256,
    })
}

#[cfg(target_os = "macos")]
pub(super) fn current_macos_app_bundle() -> Option<PathBuf> {
    std::env::current_exe().ok()?.ancestors().find_map(|path| {
        (path.extension().and_then(|ext| ext.to_str()) == Some("app")).then(|| path.to_path_buf())
    })
}

#[cfg(target_os = "macos")]
pub(super) fn is_macos_update_writable(app: &Path) -> bool {
    app.parent().is_some_and(can_stage_replacement_in)
}

#[cfg(target_os = "macos")]
pub(super) fn bundled_updater() -> Option<PathBuf> {
    let updater = current_macos_app_bundle()?.join("Contents/MacOS/anterm-updater");
    updater.is_file().then_some(updater)
}

#[cfg(target_os = "windows")]
pub(super) fn bundled_updater() -> Option<PathBuf> {
    let updater = current_windows_update_layout()?
        .directory()
        .join("anterm-updater.exe");
    updater.is_file().then_some(updater)
}

/// The updater shipped inside the mounted image, beside this executable at
/// `usr/bin`. Resolved through `current_exe` rather than `$APPDIR` so a
/// stale or hand-set variable cannot point the update machinery at a binary
/// that is not the one this process actually runs beside.
#[cfg(target_os = "linux")]
pub(super) fn bundled_updater() -> Option<PathBuf> {
    let updater = std::env::current_exe().ok()?.parent()?.join("anterm-updater");
    updater.is_file().then_some(updater)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub(super) fn bundled_updater() -> Option<PathBuf> {
    None
}

/// The image this process is running from. The AppImage runtime exports
/// `$APPIMAGE`, pointing at the file it mounted; nothing else identifies
/// which of the Linux packages is installed. Required to name an existing
/// absolute path — the variable outlives moves and deletions, and every
/// answer given here is a file the updater will later rename.
#[cfg(target_os = "linux")]
pub(super) fn current_appimage() -> Option<PathBuf> {
    let appimage = PathBuf::from(std::env::var_os("APPIMAGE")?);
    (appimage.is_absolute() && appimage.is_file()).then_some(appimage)
}

#[cfg(target_os = "linux")]
pub(super) fn is_appimage_update_writable(appimage: &Path) -> bool {
    appimage.parent().is_some_and(can_stage_replacement_in)
}

/// The Linux answer, split from the probing so the policy is testable
/// without an AppImage runtime setting variables. Only an AppImage that can
/// stage and swap itself is offered an install; every other shape keeps the
/// named-package hint it always had — a tarball unpacks wherever the user
/// chose, a distro package belongs to its package manager, and guessing at
/// either is exactly what `package_for_current_install` must never do.
#[cfg(target_os = "linux")]
pub(super) fn linux_package_for(
    version: &str,
    arch: &str,
    is_appimage: bool,
    can_self_update: bool,
) -> Result<PackageOffer, UpdateInstallHint> {
    if !is_appimage {
        return Err(UpdateInstallHint::LinuxManualPackage(format!(
            "anterm-{version}-linux-{arch}.tar.gz"
        )));
    }
    let name = format!("anterm-{version}-linux-{arch}.AppImage");
    if can_self_update {
        Ok(PackageOffer::plain(name))
    } else {
        Err(UpdateInstallHint::LinuxManualPackage(name))
    }
}

#[cfg(target_os = "windows")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WindowsUpdateLayout {
    Inno(PathBuf),
    Portable(PathBuf),
}

#[cfg(target_os = "windows")]
impl WindowsUpdateLayout {
    fn directory(&self) -> &Path {
        match self {
            Self::Inno(directory) | Self::Portable(directory) => directory,
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn windows_package_for_layout(version: &str, layout: &WindowsUpdateLayout) -> Option<String> {
    // Labelled like the macOS packages (`arm64`), not like the server assets
    // (`aarch64`): these are the desktop app's names. An x64 build running
    // under emulation on ARM64 keeps taking x64 packages — switching it to
    // the native build is the user's reinstall, not something an update does.
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        return None;
    };
    Some(match layout {
        WindowsUpdateLayout::Inno(_) => format!("anterm-{version}-windows-{arch}-setup.exe"),
        WindowsUpdateLayout::Portable(_) => format!("anterm-{version}-windows-{arch}.zip"),
    })
}

#[cfg(target_os = "windows")]
pub(super) fn current_windows_update_layout() -> Option<WindowsUpdateLayout> {
    let executable = std::env::current_exe().ok()?;
    windows_update_layout_for(&executable)
}

#[cfg(target_os = "windows")]
pub(super) fn windows_update_layout_for(executable: &Path) -> Option<WindowsUpdateLayout> {
    let directory = executable.parent()?;
    if directory.join(WINDOWS_INNO_INSTALL_MARKER).is_file() {
        return Some(WindowsUpdateLayout::Inno(directory.to_path_buf()));
    }
    let marker = std::fs::read(directory.join(WINDOWS_PORTABLE_MARKER)).ok()?;
    (marker == WINDOWS_PORTABLE_MARKER_CONTENT)
        .then(|| WindowsUpdateLayout::Portable(directory.to_path_buf()))
}

#[cfg(target_os = "windows")]
pub(super) fn windows_directory_is_writable(directory: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".anterm-update-write-test-")
        .tempfile_in(directory)
        .is_ok()
}

/// What this Windows installation layout means for an in-place update.
#[cfg(target_os = "windows")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WindowsUpdatability {
    /// A per-user Inno install, or a writable portable directory: the
    /// updater runs as the signed-in user, no prompt involved.
    Updatable,
    /// An Inno install replacing it needs administrator rights for (#504):
    /// the elevated chain handles it — one announced UAC prompt, and the app
    /// never runs elevated itself.
    NeedsElevation,
    Unsupported(UpdateInstallHint),
}

/// Sorts the Windows installation layouts by what replacing them takes,
/// before anything is downloaded.
#[cfg(target_os = "windows")]
pub(super) fn windows_layout_updatability(layout: &WindowsUpdateLayout) -> WindowsUpdatability {
    match layout {
        WindowsUpdateLayout::Inno(directory) => {
            if windows_inno_needs_elevation(directory) {
                return WindowsUpdatability::NeedsElevation;
            }
            WindowsUpdatability::Updatable
        }
        WindowsUpdateLayout::Portable(directory) => {
            if !windows_directory_is_writable(directory) {
                return WindowsUpdatability::Unsupported(UpdateInstallHint::UnsupportedWindows);
            }
            WindowsUpdatability::Updatable
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn windows_inno_needs_elevation(install_dir: &Path) -> bool {
    windows_inno_needs_elevation_for(
        windows_all_users_install_path().as_deref(),
        install_dir,
        windows_directory_is_writable(install_dir),
    )
}

/// Whether replacing this Inno installation would need administrator rights.
///
/// The updater runs the release Setup silently, as the signed-in user, from a
/// private staging directory. That is only correct for a per-user install.
/// Two independent signals, because either alone misreads a real machine:
///
///   * An all-users install records its state under `HKLM`. A silent Setup
///     launched without elevation resolves `{autopf}` to
///     `%LocalAppData%\Programs`, never sees that state, and installs a
///     *second* copy while the real installation goes untouched — or Inno
///     re-launches itself elevated and the user gets a bare UAC prompt for an
///     unsigned executable in `%TEMP%`, seconds after the GUI vanished.
///     Neither outcome is one anterm should produce on its own initiative.
///   * A directory this process cannot write is one Setup cannot write
///     either, whatever the registry says. This also catches an installation
///     whose uninstall entry was pruned, relocated, or written by a different
///     user account.
///
/// Pure so the decision is unit-tested without touching the registry or
/// `C:\Program Files`.
#[cfg(target_os = "windows")]
pub(super) fn windows_inno_needs_elevation_for(
    all_users_app_path: Option<&Path>,
    install_dir: &Path,
    writable: bool,
) -> bool {
    if all_users_app_path.is_some_and(|path| same_windows_directory(path, install_dir)) {
        return true;
    }
    !writable
}

/// Compares two Windows directory paths the way the filesystem does: without
/// regard to case, and without letting a trailing separator make
/// `C:\Program Files\anterm\` a different place from `C:\Program Files\anterm`.
/// Deliberately textual — `canonicalize` would hit the disk and answers
/// `\\?\`-prefixed, which is not what the registry stores.
#[cfg(target_os = "windows")]
pub(super) fn same_windows_directory(left: &Path, right: &Path) -> bool {
    fn normalize(path: &Path) -> Option<String> {
        let text = path.to_str()?.trim_end_matches(['\\', '/']);
        (!text.is_empty()).then(|| text.to_lowercase())
    }
    match (normalize(left), normalize(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// The `{app}` directory of an all-users anterm installation, read from the
/// machine hive. `AppId` is frozen in `windows-installer.iss` for exactly this
/// kind of lookup, and Inno stamps the resolved install directory into
/// `Inno Setup: App Path`. Absent for a per-user install, whose uninstall
/// entry lives under `HKCU` instead.
#[cfg(target_os = "windows")]
pub(super) fn windows_all_users_install_path() -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    const UNINSTALL_KEY: &str = concat!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\",
        r"{9A3F6C1E-4B7D-4E2A-8C5F-D01B92E64A37}_is1"
    );
    const APP_PATH_VALUE: &str = "Inno Setup: App Path";

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            // SAFETY: only constructed from a successful `RegOpenKeyExW`, and
            // owns exactly one handle.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let path = wide(UNINSTALL_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and live for the call; `key` is a valid
    // out-parameter, wrapped only when the call reports success. The anterm
    // installer is x64-only, so the native 64-bit view is the only one its
    // uninstall entry can appear in.
    let code = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) };
    if code != ERROR_SUCCESS {
        return None;
    }
    let key = RegistryKey(key);

    let name = wide(APP_PATH_VALUE);
    let mut kind = 0u32;
    let mut bytes = 0u32;
    // SAFETY: the key is live, the value name is NUL-terminated, and the
    // type/size out-parameters are valid; a null data pointer asks for the
    // size only.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if code != ERROR_SUCCESS || kind != REG_SZ || bytes == 0 || !bytes.is_multiple_of(2) {
        return None;
    }

    let mut value = vec![0u16; bytes as usize / 2];
    // SAFETY: `value` is sized from the query above and stays live; Win32 is
    // told its capacity in bytes through `bytes`.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if code != ERROR_SUCCESS {
        return None;
    }
    value.truncate(bytes as usize / 2);
    while value.last() == Some(&0) {
        value.pop();
    }
    (!value.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&value)))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) fn can_stage_replacement_in(dir: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".anterm-update-write-test-")
        .tempfile_in(dir)
        .is_ok()
}

pub(super) fn run_updater(updater: &Path, args: impl IntoIterator<Item = PathBuf>) -> Result<()> {
    let mut command = Command::new(updater);
    command.args(args);
    let output = anterm_core::proc::hide_console(&mut command)
        .output()
        .context("running anterm-updater verification")?;
    if !output.status.success() {
        anyhow::bail!(
            "anterm-updater verification failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(())
}

