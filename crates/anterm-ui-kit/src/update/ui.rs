//! GPUI update prompt dialog, localized status/hint formatting, and external
//! release-page URL opening.

use gpui::{App, PromptLevel, Window};

use crate::config::{Config, UpdateChannel};
use anterm_i18n::{L10nKey, t, t_fmt};

use super::{
    AvailableUpdate, NIGHTLY_RELEASE_URL, RELEASES_URL, UpdateFailure, UpdateInstallHint,
    UpdatePhase, current_version, install_available, remind_later, stage_for_next_launch,
};

pub(super) fn prompt_update(update: &AvailableUpdate, window: &mut Window, cx: &mut App) {
    let hint = update
        .install_hint
        .as_ref()
        .map(localized_update_install_hint);
    let detail = if update.installable {
        // Windows cannot replace a running daemon's image, so its install path
        // stops the background service — the promise that panes survive is
        // only true where the daemon really does keep running (macOS).
        let detail_key = if cfg!(target_os = "windows") {
            L10nKey::UpdateDialogDetailWindows
        } else {
            L10nKey::UpdateDialogDetail
        };
        let base = t_fmt(
            detail_key,
            &[
                ("version", update.version.as_str()),
                ("current", current_version()),
            ],
        );
        // The elevation warning outranks any other note: it is the one thing
        // the user must know *before* the app quits, because the next thing
        // they see is a UAC prompt (#504).
        if update.needs_elevation() {
            format!("{base} {}", t(L10nKey::UpdateDialogNeedsElevation))
        } else {
            match hint.as_deref() {
                Some(note) => format!("{base} {note}"),
                None => base,
            }
        }
    } else {
        t_fmt(
            L10nKey::UpdateDialogDetailManual,
            &[
                ("version", update.version.as_str()),
                ("current", current_version()),
                (
                    "hint",
                    hint.as_deref()
                        .unwrap_or_else(|| t(L10nKey::UpdateDialogCannotSelfUpdate)),
                ),
            ],
        )
    };
    // "Later" is the cancel answer: it takes Escape, and a closed window or a
    // dropped channel falls through to it too, so the outcome nobody chose is
    // always the one that changes nothing. Skipping a version is a decision
    // people should have to reach for — it lives in Settings, not one stray
    // keystroke away.
    let buttons: Vec<gpui::PromptButton> = if update.installable {
        let mut buttons = vec![gpui::PromptButton::ok(t(
            L10nKey::SettingsUpdateAndRelaunch,
        ))];
        // An unattended next-launch install is a promise an elevation-needing
        // package cannot keep: nobody is there to answer UAC before the first
        // window (#504). The only honest offer is "now".
        if !update.needs_elevation() {
            buttons.push(gpui::PromptButton::ok(t(L10nKey::UpdateDialogNextLaunch)));
        }
        buttons.push(gpui::PromptButton::cancel(t(L10nKey::UpdateDialogLater)));
        buttons
    } else {
        vec![
            gpui::PromptButton::ok(t(L10nKey::SettingsUpdateViewRelease)),
            gpui::PromptButton::cancel(t(L10nKey::UpdateDialogLater)),
        ]
    };
    let answer = window.prompt(
        PromptLevel::Info,
        t(L10nKey::UpdateDialogTitle),
        Some(&detail),
        &buttons,
        cx,
    );
    let update = update.clone();
    cx.spawn(async move |cx| {
        let installable = update.installable;
        // With elevation in play, button 1 *is* "Later" — only a layout that
        // offered "next launch" may treat index 1 as that answer.
        let offered_next_launch = installable && !update.needs_elevation();
        match answer.await {
            Ok(0) if installable => {
                cx.update(install_available);
            }
            Ok(0) => open_releases_page(),
            Ok(1) if offered_next_launch => {
                cx.update(|cx| stage_for_next_launch(update, cx));
            }
            // "Later" — index 1 or 2 depending on the shape — plus a dropped
            // channel and a closed window. All of them change nothing.
            _ => remind_later(),
        }
    })
    .detach();
}


pub fn open_releases_page() {
    open_url(match Config::load().update_channel {
        UpdateChannel::Stable => RELEASES_URL,
        UpdateChannel::Nightly => NIGHTLY_RELEASE_URL,
    });
}

pub fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(url).spawn() {
        log::warn!("failed to open {url}: {e}");
    }
}

/// Installs a package the user asked to have applied at the next launch, and
/// reports whether this process should now get out of the way.
///
/// This is what makes "Install on Next Launch" worth offering: the user never
/// waits for a download and never answers a second question — they quit anterm
/// one evening and start a new version the next morning.
///
/// Must run before the daemon is contacted and before any window exists.
/// Spawning a daemon this process is about to abandon costs a restart, and a
/// window that appears and vanishes reads as a crash.
///
/// Every failure path clears the plan and returns `false`. An update that
/// cannot be applied must never become an app that will not start.

pub fn localized_update_phase(phase: &UpdatePhase) -> Option<String> {
    match phase {
        UpdatePhase::Idle => None,
        UpdatePhase::Checking => Some(t(L10nKey::SettingsUpdateChecking).to_string()),
        UpdatePhase::UpToDate => Some(t(L10nKey::SettingsUpdateUpToDate).to_string()),
        UpdatePhase::Downloading { received, total } => Some(match total {
            // A percentage needs both ends; GitHub occasionally serves the
            // asset without a usable content-length, and "42%" of an unknown
            // whole is worse than an honest byte count.
            Some(total) if *total > 0 => t_fmt(
                L10nKey::SettingsUpdateDownloadingPercent,
                &[
                    (
                        "percent",
                        &(received.saturating_mul(100) / total).min(100).to_string(),
                    ),
                    ("size", &human_bytes(*total)),
                ],
            ),
            _ => t_fmt(
                L10nKey::SettingsUpdateDownloadingBytes,
                &[("received", &human_bytes(*received))],
            ),
        }),
        UpdatePhase::Verifying => Some(t(L10nKey::SettingsUpdateVerifying).to_string()),
        UpdatePhase::Installing => Some(t(L10nKey::SettingsUpdateInstalling).to_string()),
        UpdatePhase::Failed(failure) => {
            let (key, error) = match failure {
                UpdateFailure::Check(error) => (L10nKey::SettingsUpdateCheckFailed, error),
                UpdateFailure::Prepare(error) => (L10nKey::SettingsUpdatePrepareFailed, error),
                UpdateFailure::Launch(error) => (L10nKey::SettingsUpdateLaunchFailed, error),
            };
            Some(t_fmt(key, &[("error", error)]))
        }
    }
}

pub fn localized_update_install_hint(hint: &UpdateInstallHint) -> String {
    match hint {
        #[cfg(target_os = "macos")]
        UpdateInstallHint::UnsupportedMacos => {
            t(L10nKey::SettingsUpdateUnsupportedMacos).to_string()
        }
        #[cfg(target_os = "linux")]
        UpdateInstallHint::UnsupportedLinux => {
            t(L10nKey::SettingsUpdateUnsupportedLinux).to_string()
        }
        #[cfg(target_os = "linux")]
        UpdateInstallHint::LinuxManualPackage(name) => {
            t_fmt(L10nKey::SettingsUpdateLinuxPackage, &[("name", name)])
        }
        #[cfg(target_os = "windows")]
        UpdateInstallHint::UnsupportedWindows => {
            t(L10nKey::SettingsUpdateUnsupportedWindows).to_string()
        }
        #[cfg(target_os = "windows")]
        UpdateInstallHint::WindowsAllUsersInstall => {
            t(L10nKey::SettingsUpdateWindowsAllUsers).to_string()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        UpdateInstallHint::UnsupportedPlatform => {
            t(L10nKey::SettingsUpdateUnsupportedPlatform).to_string()
        }
        UpdateInstallHint::MissingPackage(name) => {
            t_fmt(L10nKey::SettingsUpdateMissingPackage, &[("name", name)])
        }
        UpdateInstallHint::MissingChecksums => {
            t(L10nKey::SettingsUpdateMissingChecksums).to_string()
        }
    }
}

fn human_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    format!("{:.1} MB", bytes as f64 / MB)
}

