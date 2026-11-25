#[cfg(target_os = "windows")]
use anyhow::Context as _;
use anyhow::Result;
use gpui::{AnyWindowHandle, App, AsyncApp, Global};
use smol::future::FutureExt as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::config::Config;

mod github;
mod platform;
mod ui;
#[cfg(test)]
mod tests;

use github::*;
use platform::*;
pub use ui::{localized_update_install_hint, localized_update_phase};
pub use ui::open_releases_page;
use ui::prompt_update;
#[cfg(target_os = "windows")]
use anterm_core::daemon::install::updater_contract::{
    PORTABLE_BACKUP_INCOMPLETE as WINDOWS_PORTABLE_BACKUP_INCOMPLETE,
    PORTABLE_BACKUP_PREFIX as WINDOWS_PORTABLE_BACKUP_PREFIX,
    PORTABLE_MARKER as WINDOWS_PORTABLE_MARKER,
    PORTABLE_MARKER_CONTENT as WINDOWS_PORTABLE_MARKER_CONTENT,
};

const REPO: &str = "netkn001/anterm";

/// The rolling prerelease the Nightly channel follows. Force-moved to a new
/// commit every night, which is exactly why it cannot double as a version.
const NIGHTLY_TAG: &str = "nightly";

/// Published beside the nightly packages so the version is stated rather than
/// inferred. See `resolve_version`.
const NIGHTLY_MANIFEST: &str = "nightly.json";

pub const RELEASES_URL: &str = "https://github.com/netkn001/anterm/releases/latest";

/// The nightly release's own page. Unlike Stable's, this URL is stable across
/// nights — the tag stays put even as the commit under it moves. Spelled out
/// rather than built from `NIGHTLY_TAG`, which `concat!` cannot take; the tail
/// is asserted against it in `each_channel_reads_its_own_feed` instead.
pub const NIGHTLY_RELEASE_URL: &str = "https://github.com/netkn001/anterm/releases/tag/nightly";

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// A terminal workbench is left open for weeks, so a launch-only check reaches
/// only the people who restart anyway — the ones who would have found the
/// update themselves.
const RECHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long "Later" silences one version for. It has to be a real delay rather
/// than the old behaviour (prompted once, then silent forever), or declining an
/// update once removes it from the user's world permanently.
const REMIND_LATER: Duration = Duration::from_secs(3 * 24 * 60 * 60);

/// Staging directories older than this belong to a run that died before its
/// updater could clean up — a download the user cancelled by quitting, or a
/// crash. On macOS they sit in /Applications, so nobody finds them by accident.
const STAGE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Set by `cancel_download`, read by the transfer's progress callback. One
/// download runs at a time (`spawn_download` refuses to start a second), so a
/// single flag is the whole mechanism.
static DOWNLOAD_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Bumped by `switch_channel`. A download carries the value it started under,
/// so a package prepared for the feed the user just left is thrown away instead
/// of being staged. `DOWNLOAD_CANCELLED` handles the common case — a transfer
/// still reading bytes — but stops being observed once the download is through
/// and the work moves on to hashing and unpacking, and that tail is exactly
/// long enough for a switch to land inside it.
static CHANNEL_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Whether the user has asked to install as soon as the package is ready. Set
/// by pressing the install button while a background download is still going,
/// which is the common case now that checking starts one on its own.
static INSTALL_WHEN_READY: AtomicBool = AtomicBool::new(false);

/// Whether the staged package should be applied by the next launch without
/// asking again.
static APPLY_ON_LAUNCH: AtomicBool = AtomicBool::new(false);

/// Byte counters the download thread publishes and the UI samples. Cheaper and
/// less fussy than a channel for something the user reads a few times a second.
static DOWNLOAD_RECEIVED: AtomicU64 = AtomicU64::new(0);
/// Zero when the server sent no usable `content-length`.
static DOWNLOAD_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Set once the bytes are in and the hashing, unpacking and signature checks
/// start. That work has no progress to report, and a percentage frozen at 100
/// is indistinguishable from a hang.
static DOWNLOAD_VERIFYING: AtomicBool = AtomicBool::new(false);

const PROGRESS_TICK: Duration = Duration::from_millis(120);

#[cfg(target_os = "windows")]
const WINDOWS_INNO_INSTALL_MARKER: &str = ".anterm-inno-install";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: String,
    pub installable: bool,
    pub install_hint: Option<UpdateInstallHint>,
    /// Windows, all-users Inno layout: installable, but the install raises
    /// one UAC prompt (#504). The dialog says so up front — a prompt the
    /// user was told about is consent; one that appears uninvited is not.
    #[cfg(target_os = "windows")]
    pub needs_elevation: bool,
    asset: Option<ReleaseAsset>,
}

impl AvailableUpdate {
    /// Whether installing this package raises a UAC prompt. A method rather
    /// than the field so dialog code stays cross-platform: no other layout
    /// ever elevates.
    #[cfg(target_os = "windows")]
    fn needs_elevation(&self) -> bool {
        self.needs_elevation
    }

    #[cfg(not(target_os = "windows"))]
    fn needs_elevation(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateInstallHint {
    #[cfg(target_os = "macos")]
    UnsupportedMacos,
    #[cfg(target_os = "linux")]
    UnsupportedLinux,
    /// The Linux shapes that still update by hand — a tarball install, or an
    /// AppImage that cannot replace itself (read-only directory, no bundled
    /// helper). "Use the release page" would leave the user to work out which
    /// of five files is theirs; this names it.
    #[cfg(target_os = "linux")]
    LinuxManualPackage(String),
    #[cfg(target_os = "windows")]
    UnsupportedWindows,
    #[cfg(target_os = "windows")]
    WindowsAllUsersInstall,
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    UnsupportedPlatform,
    MissingPackage(String),
    MissingChecksums,
}

impl UpdateInstallHint {
    /// The reason in plain English, pinned by tests. Anything a user reads
    /// goes through `localized_update_install_hint` instead (#602).
    #[cfg(all(test, target_os = "windows"))]
    fn english(&self) -> String {
        match self {
            #[cfg(target_os = "macos")]
            Self::UnsupportedMacos => "This copy is not running from a writable anterm.app bundle, so replacing it would be unsafe. Move anterm to Applications or another writable folder, or open the release page to install the update.".to_string(),
            #[cfg(target_os = "linux")]
            Self::UnsupportedLinux => "The release has no Linux package for this architecture. Build from source or use your package manager.".to_string(),
            #[cfg(target_os = "linux")]
            Self::LinuxManualPackage(name) => format!(
                "Linux installations are updated by hand. Download {name} from the release page, or use your package manager."
            ),
            #[cfg(target_os = "windows")]
            Self::UnsupportedWindows => "Automatic Windows updates are available for recognized Inno Setup and portable ZIP installations. This copy is missing a valid installation marker, updater, or writable portable directory, so open the release page to update it manually.".to_string(),
            #[cfg(target_os = "windows")]
            Self::WindowsAllUsersInstall => "anterm is installed for all users, which needs administrator rights to replace. anterm will not raise an elevation prompt on its own behalf, so open the release page and run the installer yourself to update it.".to_string(),
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            Self::UnsupportedPlatform => "Automatic installation is not available on this platform. Open the release page.".to_string(),
            Self::MissingPackage(name) => format!(
                "The release has no {name} package for this installation. Open the release page to choose another package."
            ),
            Self::MissingChecksums => "The release has no checksums.txt, so anterm refuses to install it automatically.".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading {
        received: u64,
        total: Option<u64>,
    },
    /// Hashing the package and running the updater's pre-flight checks. Split
    /// out from `Downloading` because it is the part with no progress to show,
    /// and a percentage frozen at 100 reads as a hang.
    Verifying,
    Installing,
    Failed(UpdateFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateFailure {
    Check(String),
    Prepare(String),
    Launch(String),
}

#[derive(Clone, Debug, Default)]
pub struct UpdateStatus {
    pub available: Option<AvailableUpdate>,
    pub phase: UpdatePhase,
    /// A package that is downloaded, verified and waiting. Mirrors what is on
    /// disk in `update.json` so the UI can offer "install now" without knowing
    /// where the staging directory is.
    pub ready: Option<PendingUpdate>,
    /// The last failure, carried across restarts. A failure that evaporates
    /// when the app closes is one the user can neither act on nor report.
    pub failure: Option<FailureRecord>,
}

impl Global for UpdateStatus {}

pub fn spawn_check(cx: &mut App) {
    // Before hydration and before the config gate: an interrupted portable
    // replacement has to surface whether or not checking is on, and it is
    // recorded as a failure so `hydrate_from_disk` below carries it into the
    // UI. The scan itself is one `read_dir`; the deletions it hands back ride
    // the background sweep.
    #[cfg(target_os = "windows")]
    let finished_backups = reconcile_portable_backups();
    #[cfg(not(target_os = "windows"))]
    let finished_backups: Vec<PathBuf> = Vec::new();
    // Before the config gate: a package staged by an earlier run, or a failure
    // from one, has to reach Settings whether or not checking is still on.
    hydrate_from_disk(cx);
    // Off the startup path: this can remove a 30 MB directory, and nothing
    // waits on the result.
    let keep = UpdateState::load().pending.map(|pending| pending.stage);
    cx.background_executor()
        .spawn(async move {
            for backup in finished_backups {
                log::info!(
                    "removing a completed update's leftover backup at {}",
                    backup.display()
                );
                let _ = std::fs::remove_dir_all(&backup);
            }
            sweep_orphaned_stages(keep)
        })
        .detach();
    if !cx.global::<Config>().check_for_updates {
        return;
    }
    spawn_check_inner(false, cx);
    spawn_recheck_loop(cx);
}

pub fn spawn_check_forced(cx: &mut App) {
    spawn_check_inner(true, cx);
}

/// Republishes what the last run left on disk. Without this the UI opens
/// claiming nothing is happening while a verified package sits in staging.
fn hydrate_from_disk(cx: &mut App) {
    let mut state = UpdateState::load();
    // A package for a version we are already running is finished business —
    // most often because it is the very update that just installed itself.
    if let Some(pending) = &state.pending
        && (!pending.is_usable() || !is_update_available(&pending.version, current_version()))
    {
        let stage = pending.stage.clone();
        state.pending = None;
        state.save();
        let _ = std::fs::remove_dir_all(stage);
    }
    let (ready, failure) = (state.pending.clone(), state.last_failure.clone());
    update_status(cx, |status| {
        status.ready = ready;
        status.failure = failure;
    });
}

fn spawn_recheck_loop(cx: &mut App) {
    // No exit condition: the task is dropped with the executor when the app
    // shuts down, and the config is re-read each time so switching checking off
    // takes effect without tearing the loop down.
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(RECHECK_INTERVAL).await;
            cx.update(|cx| {
                if cx.global::<Config>().check_for_updates {
                    spawn_check_inner(false, cx);
                }
            });
        }
    })
    .detach();
}

fn spawn_check_inner(report_failure: bool, cx: &mut App) {
    if is_busy(cx) {
        return;
    }
    update_status(cx, |status| status.phase = UpdatePhase::Checking);

    let manual_proxy = cx.global::<Config>().http_proxy.clone();
    let auto_download = cx.global::<Config>().auto_download_updates;
    let channel = cx.global::<Config>().update_channel;

    cx.spawn(async move |cx| {
        let current = current_version();
        let (release, version) = match fetch_latest_release(channel, manual_proxy)
            .or(async {
                cx.background_executor().timer(CHECK_TIMEOUT).await;
                Err(anyhow::anyhow!("timed out after {CHECK_TIMEOUT:?}"))
            })
            .await
        {
            Ok(v) => v,
            Err(e) => {
                log::debug!("update check skipped: {e:#}");
                let detail = format!("{e:#}");
                cx.update(|cx| {
                    update_status(cx, |status| {
                        status.phase = if report_failure {
                            UpdatePhase::Failed(UpdateFailure::Check(detail))
                        } else {
                            UpdatePhase::Idle
                        };
                    })
                });
                return;
            }
        };

        if !is_update_available(&version, current) {
            log::debug!("update check: up to date ({channel:?} {version}, running {current})");
            cx.update(|cx| {
                update_status(cx, |status| {
                    status.available = None;
                    status.phase = UpdatePhase::UpToDate;
                })
            });
            return;
        }

        let selection = select_release_asset(&version, &release.assets);
        let available = AvailableUpdate {
            version: version.clone(),
            installable: selection.asset.is_some(),
            install_hint: selection.reason,
            #[cfg(target_os = "windows")]
            needs_elevation: selection.needs_elevation,
            asset: selection.asset,
        };
        log::info!("update available: {version} (running {current})");

        cx.update(|cx| {
            update_status(cx, |status| {
                status.available = Some(available.clone());
                status.phase = UpdatePhase::Idle;
            })
        });

        let state = UpdateState::load();
        // Fetch while the prompt is up rather than after it: the answer people
        // give depends on how long the work sounds, and by the time they have
        // read the dialog the package is usually already there.
        let staged = state.pending.as_ref().is_some_and(|p| p.version == version);
        if auto_download && available.installable && !staged {
            // Fetching ahead is not consent to install. Cleared explicitly
            // because the flag outlives one download: a package armed by an
            // earlier "Install on Next Launch" must not arm this one too.
            APPLY_ON_LAUNCH.store(false, Ordering::Relaxed);
            cx.update(|cx| spawn_download(available.clone(), cx));
        }

        if !should_prompt(&state, &version) {
            return;
        }
        let Some(window) = wait_for_window(cx).await else {
            return;
        };
        let shown = cx.update(|cx| {
            window
                .update(cx, |_root, window, cx| {
                    prompt_update(&available, window, cx)
                })
                .is_ok()
        });
        if shown {
            let mut state = UpdateState::load();
            state.last_prompted = Some(version);
            state.remind_after = None;
            state.save();
        }
    })
    .detach();
}

/// Whether this version has earned another interruption.
///
/// The rule this replaces was "prompted once, never again", which meant a
/// failed install — or a mis-click — retired the version permanently and left
/// Settings as the only place it still existed.
fn should_prompt(state: &UpdateState, version: &str) -> bool {
    if state.last_prompted.as_deref() != Some(version) {
        return true;
    }
    // Same version we already asked about: only once "Later" has expired.
    state.remind_after.is_some_and(|due| now_secs() >= due)
}

fn is_busy(cx: &App) -> bool {
    cx.try_global::<UpdateStatus>().is_some_and(|status| {
        matches!(
            status.phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading { .. }
                | UpdatePhase::Verifying
                | UpdatePhase::Installing
        )
    })
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn update_status(cx: &mut App, edit: impl FnOnce(&mut UpdateStatus)) {
    let mut status = cx.try_global::<UpdateStatus>().cloned().unwrap_or_default();
    edit(&mut status);
    cx.set_global(status);
    cx.refresh_windows();
}

async fn wait_for_window(cx: &mut AsyncApp) -> Option<AnyWindowHandle> {
    for _ in 0..50 {
        if let Some(handle) = cx.update(|cx| cx.windows().first().copied()) {
            return Some(handle);
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    None
}


/// Pushes this version's next prompt out by `REMIND_LATER` instead of retiring
/// it. Any background download already under way is left alone: the package
/// being ready costs nothing and turns the next prompt into one keystroke.
fn remind_later() {
    let mut state = UpdateState::load();
    state.remind_after = Some(now_secs() + REMIND_LATER.as_secs());
    state.save();
}

/// Drops everything the previous channel produced, then checks the new feed.
///
/// A staged package and a deferred prompt are both answers to a question the
/// old feed asked; neither carries over. The staged package especially — one
/// armed for the next launch would otherwise install a Stable build onto
/// someone who just moved to Nightly.
///
/// "Everything" includes a download still in flight, which is the likely one:
/// checking starts a background transfer on its own, so the seconds spent
/// finding this setting are seconds that transfer is running. Cancelling stops
/// it where it can be stopped, and the generation bump covers the rest.
///
/// Nightly to Stable deliberately does *not* downgrade. The nightly in hand
/// keeps running until a stable release supersedes it, which is already how
/// `parse_version` orders a release above the prerelease sharing its core
/// version. Rolling back to an older build to honour the switch immediately
/// would be a bigger surprise than arriving there one release later.
pub fn switch_channel(cx: &mut App) {
    CHANNEL_GENERATION.fetch_add(1, Ordering::Relaxed);
    cancel_download(cx);
    discard_pending(cx);
    let mut state = UpdateState::load();
    state.last_prompted = None;
    state.remind_after = None;
    state.last_failure = None;
    state.save();
    update_status(cx, |status| {
        status.available = None;
        status.failure = None;
        status.phase = UpdatePhase::Idle;
    });
    spawn_check_forced(cx);
}

/// Abandons the transfer. The staging directory is removed by the download
/// thread as it unwinds, so nothing is left to collect.
pub fn cancel_download(cx: &mut App) {
    DOWNLOAD_CANCELLED.store(true, Ordering::Relaxed);
    INSTALL_WHEN_READY.store(false, Ordering::Relaxed);
    APPLY_ON_LAUNCH.store(false, Ordering::Relaxed);
    update_status(cx, |status| status.phase = UpdatePhase::Idle);
}

/// Install as soon as there is something to install: now if the package is
/// already staged, otherwise when the download in flight finishes.
pub fn install_available(cx: &mut App) {
    let status = cx.try_global::<UpdateStatus>().cloned().unwrap_or_default();
    if let Some(pending) = status.ready.clone()
        && pending.is_usable()
        && is_update_available(&pending.version, current_version())
    {
        launch_pending(pending, cx);
        return;
    }
    let Some(update) = status.available.clone() else {
        return;
    };
    if !update.installable {
        open_releases_page();
        return;
    }
    INSTALL_WHEN_READY.store(true, Ordering::Relaxed);
    // Asking to install now overrides an earlier "at next launch": whatever
    // lands is being handed straight to the updater.
    APPLY_ON_LAUNCH.store(false, Ordering::Relaxed);
    // A check with auto-download on has usually started one already; joining it
    // beats running a second copy of the same transfer.
    if !matches!(
        status.phase,
        UpdatePhase::Downloading { .. } | UpdatePhase::Verifying
    ) {
        spawn_download(update, cx);
    }
}

/// Have the package waiting, and let the next launch apply it unattended. This
/// is the option that costs the user nothing: no interrupted work now, and no
/// decision to make later either.
pub fn stage_for_next_launch(update: AvailableUpdate, cx: &mut App) {
    #[cfg(target_os = "windows")]
    if update.needs_elevation {
        // "Next launch" promises an unattended install, which an
        // elevation-needing package cannot keep — there is nobody to answer a
        // UAC prompt before the first window exists (#504). Offer the one
        // path that works instead: install now, with the user present.
        install_available(cx);
        return;
    }
    APPLY_ON_LAUNCH.store(true, Ordering::Relaxed);
    // Overrides a pending "install as soon as it lands": choosing next launch
    // is choosing not to be restarted now.
    INSTALL_WHEN_READY.store(false, Ordering::Relaxed);
    let status = cx.try_global::<UpdateStatus>().cloned().unwrap_or_default();
    if let Some(pending) = status
        .ready
        .clone()
        .filter(|pending| pending.version == update.version && pending.is_usable())
    {
        arm_pending(pending, cx);
        return;
    }
    if !matches!(
        status.phase,
        UpdatePhase::Downloading { .. } | UpdatePhase::Verifying
    ) {
        spawn_download(update, cx);
    }
}

/// Marks an already-staged package for unattended install at next launch.
fn arm_pending(mut pending: PendingUpdate, cx: &mut App) {
    pending.apply_on_launch = true;
    let mut state = UpdateState::load();
    state.pending = Some(pending.clone());
    state.save();
    update_status(cx, |status| status.ready = Some(pending));
}

/// Throws away a staged package and its staging directory.
pub fn discard_pending(cx: &mut App) {
    let mut state = UpdateState::load();
    if let Some(pending) = state.pending.take() {
        let _ = std::fs::remove_dir_all(&pending.stage);
    }
    state.save();
    update_status(cx, |status| status.ready = None);
}

/// Clears the recorded failure once the user has seen it.
pub fn dismiss_failure(cx: &mut App) {
    let mut state = UpdateState::load();
    state.last_failure = None;
    state.save();
    update_status(cx, |status| {
        status.failure = None;
        if matches!(status.phase, UpdatePhase::Failed(_)) {
            status.phase = UpdatePhase::Idle;
        }
    });
}

fn spawn_download(update: AvailableUpdate, cx: &mut App) {
    if matches!(
        cx.try_global::<UpdateStatus>().map(|status| &status.phase),
        Some(UpdatePhase::Downloading { .. } | UpdatePhase::Verifying | UpdatePhase::Installing)
    ) {
        return;
    }
    let Some(asset) = update.asset.clone() else {
        open_releases_page();
        return;
    };

    DOWNLOAD_CANCELLED.store(false, Ordering::Relaxed);
    DOWNLOAD_VERIFYING.store(false, Ordering::Relaxed);
    DOWNLOAD_RECEIVED.store(0, Ordering::Relaxed);
    DOWNLOAD_TOTAL.store(0, Ordering::Relaxed);
    update_status(cx, |status| {
        status.available = Some(update.clone());
        status.failure = None;
        status.phase = UpdatePhase::Downloading {
            received: 0,
            total: None,
        };
    });
    spawn_progress_pump(cx);

    let generation = CHANNEL_GENERATION.load(Ordering::Relaxed);
    let version = update.version.clone();
    let task = cx.background_executor().spawn(smol::unblock(move || {
        prepare_update(&version, &asset, &|received, total| {
            DOWNLOAD_RECEIVED.store(received, Ordering::Relaxed);
            DOWNLOAD_TOTAL.store(total.unwrap_or(0), Ordering::Relaxed);
            if DOWNLOAD_CANCELLED.load(Ordering::Relaxed) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
    }));

    cx.spawn(async move |cx| {
        let prepared = match task.await {
            Ok(prepared) => prepared,
            Err(error) => {
                // Both flags are consent to *this* attempt. Leaving either set
                // would hand the next background download — one the user never
                // asked for — a licence to restart the app under them.
                INSTALL_WHEN_READY.store(false, Ordering::Relaxed);
                APPLY_ON_LAUNCH.store(false, Ordering::Relaxed);
                if DOWNLOAD_CANCELLED.load(Ordering::Relaxed) {
                    log::info!("update download cancelled");
                    // Only if nothing has claimed the phase since. A channel
                    // switch cancels this download and starts a check in the
                    // same breath, and the check is already the live work by
                    // the time the transfer notices it was called off.
                    cx.update(|cx| {
                        update_status(cx, |status| {
                            if matches!(
                                status.phase,
                                UpdatePhase::Downloading { .. } | UpdatePhase::Verifying
                            ) {
                                status.phase = UpdatePhase::Idle;
                            }
                        })
                    });
                    return;
                }
                let detail = format!("{error:#}");
                log::error!("update failed: {detail}");
                cx.update(|cx| {
                    record_failure(&update.version, &detail, cx);
                    update_status(cx, |status| {
                        status.phase = UpdatePhase::Failed(UpdateFailure::Prepare(detail));
                    });
                });
                return;
            }
        };

        let pending = prepared.into_pending(
            update.version.clone(),
            APPLY_ON_LAUNCH.load(Ordering::Relaxed),
        );
        // The feed moved under this download. Staging it anyway would hand a
        // Nightly user the Stable package they were mid-way through fetching
        // when they left — and if they had pressed install, relaunch them into
        // it. Both flags were consent to a version from the old channel.
        if CHANNEL_GENERATION.load(Ordering::Relaxed) != generation {
            log::info!(
                "discarding {}: the update channel changed while it was being prepared",
                update.version
            );
            INSTALL_WHEN_READY.store(false, Ordering::Relaxed);
            APPLY_ON_LAUNCH.store(false, Ordering::Relaxed);
            let _ = std::fs::remove_dir_all(&pending.stage);
            return;
        }
        let mut state = UpdateState::load();
        state.pending = Some(pending.clone());
        state.last_failure = None;
        state.save();
        cx.update(|cx| {
            update_status(cx, |status| {
                status.ready = Some(pending.clone());
                status.failure = None;
                status.phase = UpdatePhase::Idle;
            });
            if INSTALL_WHEN_READY.swap(false, Ordering::Relaxed) {
                launch_pending(pending, cx);
            }
        });
    })
    .detach();
}

/// Samples the download counters into the global the UI renders from. Stops as
/// soon as the phase leaves the transfer, so a finished, cancelled or failed
/// download does not leave a timer running.
fn spawn_progress_pump(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(PROGRESS_TICK).await;
            let live = cx.update(|cx| {
                let received = DOWNLOAD_RECEIVED.load(Ordering::Relaxed);
                let total = DOWNLOAD_TOTAL.load(Ordering::Relaxed);
                let verifying = DOWNLOAD_VERIFYING.load(Ordering::Relaxed);
                let mut live = false;
                update_status(cx, |status| {
                    if matches!(
                        status.phase,
                        UpdatePhase::Downloading { .. } | UpdatePhase::Verifying
                    ) {
                        status.phase = if verifying {
                            UpdatePhase::Verifying
                        } else {
                            UpdatePhase::Downloading {
                                received,
                                total: (total > 0).then_some(total),
                            }
                        };
                        live = true;
                    }
                });
                live
            });
            if !live {
                return;
            }
        }
    })
    .detach();
}

fn launch_pending(pending: PendingUpdate, cx: &mut App) {
    update_status(cx, |status| status.phase = UpdatePhase::Installing);
    #[cfg(target_os = "windows")]
    if pending.needs_elevation {
        return launch_pending_elevated(pending, cx);
    }
    match pending.launch() {
        Ok(()) => {
            // The updater owns the staging directory from here. Forgetting it
            // now is what stops a relaunch from trying to install it twice.
            let mut state = UpdateState::load();
            state.pending = None;
            state.last_prompted = None;
            state.save();
            cx.quit();
        }
        Err(error) => {
            let detail = format!("{error:#}");
            log::error!("could not start the installer: {detail}");
            record_failure(&pending.version, &detail, cx);
            update_status(cx, |status| {
                status.phase = UpdatePhase::Failed(UpdateFailure::Launch(detail));
            });
        }
    }
}

/// Starts the elevated install chain (#504): a de-elevated watcher that will
/// relaunch the app, then a single UAC prompt covering the two privileged
/// stages.
///
/// Success looks exactly like a plain install from here — this process quits
/// and the watcher, not the GUI, owns the relaunch. Declining the UAC prompt
/// is not a failure: nothing ever ran elevated, nothing needs recording, and
/// the staged package is still usable, so the plan stays put and the app
/// stays up. Settings still shows it as ready to install.
#[cfg(target_os = "windows")]
fn launch_pending_elevated(pending: PendingUpdate, cx: &mut App) {
    // ShellExecuteEx cannot run on the UI thread: raising the UAC prompt
    // pumps this thread's message loop (the shell broadcasts change
    // notifications through the window), which re-enters gpui while its App
    // is borrowed and aborts the process on a RefCell double-borrow. The
    // whole launch — watcher spawn included, so the pairing stays atomic —
    // runs on a background thread; only the bookkeeping comes back.
    let launch = cx.background_executor().spawn(smol::unblock(move || {
        let version = pending.version.clone();
        (pending.launch_elevated(), version)
    }));
    cx.spawn(async move |cx| {
        let (result, version) = launch.await;
        cx.update(|cx| match result {
            Ok(ElevationStart::Started) => {
                // The watcher owns the staging directory from here. Forgetting
                // it now is what stops a relaunch from trying to install it
                // twice.
                let mut state = UpdateState::load();
                state.pending = None;
                state.last_prompted = None;
                state.save();
                cx.quit();
            }
            Ok(ElevationStart::Declined) => {
                log::info!(
                    "administrator approval for the {version} update was declined; \
                     the package stays staged"
                );
                update_status(cx, |status| status.phase = UpdatePhase::Idle);
            }
            Err(error) => {
                let detail = format!("{error:#}");
                log::error!("could not start the elevated install: {detail}");
                record_failure(&version, &detail, cx);
                update_status(cx, |status| {
                    status.phase = UpdatePhase::Failed(UpdateFailure::Launch(detail));
                });
            }
        })
    })
    .detach();
}

/// Records a failure where the user can still find it tomorrow, and lets the
/// version prompt again.
///
/// The old code wrote `last_prompted` the moment a dialog appeared and never
/// looked back, so an install that failed a second later took the version with
/// it: never prompted again, and the only trace was a phase that died with the
/// process.
fn record_failure(version: &str, detail: &str, cx: &mut App) {
    let record = FailureRecord {
        version: version.to_string(),
        detail: detail.to_string(),
    };
    let mut state = UpdateState::load();
    state.last_failure = Some(record.clone());
    if state.last_prompted.as_deref() == Some(version) {
        state.last_prompted = None;
        state.remind_after = None;
    }
    state.save();
    update_status(cx, |status| status.failure = Some(record));
}

/// Opens the page for whichever channel this installation follows. A Nightly
/// user sent to the Stable release page would be handed the wrong package —
/// and, since Linux and unsupported installs update by hand, that page is the
/// entire update path for some of them.
///
/// Reads the config from disk rather than the global: several callers reach
/// here from a background thread where the `App` is out of reach.

pub fn apply_pending_at_launch() -> bool {
    let mut state = UpdateState::load();
    let Some(pending) = state.pending.clone() else {
        return false;
    };
    if !pending.apply_on_launch {
        return false;
    }
    if !pending.is_usable() || !is_update_available(&pending.version, current_version()) {
        log::info!(
            "discarding the staged {} update: no longer applicable to {}",
            pending.version,
            current_version()
        );
        state.pending = None;
        state.save();
        let _ = std::fs::remove_dir_all(&pending.stage);
        return false;
    }

    #[cfg(target_os = "windows")]
    if pending.needs_elevation {
        // An unattended start has nobody to approve a UAC prompt, and raising
        // one before the first window — unsigned path and all — reads as
        // malware. The plan stays staged; once the app is up, Settings offers
        // the install with the window behind it (#504).
        log::info!(
            "the staged {} update needs administrator approval; leaving it for an interactive install",
            pending.version
        );
        return false;
    }

    log::info!(
        "applying the staged {} update before startup",
        pending.version
    );
    // Cleared before the handover rather than after: the updater takes the
    // staging directory with it, so a plan left on disk would be retried at the
    // next launch against a directory that no longer exists.
    state.pending = None;
    state.last_prompted = None;
    state.remind_after = None;
    state.save();

    match pending.launch() {
        Ok(()) => true,
        Err(error) => {
            let detail = format!("{error:#}");
            log::error!("could not apply the staged update: {detail}");
            let mut state = UpdateState::load();
            state.last_failure = Some(FailureRecord {
                version: pending.version.clone(),
                detail,
            });
            state.save();
            false
        }
    }
}

/// Folds the outcome the updater recorded for the install attempt that
/// produced this launch into the on-disk update state, then removes the file.
///
/// This is what a failed install used to lack (#540): the GUI quits as soon
/// as the helper is spawned, so without the outcome file a failure lived only
/// in `update.log` — and because launching the helper had already cleared the
/// prompt state, the next check simply offered the same version again. Runs
/// before any window exists; `spawn_check`'s hydration carries the result
/// into Settings.
pub fn absorb_update_outcome_at_launch() {
    use anterm_core::daemon::install::outcome::{UpdateOutcome, read_outcome};

    let Some(path) = update_outcome_path() else {
        return;
    };
    let outcome = match read_outcome(&path) {
        Ok(Some(outcome)) => outcome,
        Ok(None) => return,
        // A result that exists but cannot be read is itself a result: an
        // updater ran, and what it left is unusable.
        Err(error) => UpdateOutcome {
            version: current_version().to_string(),
            ok: false,
            detail: Some(format!(
                "the update result at {} could not be read: {error}",
                path.display()
            )),
        },
    };
    // Consumed either way: an outcome describes the attempt that already ran,
    // never the next one.
    let _ = std::fs::remove_file(&path);

    let mut state = UpdateState::load();
    if outcome.ok {
        if outcome.version == current_version() {
            log::info!("the update to {} completed", outcome.version);
            // A failure recorded by an earlier attempt at this same version
            // is finished business now.
            if state
                .last_failure
                .as_ref()
                .is_some_and(|failure| failure.version == outcome.version)
            {
                state.last_failure = None;
                state.save();
            }
        } else {
            // The helper said the install went in, yet this process is a
            // different version — someone reinstalled by hand in between.
            // Nothing to show, but the log should have it.
            log::warn!(
                "the updater reported installing {} but this is {}; ignoring the stale outcome",
                outcome.version,
                current_version()
            );
        }
        return;
    }

    let detail = outcome
        .detail
        .unwrap_or_else(|| "the update failed without recording a reason".to_string());
    log::error!("the update to {} failed: {detail}", outcome.version);
    state.last_failure = Some(FailureRecord {
        version: outcome.version.clone(),
        detail,
    });
    // A failure must not retire the version — `last_prompted` set with no
    // `remind_after` is "one failed install retires it for good", the
    // invariant `a_failure_lets_the_version_prompt_again` pins. But this
    // attempt already restarted the app once, and `spawn_check` runs at every
    // launch: asking again seconds later is the nag loop #540 is about. The
    // middle course is the one "Later" already uses — keep the version marked
    // as asked, and push the next ask out by `REMIND_LATER`. The failure sits
    // in Settings the whole while.
    state.last_prompted = Some(outcome.version);
    state.remind_after = Some(now_secs() + REMIND_LATER.as_secs());
    state.save();
}

/// Removes staging directories belonging to a run that died before its updater
/// could clean up — quitting mid-download is the usual cause.
///
/// Worth doing: on macOS staging is created beside the app bundle, which is
/// normally /Applications, and a hidden 30 MB directory there is one nobody
/// finds on purpose. The live plan's own directory is kept however old it is.
fn sweep_orphaned_stages(keep: Option<PathBuf>) {
    for root in stage_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if keep.as_deref() == Some(path.as_path()) {
                continue;
            }
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_stage_name)
            {
                continue;
            }
            let expired = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > STAGE_TTL);
            if expired {
                log::info!("removing orphaned update staging at {}", path.display());
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

/// Where each platform's staging directories are created — beside the bundle
/// on macOS and beside the image on Linux (both have to be on the installed
/// file's own volume to rename into place), and the per-user temp directory
/// on Windows.
// The `return`s are what let one cfg block win per platform; clippy sees only
// the surviving one and reads it as redundant.
#[allow(clippy::needless_return)]
fn stage_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return current_macos_app_bundle()
            .and_then(|app| app.parent().map(Path::to_path_buf))
            .into_iter()
            .collect();
    }
    #[cfg(target_os = "linux")]
    {
        return current_appimage()
            .and_then(|appimage| appimage.parent().map(Path::to_path_buf))
            .into_iter()
            .collect();
    }
    #[cfg(target_os = "windows")]
    {
        return vec![std::env::temp_dir()];
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Vec::new()
}

/// The prefixes `update_staging_dir` and `system_update_staging_dir` hand to
/// `tempfile`.
fn is_stage_name(name: &str) -> bool {
    name.starts_with(".anterm-update-") || name.starts_with("anterm-update-")
}

/// Deals with `.anterm-update-backup-*` directories a previous portable update
/// left in the installation.
///
/// One still carrying the incomplete marker means the replacement was cut
/// short — power loss, a kill — and the installed files may mix two versions.
/// That is recorded as an update failure so Settings shows it (and shows it
/// again at every launch until the user restores or deletes the backup: the
/// warning describes a condition, not an event). The backup itself is kept —
/// it holds the only copy of the previous files.
///
/// One without the marker is a completed update whose backup deletion lost to
/// an antivirus scan; those are returned for the background sweep to remove.
#[cfg(target_os = "windows")]
fn reconcile_portable_backups() -> Vec<PathBuf> {
    let Some(WindowsUpdateLayout::Portable(dir)) = current_windows_update_layout() else {
        return Vec::new();
    };
    let (interrupted, finished) = scan_portable_backups(&dir);
    if !interrupted.is_empty() {
        // All of them, not the first: two interrupted attempts in a row leave
        // two backups, and one the user never hears about is one they delete
        // blind or keep forever.
        let preserved = interrupted
            .iter()
            .map(|backup| backup.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let detail = format!(
            "a previous update was interrupted while replacing the installed files, so {} may \
             mix two versions; the files from before are preserved at {preserved} — restore \
             them or reinstall, then delete the backup",
            dir.display()
        );
        log::warn!("{detail}");
        let mut state = UpdateState::load();
        if state.last_failure.as_ref().map(|failure| &failure.detail) != Some(&detail) {
            state.last_failure = Some(FailureRecord {
                version: current_version().to_string(),
                detail,
            });
            state.save();
        }
    }
    finished
}

/// Splits the backups under `dir` into (interrupted, finished) by the
/// incomplete marker. Pure directory inspection, so the policy above stays
/// testable without a real installation.
#[cfg(target_os = "windows")]
fn scan_portable_backups(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut interrupted = Vec::new();
    let mut finished = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (interrupted, finished);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let named = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(WINDOWS_PORTABLE_BACKUP_PREFIX));
        if !named || !path.is_dir() {
            continue;
        }
        if path.join(WINDOWS_PORTABLE_BACKUP_INCOMPLETE).is_file() {
            interrupted.push(path);
        } else {
            finished.push(path);
        }
    }
    (interrupted, finished)
}


/// The shape of a persisted [`PendingUpdate`]. A plan written by a build
/// whose updater invocation differs from this one's is discarded rather than
/// launched: its staged helper is the *old* build's updater and would not
/// understand this build's arguments. Absent from pre-parameterization
/// plans, which deserialize as 0 and never match.
const PLAN_VERSION: u32 = 2;

/// A downloaded, verified package waiting to be installed.
///
/// Splitting "fetch" from "install" is the point of the whole design: it turns
/// the question put to the user from "will you spend five minutes on this now"
/// into "may I restart", which is the difference between an update that gets
/// applied and one that gets postponed indefinitely.
///
/// The parent pid is *not* stored. It is the one argument that cannot survive a
/// restart, so `launch` supplies it fresh — see `PreparedUpdate::args`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingUpdate {
    pub version: String,
    /// Whether the next launch installs this without asking. False for a
    /// package that was merely fetched ahead of time: it waits in Settings.
    #[serde(default)]
    pub apply_on_launch: bool,
    /// See [`PLAN_VERSION`].
    #[serde(default)]
    plan_version: u32,
    updater: PathBuf,
    command: String,
    rest: Vec<PathBuf>,
    config_dir: Option<PathBuf>,
    stage: PathBuf,
    /// Windows, all-users Inno layout: install through the elevated chain
    /// (#504) rather than by spawning the staged helper directly.
    #[serde(default)]
    needs_elevation: bool,
    /// The staged package's digest as the release server published it. The
    /// elevated chain's trust anchor: the checksums file beside the package
    /// cannot serve there, because a medium-integrity process can rewrite
    /// both together.
    #[serde(default)]
    expected_sha256: Option<String>,
}

impl PendingUpdate {
    /// Whether the package is still on disk and still speaks this build's
    /// updater protocol. Staging lives in a temporary directory that a
    /// cleaner, an antivirus, or a reboot may have taken; the plan version
    /// is the other half — see [`PLAN_VERSION`].
    pub fn is_usable(&self) -> bool {
        self.plan_version == PLAN_VERSION && self.updater.is_file() && self.stage.is_dir()
    }

    fn launch(&self) -> Result<()> {
        PreparedUpdate {
            updater: self.updater.clone(),
            command: self.command.clone(),
            rest: self.rest.clone(),
            config_dir: self.config_dir.clone(),
            stage: self.stage.clone(),
            needs_elevation: self.needs_elevation,
            expected_sha256: self.expected_sha256.clone(),
        }
        .launch()
    }

    /// The pieces of a staged Inno plan that the elevated launch needs, read
    /// out of `rest` by position — the order `prepare_windows_update` pushes
    /// them. A plan that cannot name every piece is not elevated.
    #[cfg(target_os = "windows")]
    fn elevated_parts(&self) -> Option<ElevatedPlanParts> {
        if !self.needs_elevation || self.command != "install" || self.rest.len() != 7 {
            return None;
        }
        Some(ElevatedPlanParts {
            installer: self.rest[0].clone(),
            asset_name: self.rest[2].to_str()?.to_string(),
            install_dir: self.rest[3].clone(),
            version: self.rest[4].to_str()?.to_string(),
            log: self.rest[5].clone(),
            stage: self.rest[6].clone(),
            expected_sha256: self.expected_sha256.clone()?,
        })
    }
}

/// See [`PendingUpdate::elevated_parts`].
#[cfg(target_os = "windows")]
struct ElevatedPlanParts {
    installer: PathBuf,
    asset_name: String,
    install_dir: PathBuf,
    version: String,
    log: PathBuf,
    stage: PathBuf,
    expected_sha256: String,
}

/// How asking Windows to start the privileged half ended. Only `Started`
/// hands the machine to elevated processes; `Declined` means the user
/// answered the UAC prompt with "no" and nothing ever ran elevated.
#[cfg(target_os = "windows")]
enum ElevationStart {
    Started,
    Declined,
}

#[cfg(target_os = "windows")]
impl PendingUpdate {
    /// Spawns the de-elevated relaunch watcher, then raises the single UAC
    /// prompt that covers both privileged stages (#504).
    ///
    /// The watcher goes first so a declined prompt leaves exactly one
    /// medium-integrity process to reap. The elevated half is always the
    /// *installed* updater — the trust root a medium-integrity process cannot
    /// rewrite — and everything it needs arrives as arguments, because an
    /// elevated child (over-the-shoulder especially) does not inherit this
    /// process's environment.
    fn launch_elevated(&self) -> Result<ElevationStart> {
        use windows_sys::Win32::Foundation::ERROR_CANCELLED;

        let mut parts = self
            .elevated_parts()
            .context("the staged package does not describe an elevated install")?;
        // The prompt names the installation this process is running from, not
        // the one the plan names. `update.json` lives in the user's config
        // directory: a plan naming some other directory would aim a UAC
        // prompt — and the elevated half's `<install-dir>` — at a binary of
        // the plan writer's choosing.
        let installed_updater =
            bundled_updater().context("anterm-updater.exe is not installed beside this app")?;
        let installed_dir = installed_updater
            .parent()
            .context("the installed updater names no directory")?
            .to_path_buf();
        parts.install_dir = installed_dir.clone();
        let Some(status) = update_elevation_status_path() else {
            anyhow::bail!("no config directory for the elevation status file");
        };
        // The watcher requires it: without the outcome file the install's
        // result would die with the elevated chain (#540).
        let Some(outcome) = update_outcome_path() else {
            anyhow::bail!("no config directory for the install outcome file");
        };
        let app = std::env::current_exe().context("locating the running app")?;
        // Stale markers from an earlier attempt have to be gone: the watcher
        // reads both files, and would otherwise act on a dead attempt's
        // answer.
        let _ = std::fs::remove_file(&status);
        let _ = std::fs::remove_file(&outcome);

        let mut watcher = Command::new(&self.updater);
        watcher
            .arg("relaunch-watcher")
            .arg("--status-file")
            .arg(&status)
            .arg("--result-file")
            .arg(&outcome)
            .arg("--app-path")
            .arg(&app)
            .arg("--log")
            .arg(&parts.log)
            .arg("--expected-version")
            .arg(&parts.version)
            // So the watcher can tell a GUI that is quitting from one that
            // stayed: a declined prompt leaves this process on screen, and
            // the relaunch it would otherwise perform on its own timeout
            // would put a second window beside it.
            .arg("--gui-pid")
            .arg(std::process::id().to_string());
        if let Some(dir) = &self.config_dir {
            watcher.arg("--config-dir").arg(dir);
        }
        let mut watcher = anterm_core::proc::hide_console(&mut watcher)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("launching the relaunch watcher")?;

        let parameters =
            elevated_stage_arguments(&parts, &status, &outcome, self.config_dir.as_deref());
        match shell_execute_elevated(&installed_updater, &parameters, &installed_dir) {
            Ok(()) => Ok(ElevationStart::Started),
            Err(code) => {
                // Whatever the answer was, the watcher must not outlive this
                // process when no elevated half is coming: it would spend its
                // whole 15-minute grace waiting for a chain that will never
                // report, while the app it was to relaunch is still running.
                let _ = watcher.kill();
                let _ = std::fs::remove_file(&status);
                if code == ERROR_CANCELLED {
                    return Ok(ElevationStart::Declined);
                }
                Err(anyhow::anyhow!(
                    "asking Windows to run the install elevated failed (error {code})"
                ))
            }
        }
    }
}

/// The command line for the privileged first stage, as a single string:
/// ShellExecuteEx hands it to the child's CRT argv splitter verbatim, so
/// every argument is quoted — the install directory typically lives under
/// `C:\Program Files`. Built as an `OsString` so a path that is not valid
/// Unicode survives the round trip.
#[cfg(target_os = "windows")]
fn elevated_stage_arguments(
    parts: &ElevatedPlanParts,
    status_file: &Path,
    outcome: &Path,
    config_dir: Option<&Path>,
) -> std::ffi::OsString {
    let mut line = std::ffi::OsString::new();
    let mut push = |arg: &std::ffi::OsStr| push_quoted(&mut line, arg);
    push(std::ffi::OsStr::new("elevated-stage"));
    push(std::ffi::OsStr::new(&std::process::id().to_string()));
    push(parts.installer.as_os_str());
    push(std::ffi::OsStr::new(&parts.asset_name));
    push(parts.install_dir.as_os_str());
    push(std::ffi::OsStr::new(&parts.version));
    push(parts.log.as_os_str());
    push(parts.stage.as_os_str());
    push(std::ffi::OsStr::new("--expected-sha256"));
    push(std::ffi::OsStr::new(&parts.expected_sha256));
    push(std::ffi::OsStr::new("--status-file"));
    push(status_file.as_os_str());
    if let Some(dir) = config_dir {
        push(std::ffi::OsStr::new("--config-dir"));
        push(dir.as_os_str());
    }
    push(std::ffi::OsStr::new("--result-file"));
    push(outcome.as_os_str());
    line
}

/// Appends one argument the way `CommandLineToArgvW` — and the CRT splitter
/// the elevated helper is parsed by — reads back verbatim.
///
/// Wrapping in quotes is not enough on its own. A backslash is an escape
/// character only in front of a quote, so a path that *ends* in one
/// (`--config-dir "D:\anterm\"`) escapes its own closing quote and swallows the
/// rest of the command line into a single argument; an embedded quote splits
/// one argument into several. Neither can be reached across the integrity
/// boundary — every value here comes from this user's own session — but the
/// receiver runs elevated, and an argument list it can misread is not a thing
/// to leave to the shape of a path.
#[cfg(target_os = "windows")]
fn push_quoted(line: &mut std::ffi::OsString, arg: &std::ffi::OsStr) {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;

    let mut quoted = vec![QUOTE];
    // The run of backslashes immediately behind the cursor: doubled if a
    // quote follows it, left alone otherwise.
    let mut backslashes = 0usize;
    for unit in arg.encode_wide() {
        match unit {
            BACKSLASH => {
                backslashes += 1;
                quoted.push(unit);
            }
            QUOTE => {
                quoted.resize(quoted.len() + backslashes, BACKSLASH);
                quoted.push(BACKSLASH);
                quoted.push(QUOTE);
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                quoted.push(unit);
            }
        }
    }
    // The closing quote is a quote like any other: the run in front of it has
    // to be doubled too.
    quoted.resize(quoted.len() + backslashes, BACKSLASH);
    quoted.push(QUOTE);

    if !line.is_empty() {
        line.push(" ");
    }
    line.push(std::ffi::OsString::from_wide(&quoted));
}

/// Starts `executable` elevated — the `runas` verb is what turns the request
/// into exactly one UAC prompt — and returns as soon as Windows answers. The
/// elevated process is deliberately not tracked by handle: the status file
/// and the watcher own liveness from here, which is what lets this process
/// quit immediately.
///
/// The error case carries the raw Windows error code so the caller can tell
/// "the user said no" (`ERROR_CANCELLED`) apart from a genuine launch
/// failure.
#[cfg(target_os = "windows")]
fn shell_execute_elevated(
    executable: &Path,
    parameters: &std::ffi::OsStr,
    working_dir: &Path,
) -> std::result::Result<(), u32> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb = wide(std::ffi::OsStr::new("runas"));
    let file = wide(executable.as_os_str());
    let params = wide(parameters);
    let directory = wide(working_dir.as_os_str());
    // SAFETY: zero-initializing is valid for this all-POD struct; every field
    // that matters is set below.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    // No error dialog from the shell itself: a decline is a normal answer the
    // caller handles, and a real failure is reported in-process.
    info.fMask = SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = directory.as_ptr();
    // The updater is a console program; an elevated console window flashing
    // up beside the GUI reads as a crash.
    info.nShow = SW_HIDE;
    // SAFETY: `info` is fully initialized, and every pointer names a
    // NUL-terminated buffer that outlives the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: called immediately after the failed call on the same
        // thread, so the error code is the one ShellExecuteExW set.
        return Err(unsafe { GetLastError() });
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FailureRecord {
    pub version: String,
    pub detail: String,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct UpdateState {
    /// The version a dialog has already been shown for. Paired with
    /// `remind_after`, which decides when that stops counting.
    #[serde(default)]
    last_prompted: Option<String>,
    /// Unix seconds after which `last_prompted` may prompt again.
    #[serde(default)]
    remind_after: Option<u64>,
    #[serde(default)]
    last_failure: Option<FailureRecord>,
    #[serde(default)]
    pending: Option<PendingUpdate>,
}

impl UpdateState {
    fn path() -> Option<std::path::PathBuf> {
        crate::config::config_path("update.json")
    }

    fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("failed to parse {}: {e}; ignoring", path.display());
            Self::default()
        })
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let json = match serde_json::to_string_pretty(self) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("failed to serialize update state: {e}");
                return;
            }
        };
        if let Err(e) = crate::config::write_atomic(&path, json.as_bytes()) {
            log::warn!("failed to write {}: {e}", path.display());
        }
    }
}

