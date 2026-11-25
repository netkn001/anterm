use super::*;
use crate::config::UpdateChannel;

#[test]
fn stable_desktop_fallback_reads_past_a_page_of_mobile_releases() {
    let mut pages = Vec::new();
    let release = smol::block_on(find_stable_desktop_release(async |page| {
        pages.push(page);
        Ok(if page == 1 {
            vec![
                LatestRelease {
                    tag_name: "mobile-v0.1.6".into(),
                    draft: false,
                    prerelease: false,
                    assets: vec![github_asset("anterm-0.1.6-android-arm64.apk")],
                };
                100
            ]
        } else {
            vec![LatestRelease {
                tag_name: "v26.9.4".into(),
                draft: false,
                prerelease: false,
                assets: vec![github_asset("anterm-26.9.4-macos-arm64.zip")],
            }]
        })
    }))
    .unwrap();
    assert_eq!(pages, [1, 2]);
    assert_eq!(release.tag_name, "v26.9.4");
}

#[test]
fn stable_desktop_fallback_stops_at_the_end_and_propagates_errors() {
    let absent = smol::block_on(find_stable_desktop_release(async |_| Ok(vec![])));
    assert!(
        absent
            .unwrap_err()
            .to_string()
            .contains("no stable desktop release")
    );
    let failure = smol::block_on(find_stable_desktop_release(async |_| {
        anyhow::bail!("GitHub API rate limit exceeded")
    }));
    assert!(
        failure
            .unwrap_err()
            .to_string()
            .contains("rate limit exceeded")
    );
}

#[test]
fn stable_desktop_filter_skips_mobile_drafts_and_nightlies() {
    let releases: Vec<LatestRelease> = serde_json::from_value(serde_json::json!([
        {"tag_name": "mobile-v0.1.6", "assets": [
            {"name": "anterm-0.1.6-android-arm64.apk", "browser_download_url": "https://example.com/mobile"}
        ]},
        {"tag_name": "v26.9.6", "draft": true, "assets": [
            {"name": "anterm-26.9.6-macos-arm64.zip", "browser_download_url": "https://example.com/draft"}
        ]},
        {"tag_name": "v26.9.5", "prerelease": true, "assets": [
            {"name": "anterm-26.9.5-macos-arm64.zip", "browser_download_url": "https://example.com/prerelease"}
        ]},
        {"tag_name": "v26.9.5-nightly.20261012", "assets": [
            {"name": "anterm-26.9.5-nightly.20261012-macos-arm64.zip", "browser_download_url": "https://example.com/nightly"}
        ]},
        {"tag_name": "v26.9.4", "draft": false, "prerelease": false, "assets": [
            {"name": "anterm-26.9.4-macos-arm64.zip", "browser_download_url": "https://example.com/desktop"}
        ]}
    ])).unwrap();
    assert!(
        releases[..4]
            .iter()
            .all(|release| !release.is_stable_desktop())
    );
    assert_eq!(
        releases
            .iter()
            .find(|release| release.is_stable_desktop())
            .unwrap()
            .tag_name,
        "v26.9.4"
    );
}

#[test]
fn desktop_release_requires_a_matching_desktop_package() {
    let mut release = LatestRelease {
        tag_name: "v26.9.4".into(),
        draft: false,
        prerelease: false,
        assets: vec![],
    };
    for name in [
        "SHA256SUMS",
        "anterm-server-linux-x86_64-musl",
        "anterm-26.9.4-android-arm64.apk",
        "anterm-26.9.3-macos-arm64.zip",
    ] {
        release.assets.push(github_asset(name));
    }
    assert!(!release.is_stable_desktop());
    release
        .assets
        .push(github_asset("anterm-26.9.4-linux-x86_64.tar.gz"));
    assert!(release.is_stable_desktop());
}

#[test]
fn github_rate_limit_error_says_when_to_retry() {
    let error = github_http_error(
        403,
        Some("0"),
        Some("4600"),
        br#"{"message":"API rate limit exceeded for 203.0.113.1."}"#,
        1000,
    );

    assert_eq!(
        error,
        "GitHub API rate limit exceeded; try again in about 60 minutes (HTTP 403)"
    );
}

/// GitHub answers a spent quota with 403 or 429 depending on the endpoint
/// and the era. Only the 403 spelling used to reach the retry advice, so a
/// 429 told the reader the quota was gone without saying when it returns.
#[test]
fn a_429_is_a_rate_limit_too() {
    let error = github_http_error(
        429,
        Some("0"),
        Some("1600"),
        br#"{"message":"API rate limit exceeded for 203.0.113.1."}"#,
        1000,
    );

    assert_eq!(
        error,
        "GitHub API rate limit exceeded; try again in about 10 minutes (HTTP 429)"
    );
}

#[test]
fn expired_github_rate_limit_says_to_retry_shortly() {
    let error = github_http_error(
        403,
        Some("0"),
        Some("999"),
        br#"{"message":"API rate limit exceeded."}"#,
        1000,
    );

    assert_eq!(
        error,
        "GitHub API rate limit exceeded; try again shortly (HTTP 403)"
    );
}

#[test]
fn github_json_error_keeps_a_short_actionable_message() {
    let error = github_http_error(
        404,
        None,
        None,
        br#"{"message":"Not Found\nPlease check the repository"}"#,
        1000,
    );

    assert_eq!(
        error,
        "GitHub returned HTTP 404: Not Found Please check the repository"
    );
}

fn github_asset(name: &str) -> GitHubAsset {
    GitHubAsset {
        name: name.to_string(),
        browser_download_url: format!("https://example.test/{name}"),
    }
}

#[test]
fn release_asset_requires_the_platform_package_and_checksums() {
    let name = "anterm-27.1.0-macos-arm64.zip";
    let assets = [github_asset(name), github_asset("checksums.txt")];
    let selected = select_release_asset_for(Ok(PackageOffer::plain(name.to_string())), &assets);
    assert_eq!(
        selected.asset,
        Some(ReleaseAsset {
            name: name.to_string(),
            url: format!("https://example.test/{name}"),
            checksums_url: "https://example.test/checksums.txt".to_string(),
        })
    );
    assert_eq!(selected.reason, None);
}

#[test]
fn release_without_checksums_is_never_installable() {
    let name = "anterm-27.1.0-macos-arm64.zip";
    let selected = select_release_asset_for(
        Ok(PackageOffer::plain(name.to_string())),
        &[github_asset(name)],
    );
    assert!(selected.asset.is_none());
    assert_eq!(selected.reason, Some(UpdateInstallHint::MissingChecksums));
}

#[test]
fn release_without_the_exact_platform_package_is_never_guessed() {
    let selected = select_release_asset_for(
        Ok(PackageOffer::plain(
            "anterm-27.1.0-macos-arm64.zip".to_string(),
        )),
        &[
            github_asset("anterm-27.1.0-macos-x86_64.zip"),
            github_asset("checksums.txt"),
        ],
    );
    assert!(selected.asset.is_none());
    assert_eq!(
        selected.reason,
        Some(UpdateInstallHint::MissingPackage(
            "anterm-27.1.0-macos-arm64.zip".to_string()
        ))
    );
}

/// The one Linux shape that installs itself: an AppImage whose directory
/// takes a staging directory and whose image carries the helper.
#[cfg(target_os = "linux")]
#[test]
fn an_appimage_that_can_replace_itself_is_offered_the_appimage() {
    let offer = linux_package_for("27.1.0", "x86_64", true, true)
        .expect("a self-updating AppImage yields an offer");
    assert_eq!(offer.name, "anterm-27.1.0-linux-x86_64.AppImage");
}

/// Everything else keeps the manual hint, and the hint names the exact
/// package for the installation shape rather than pointing at a release
/// page with five files on it.
#[cfg(target_os = "linux")]
#[test]
fn linux_installs_that_cannot_self_update_name_their_package() {
    // An AppImage in a read-only directory, or one from before the
    // helper shipped: still an AppImage, still updated by hand.
    assert_eq!(
        linux_package_for("27.1.0", "x86_64", true, false).unwrap_err(),
        UpdateInstallHint::LinuxManualPackage("anterm-27.1.0-linux-x86_64.AppImage".to_string())
    );
    // A tarball (or distro-packaged) install is never guessed at.
    assert_eq!(
        linux_package_for("27.1.0", "x86_64", false, false).unwrap_err(),
        UpdateInstallHint::LinuxManualPackage("anterm-27.1.0-linux-x86_64.tar.gz".to_string())
    );
    // `can_self_update` without an AppImage cannot happen (the probe is
    // gated on the variable), but the policy must not invent an offer if
    // it ever does.
    assert!(linux_package_for("27.1.0", "x86_64", false, true).is_err());
}

/// The offered AppImage name must match what the release actually
/// publishes, checksums manifest included — the same end-to-end shape the
/// macOS selection test pins.
#[cfg(target_os = "linux")]
#[test]
fn appimage_selection_matches_the_published_asset_names() {
    let name = "anterm-27.1.0-linux-x86_64.AppImage";
    let offer = linux_package_for("27.1.0", "x86_64", true, true).unwrap();
    let assets = [
        github_asset("anterm-27.1.0-linux-x86_64.tar.gz"),
        github_asset(name),
        github_asset("checksums.txt"),
    ];
    let selected = select_release_asset_for(Ok(offer), &assets);
    assert_eq!(
        selected.asset,
        Some(ReleaseAsset {
            name: name.to_string(),
            url: format!("https://example.test/{name}"),
            checksums_url: "https://example.test/checksums.txt".to_string(),
        })
    );
    assert_eq!(selected.reason, None);
}

#[cfg(target_os = "windows")]
#[test]
fn portable_backup_scan_separates_interrupted_from_finished() {
    let root = tempfile::tempdir().unwrap();
    let interrupted = root.path().join(".anterm-update-backup-cut");
    std::fs::create_dir(&interrupted).unwrap();
    std::fs::write(interrupted.join(WINDOWS_PORTABLE_BACKUP_INCOMPLETE), b"").unwrap();
    let finished = root.path().join(".anterm-update-backup-done");
    std::fs::create_dir(&finished).unwrap();
    // Neither a user's directory nor a stray file may be touched.
    std::fs::create_dir(root.path().join("completions")).unwrap();
    std::fs::write(root.path().join(".anterm-update-backup-not-a-dir"), b"file").unwrap();

    let (got_interrupted, got_finished) = scan_portable_backups(root.path());
    assert_eq!(got_interrupted, vec![interrupted]);
    assert_eq!(got_finished, vec![finished]);
}

#[test]
fn parses_versions_with_and_without_prefix() {
    let release = |major, minor, patch| Some((major, minor, patch, true, vec![]));
    assert_eq!(parse_version("v0.3.1"), release(0, 3, 1));
    assert_eq!(parse_version("0.3.1"), release(0, 3, 1));
    assert_eq!(parse_version(" 1.2.0 "), release(1, 2, 0));
    assert_eq!(parse_version("v2"), release(2, 0, 0));
    assert_eq!(parse_version("v2.5"), release(2, 5, 0));
    assert_eq!(
        parse_version("v0.4.0-rc.1"),
        Some((0, 4, 0, false, vec![1]))
    );
    assert_eq!(
        parse_version("26.7.1-nightly.202607161800"),
        Some((26, 7, 1, false, vec![202607161800]))
    );
    // A prerelease with nothing numeric to order by still parses; it just
    // sorts below every stamped build of the same core version.
    assert_eq!(parse_version("1.0.0-beta"), Some((1, 0, 0, false, vec![])));
    assert_eq!(parse_version("0.4.0+ci.7"), release(0, 4, 0));
    assert_eq!(parse_version("nightly"), None);
    assert_eq!(parse_version(""), None);
    assert_eq!(parse_version("v0.3.1.1"), None);
    assert_eq!(parse_version("0.3.1.0"), None);
    assert_eq!(parse_version("vv0.3.1"), None);
}

/// Every numeric identifier is collected, so appending one to the nightly
/// stamp extends the ordering instead of replacing it. Reading only the
/// last segment made `…20260807.2` score 2 and lose to the build before it.
#[test]
fn prerelease_identifiers_order_left_to_right() {
    assert_eq!(
        parse_version("26.8.2-nightly.20260807.2"),
        Some((26, 8, 2, false, vec![20260807, 2]))
    );
    assert!(is_update_available(
        "26.8.2-nightly.20260807.2",
        "26.8.2-nightly.20260807"
    ));
    assert!(!is_update_available(
        "26.8.2-nightly.20260807",
        "26.8.2-nightly.20260807.2"
    ));
    // The day still dominates whatever trails it.
    assert!(is_update_available(
        "26.8.2-nightly.20260808",
        "26.8.2-nightly.20260807.9"
    ));
    // And the move from date to minute stamps carries the installs that
    // are already out there: 202608071800 > 20260807.
    assert!(is_update_available(
        "26.8.2-nightly.202608071800",
        "26.8.2-nightly.20260807"
    ));
}

/// Two builds in one day used to collide on the date and read as "up to
/// date" — the reason the stamp goes to the minute.
#[test]
fn two_builds_on_one_day_are_distinguishable() {
    assert!(is_update_available(
        "26.8.2-nightly.202608071800",
        "26.8.2-nightly.202608070200"
    ));
}

#[test]
fn detects_newer_versions() {
    assert!(is_update_available("v0.3.1", "0.3.0"));
    assert!(is_update_available("v1.0.0", "0.9.9"));
    assert!(is_update_available("0.4.0", "0.3.99"));
    assert!(is_update_available("v26.7.0", "0.17.0"));
}

#[test]
fn ignores_same_or_older_versions() {
    assert!(!is_update_available("v0.3.0", "0.3.0"));
    assert!(!is_update_available("v0.2.9", "0.3.0"));
    assert!(!is_update_available("0.3.0", "0.3.1"));
}

#[test]
fn nightly_binaries_prompt_when_their_stable_ships() {
    assert!(is_update_available("v26.7.1", "26.7.1-nightly.20260716"));
    assert!(!is_update_available("v26.7.0", "26.7.1-nightly.20260716"));
    assert!(!is_update_available("v26.7.1-rc.1", "26.7.1"));
}

/// The Nightly channel's whole reason to exist: last night's build has to
/// be able to supersede the one before it. This is what the old ordering
/// deliberately refused to do, back when the only feed was
/// `/releases/latest` and no nightly could ever be offered.
#[test]
fn a_newer_nightly_supersedes_an_older_one() {
    assert!(is_update_available(
        "26.7.1-nightly.20260717",
        "26.7.1-nightly.20260716"
    ));
    assert!(!is_update_available(
        "26.7.1-nightly.20260716",
        "26.7.1-nightly.20260717"
    ));
    assert!(!is_update_available(
        "26.7.1-nightly.20260716",
        "26.7.1-nightly.20260716"
    ));
    // Across core versions the date is irrelevant — a nightly for the next
    // patch wins however old its build is.
    assert!(is_update_available(
        "26.7.2-nightly.20260101",
        "26.7.1-nightly.20260716"
    ));
}

/// Ordering the release flag ahead of the build number is what keeps this
/// true: a stable release outranks every dated build sharing its core
/// version, so a user who switches back to Stable still graduates.
#[test]
fn a_stable_release_still_outranks_every_nightly_of_its_core_version() {
    for date in ["20260101", "20991231"] {
        assert!(is_update_available(
            "v26.7.1",
            &format!("26.7.1-nightly.{date}")
        ));
    }
}

/// Each channel reads its own release, which is the mechanism that keeps a
/// Nightly from being walked back onto Stable by an update it never asked
/// for. `/releases/latest` excludes prereleases by definition, so the two
/// feeds cannot see each other's builds.
#[test]
fn each_channel_reads_its_own_feed() {
    assert!(release_endpoint(UpdateChannel::Stable).ends_with("/releases/latest"));
    assert!(
        release_endpoint(UpdateChannel::Nightly)
            .ends_with(&format!("/releases/tags/{NIGHTLY_TAG}"))
    );
    // The page a Nightly user is sent to has to be the release that feed
    // reads. `concat!` cannot build the URL from the constant, so this is
    // where the two are held together.
    assert!(NIGHTLY_RELEASE_URL.ends_with(&format!("/releases/tag/{NIGHTLY_TAG}")));
}

/// The fallback for a nightly published without `nightly.json`. The tag is
/// the literal "nightly", so the asset names are the only place left that
/// states which build this is.
#[test]
fn version_is_recovered_from_nightly_asset_names() {
    let assets = [
        github_asset("checksums.txt"),
        github_asset("anterm-26.8.2-nightly.20260807-macos-arm64.zip"),
        github_asset("anterm-26.8.2-nightly.20260807-windows-x86_64-setup.exe"),
    ];
    assert_eq!(
        version_from_assets(&assets).as_deref(),
        Some("26.8.2-nightly.20260807")
    );
}

/// The remote-server binaries ride along in the same release and match the
/// `anterm-…-linux-…` shape, but have no version in front of the platform.
/// `parse_version` rejecting "server" is what skips them, so no name-based
/// special case is needed.
#[test]
fn remote_server_assets_are_not_mistaken_for_a_version() {
    let assets = [
        github_asset("checksums.txt"),
        github_asset("anterm-server-linux-x86_64-musl"),
        github_asset("anterm-server-linux-aarch64-musl"),
    ];
    assert_eq!(version_from_assets(&assets), None);

    // And they must not win when a real package is also present, whatever
    // order GitHub returns them in.
    let mixed = [
        github_asset("anterm-server-linux-x86_64-musl"),
        github_asset("anterm-26.8.2-nightly.202608071800-linux-x86_64.tar.gz"),
    ];
    assert_eq!(
        version_from_assets(&mixed).as_deref(),
        Some("26.8.2-nightly.202608071800")
    );
}

/// Tonight's packages are uploaded before last night's are pruned, so both
/// nights are briefly on the release at once — and stay that way for good
/// if the prune step ever fails. GitHub lists assets oldest first, so
/// taking the first match would take yesterday's build and read as "no
/// update", stalling the channel with no error anywhere.
#[test]
fn the_newest_asset_version_wins_when_two_nights_overlap() {
    let assets = [
        github_asset("anterm-26.8.2-nightly.202608062200-macos-arm64.zip"),
        github_asset("anterm-26.8.2-nightly.202608062200-linux-x86_64.tar.gz"),
        github_asset("checksums.txt"),
        github_asset("anterm-26.8.2-nightly.202608071800-macos-arm64.zip"),
        github_asset("anterm-26.8.2-nightly.202608071800-linux-x86_64.tar.gz"),
    ];
    assert_eq!(
        version_from_assets(&assets).as_deref(),
        Some("26.8.2-nightly.202608071800")
    );
}

/// Pins the contract between `nightly.yml`'s manifest step and this side of
/// it. Renaming a field in the workflow silently drops Nightly back to
/// guessing versions out of filenames; this fails instead.
#[test]
fn nightly_manifest_matches_what_the_workflow_writes() {
    let manifest: NightlyManifest = serde_json::from_str(
        r#"{
            "version": "26.8.2-nightly.20260807",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "published_at": "2026-08-07T02:11:00Z"
        }"#,
    )
    .expect("the workflow's shape must deserialize");
    assert_eq!(manifest.version, "26.8.2-nightly.20260807");
    assert!(parse_version(&manifest.version).is_some());

    // Older manifests, or a workflow that stops writing the extras, must
    // still yield a usable version rather than failing the whole check.
    let minimal: NightlyManifest =
        serde_json::from_str(r#"{"version": "26.8.2-nightly.20260807"}"#)
            .expect("version alone is enough");
    assert_eq!(minimal.commit, "");
}

#[test]
fn unparseable_tag_never_prompts() {
    assert!(!is_update_available("garbage", "0.3.0"));
    assert!(!is_update_available("v0.3.1", "garbage"));
    assert!(!is_update_available("v0.4.0.1", "0.3.0"));
    assert!(!is_update_available("vv0.4.0", "0.3.0"));
}

#[test]
fn update_state_round_trips_and_defaults() {
    let _lock = UPDATE_STATE_LOCK.lock().unwrap();
    crate::config::pin_test_config_dir();
    let path = UpdateState::path().expect("config dir pinned");

    let _ = std::fs::remove_file(&path);
    assert_eq!(UpdateState::load().last_prompted, None);

    UpdateState {
        last_prompted: Some("0.4.0".into()),
        ..Default::default()
    }
    .save();
    assert_eq!(UpdateState::load().last_prompted.as_deref(), Some("0.4.0"));

    // A staged package has to survive the restart it exists for. Its
    // whole point is being applied by a *later* process than the one that
    // downloaded it.
    UpdateState {
        pending: Some(PendingUpdate {
            version: "27.0.0".into(),
            apply_on_launch: true,
            plan_version: PLAN_VERSION,
            updater: PathBuf::from("/tmp/anterm-updater"),
            command: "install".into(),
            rest: vec![PathBuf::from("/tmp/stage/anterm.zip")],
            config_dir: None,
            stage: PathBuf::from("/tmp/stage"),
            needs_elevation: false,
            expected_sha256: None,
        }),
        ..Default::default()
    }
    .save();
    let pending = UpdateState::load().pending.expect("pending round-trips");
    assert_eq!(pending.version, "27.0.0");
    assert!(pending.apply_on_launch);
    assert_eq!(pending.command, "install");

    let _ = std::fs::remove_file(&path);
}

/// The rule that replaced "prompted once, then silent forever".
#[test]
fn later_defers_a_version_instead_of_retiring_it() {
    let asked = UpdateState {
        last_prompted: Some("27.0.0".into()),
        ..Default::default()
    };
    // Already asked, no expiry set: stay quiet.
    assert!(!should_prompt(&asked, "27.0.0"));
    // A different version is a new question however recently we asked.
    assert!(should_prompt(&asked, "27.1.0"));

    let deferred = UpdateState {
        last_prompted: Some("27.0.0".into()),
        remind_after: Some(now_secs() + 3600),
        ..Default::default()
    };
    assert!(!should_prompt(&deferred, "27.0.0"));

    let due = UpdateState {
        last_prompted: Some("27.0.0".into()),
        remind_after: Some(now_secs().saturating_sub(1)),
        ..Default::default()
    };
    assert!(should_prompt(&due, "27.0.0"));
}

/// A failure must not leave the version marked as "already asked about",
/// or one failed install retires it for good.
#[test]
fn a_failure_lets_the_version_prompt_again() {
    let mut state = UpdateState {
        last_prompted: Some("27.0.0".into()),
        remind_after: Some(now_secs() + 3600),
        ..Default::default()
    };
    assert!(!should_prompt(&state, "27.0.0"));

    // The bookkeeping half of `record_failure`, which needs an App to run.
    state.last_failure = Some(FailureRecord {
        version: "27.0.0".into(),
        detail: "downloading anterm-27.0.0-macos-arm64.zip: timed out".into(),
    });
    if state.last_prompted.as_deref() == Some("27.0.0") {
        state.last_prompted = None;
        state.remind_after = None;
    }
    assert!(should_prompt(&state, "27.0.0"));
}

/// Serializes the tests that touch the real `update.json` under the
/// pinned per-process config dir (the pattern `update_guard`'s tests
/// document: one shared state file, parallel tests, one lock).
static UPDATE_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn updater_tail_args_carry_config_and_outcome_as_arguments() {
    assert_eq!(
        updater_tail_args(
            Some(Path::new(r"C:\cfg")),
            Some(Path::new(r"C:\cfg\update-outcome.json")),
        ),
        [
            std::ffi::OsString::from("--config-dir"),
            std::ffi::OsString::from(r"C:\cfg"),
            std::ffi::OsString::from("--result-file"),
            std::ffi::OsString::from(r"C:\cfg\update-outcome.json"),
        ]
    );
    // Each flag stands alone; a machine with no resolvable config dir
    // simply passes neither, which is the updater's pre-existing default.
    assert!(updater_tail_args(None, None).is_empty());
    assert_eq!(
        updater_tail_args(None, Some(Path::new("/tmp/outcome.json"))),
        [
            std::ffi::OsString::from("--result-file"),
            std::ffi::OsString::from("/tmp/outcome.json"),
        ]
    );
}

/// The whole outcome-file lifecycle in one test: the file and
/// `update.json` are per-process global state, and parallel tests
/// writing both would race each other.
#[test]
fn an_updater_outcome_is_absorbed_exactly_once() {
    use anterm_core::daemon::install::outcome::{UpdateOutcome, write_outcome};

    let _lock = UPDATE_STATE_LOCK.lock().unwrap();
    crate::config::pin_test_config_dir();
    let state_path = UpdateState::path().expect("config dir pinned");
    let outcome_path = update_outcome_path().expect("config dir pinned");
    let _ = std::fs::remove_file(&state_path);
    let _ = std::fs::remove_file(&outcome_path);

    // No outcome: nothing changes, nothing is invented.
    absorb_update_outcome_at_launch();
    let state = UpdateState::load();
    assert!(state.last_failure.is_none() && state.last_prompted.is_none());

    // A failure lands in the state and — the #540 half — does not set the
    // version up to prompt again on its own right away: the throttle is
    // `remind_after`, never a retirement.
    write_outcome(
        &outcome_path,
        &UpdateOutcome {
            version: "27.0.0".into(),
            ok: false,
            detail: Some("the installer exited with code 5".into()),
        },
    )
    .unwrap();
    absorb_update_outcome_at_launch();
    let mut state = UpdateState::load();
    assert_eq!(
        state.last_failure,
        Some(FailureRecord {
            version: "27.0.0".into(),
            detail: "the installer exited with code 5".into(),
        })
    );
    assert!(!should_prompt(&state, "27.0.0"));
    // The version is throttled, not retired: once the reminder expires the
    // question comes back — the same invariant
    // `a_failure_lets_the_version_prompt_again` pins for the in-process
    // path.
    let due = state
        .remind_after
        .expect("a failed install defers the next prompt rather than retiring it");
    assert!(due > now_secs());
    state.remind_after = Some(now_secs().saturating_sub(1));
    assert!(should_prompt(&state, "27.0.0"));
    assert!(!outcome_path.exists(), "the outcome is consumed once");

    // A later launch finds no file and changes nothing.
    absorb_update_outcome_at_launch();
    assert_eq!(
        UpdateState::load()
            .last_failure
            .as_ref()
            .map(|f| &f.version),
        Some(&"27.0.0".to_string())
    );

    // A success retires the failure recorded for that same version. The
    // two name the running version here because the crate version is the
    // only "current" a test can have.
    write_outcome(
        &outcome_path,
        &UpdateOutcome {
            version: current_version().to_string(),
            ok: false,
            detail: Some("first attempt failed".into()),
        },
    )
    .unwrap();
    absorb_update_outcome_at_launch();
    assert_eq!(
        UpdateState::load()
            .last_failure
            .as_ref()
            .map(|f| &f.version),
        Some(&current_version().to_string())
    );
    write_outcome(
        &outcome_path,
        &UpdateOutcome {
            version: current_version().to_string(),
            ok: true,
            detail: None,
        },
    )
    .unwrap();
    absorb_update_outcome_at_launch();
    assert_eq!(UpdateState::load().last_failure, None);
    assert!(!outcome_path.exists());

    // A success naming some other version is stale (a hand-installed
    // rollback in between): dropped, never shown.
    write_outcome(
        &outcome_path,
        &UpdateOutcome {
            version: "99.0.0".into(),
            ok: true,
            detail: None,
        },
    )
    .unwrap();
    absorb_update_outcome_at_launch();
    assert_eq!(UpdateState::load().last_failure, None);

    // Garbage is an outcome too: a failure with an odd detail beats
    // silence about an attempt that definitely ran.
    std::fs::write(&outcome_path, b"not json").unwrap();
    absorb_update_outcome_at_launch();
    let state = UpdateState::load();
    assert!(
        state
            .last_failure
            .as_ref()
            .is_some_and(|failure| failure.detail.contains("could not be read")),
        "{:?}",
        state.last_failure
    );
    assert!(!outcome_path.exists());

    let _ = std::fs::remove_file(&state_path);
}

#[test]
fn only_our_own_staging_directories_are_swept() {
    assert!(is_stage_name(".anterm-update-abc123"));
    assert!(is_stage_name("anterm-update-abc123"));
    assert!(!is_stage_name("anterm.app"));
    assert!(!is_stage_name(".Trash"));
    assert!(!is_stage_name("anterm-update"));
}

#[test]
fn a_staged_plan_supplies_a_fresh_parent_pid() {
    let prepared = PreparedUpdate {
        updater: PathBuf::from("/tmp/anterm-updater"),
        command: "install".into(),
        rest: vec![PathBuf::from("/tmp/stage/anterm.zip")],
        config_dir: None,
        stage: PathBuf::from("/tmp/stage"),
        needs_elevation: false,
        expected_sha256: None,
    };
    let args = prepared.args();
    assert_eq!(args[0], PathBuf::from("install"));
    // The pid is the one argument that cannot be persisted: a plan written
    // today names a process that is gone by the time it runs.
    assert_eq!(args[1], PathBuf::from(std::process::id().to_string()));
    assert_eq!(args[2], PathBuf::from("/tmp/stage/anterm.zip"));
}

#[cfg(target_os = "windows")]
#[test]
fn windows_markers_distinguish_inno_portable_and_unknown_layouts() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("anterm-app.exe");
    std::fs::write(&executable, b"test app").unwrap();
    assert_eq!(windows_update_layout_for(&executable), None);

    std::fs::write(root.path().join(WINDOWS_PORTABLE_MARKER), b"portable-v1").unwrap();
    assert_eq!(
        windows_update_layout_for(&executable),
        Some(WindowsUpdateLayout::Portable(root.path().to_path_buf()))
    );

    std::fs::write(root.path().join(WINDOWS_INNO_INSTALL_MARKER), b"inno-v1").unwrap();
    assert_eq!(
        windows_update_layout_for(&executable),
        Some(WindowsUpdateLayout::Inno(root.path().to_path_buf()))
    );

    std::fs::remove_file(root.path().join(WINDOWS_INNO_INSTALL_MARKER)).unwrap();
    std::fs::write(root.path().join(WINDOWS_PORTABLE_MARKER), b"invalid").unwrap();
    assert_eq!(windows_update_layout_for(&executable), None);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_layout_selects_the_matching_release_package() {
    let directory = PathBuf::from(r"C:\anterm");
    assert_eq!(
        windows_package_for_layout("26.8.2", &WindowsUpdateLayout::Inno(directory.clone()))
            .as_deref(),
        Some(if cfg!(target_arch = "aarch64") {
            "anterm-26.8.2-windows-arm64-setup.exe"
        } else {
            "anterm-26.8.2-windows-x86_64-setup.exe"
        })
    );
    assert_eq!(
        windows_package_for_layout("26.8.2", &WindowsUpdateLayout::Portable(directory))
            .as_deref(),
        Some(if cfg!(target_arch = "aarch64") {
            "anterm-26.8.2-windows-arm64.zip"
        } else {
            "anterm-26.8.2-windows-x86_64.zip"
        })
    );
}

#[cfg(target_os = "windows")]
#[test]
fn an_all_users_inno_install_is_never_updated_in_place() {
    let all_users = PathBuf::from(r"C:\Program Files\anterm");
    let per_user = PathBuf::from(r"C:\Users\someone\AppData\Local\Programs\anterm");

    // The machine-hive entry names this directory: elevation would be
    // required, so anterm declines however writable the directory looks.
    assert!(windows_inno_needs_elevation_for(
        Some(&all_users),
        &all_users,
        true
    ));
    // Inno stores the path with a trailing separator in `InstallLocation`
    // and without one in `Inno Setup: App Path`; both name one place.
    assert!(windows_inno_needs_elevation_for(
        Some(Path::new(r"C:\Program Files\anterm\")),
        &all_users,
        true
    ));
    assert!(windows_inno_needs_elevation_for(
        Some(Path::new(r"c:\program files\ANTERM")),
        &all_users,
        true
    ));

    // A per-user install on a machine that also carries an all-users one
    // updates itself: the machine entry names a different directory.
    assert!(!windows_inno_needs_elevation_for(
        Some(&all_users),
        &per_user,
        true
    ));
    assert!(!windows_inno_needs_elevation_for(None, &per_user, true));

    // No machine entry, but the directory refuses writes — a relocated or
    // pruned installation Setup could not replace either.
    assert!(windows_inno_needs_elevation_for(None, &per_user, false));
}

#[cfg(target_os = "windows")]
#[test]
fn an_all_users_inno_install_reports_the_elevation_hint() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("anterm-app.exe");
    std::fs::write(&executable, b"test app").unwrap();
    std::fs::write(root.path().join(WINDOWS_INNO_INSTALL_MARKER), b"inno-v1").unwrap();
    let layout = windows_update_layout_for(&executable).unwrap();

    // A writable temp directory is never the all-users installation, so
    // this layout is offered the normal in-place update.
    assert_eq!(
        windows_layout_updatability(&layout),
        WindowsUpdatability::Updatable
    );

    assert_eq!(
        select_release_asset_for(Err(UpdateInstallHint::WindowsAllUsersInstall), &[]).reason,
        Some(UpdateInstallHint::WindowsAllUsersInstall)
    );
    let hint = UpdateInstallHint::WindowsAllUsersInstall.english();
    assert!(hint.contains("all users"), "{hint}");
    assert!(hint.contains("release page"), "{hint}");
}

#[cfg(target_os = "windows")]
#[test]
fn an_unwritable_portable_directory_is_not_offered_an_update() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().to_path_buf();
    assert!(windows_directory_is_writable(&directory));
    assert_eq!(
        windows_layout_updatability(&WindowsUpdateLayout::Portable(directory)),
        WindowsUpdatability::Updatable
    );

    let missing = root.path().join("gone");
    assert!(!windows_directory_is_writable(&missing));
    assert_eq!(
        windows_layout_updatability(&WindowsUpdateLayout::Portable(missing)),
        WindowsUpdatability::Unsupported(UpdateInstallHint::UnsupportedWindows)
    );
}

/// Reads the real machine hive. Vacuous on a machine with no all-users
/// installation; on one that has it, the value Inno actually wrote must be
/// an absolute path and must make the decision function refuse an in-place
/// update of that directory.
#[cfg(target_os = "windows")]
#[test]
fn the_all_users_install_path_lookup_survives_this_machine() {
    let Some(path) = windows_all_users_install_path() else {
        return;
    };
    assert!(path.is_absolute(), "{}", path.display());
    assert!(
        windows_inno_needs_elevation_for(Some(&path), &path, true),
        "the installed all-users path {} was not recognised",
        path.display()
    );
}

/// A plan persisted by a build from before the tail-argument protocol
/// cannot be carried out by its own staged helper: it relied on the
/// environment, which an elevated child never inherits (#504). The plan
/// version is what quietly drops those plans at the next launch instead
/// of failing weirdly against a helper that does not understand its
/// arguments.
#[test]
fn a_plan_from_another_protocol_version_is_not_usable() {
    let root = tempfile::tempdir().unwrap();
    let updater = root.path().join("anterm-updater");
    std::fs::write(&updater, b"test updater").unwrap();
    let stage = root.path().join("stage");
    std::fs::create_dir(&stage).unwrap();

    let plan = |plan_version| PendingUpdate {
        version: "27.0.0".into(),
        apply_on_launch: true,
        plan_version,
        updater: updater.clone(),
        command: "install".into(),
        rest: vec![stage.join("anterm.zip")],
        config_dir: None,
        stage: stage.clone(),
        needs_elevation: false,
        expected_sha256: None,
    };
    assert!(plan(PLAN_VERSION).is_usable());
    // Plans written before the field existed deserialize it as 0.
    assert!(!plan(0).is_usable());
    assert!(!plan(PLAN_VERSION + 1).is_usable());
}

#[cfg(target_os = "windows")]
#[test]
fn an_elevation_capable_updater_is_recognised_by_its_own_answer() {
    assert!(capabilities_cover_elevation(
        b"elevated-stage\ninstall-elevated\nrelaunch-watcher\n"
    ));
    // Order and unrelated tokens are the updater's business.
    assert!(capabilities_cover_elevation(
        b"install\nrelaunch-watcher\nelevated-stage\ninstall-elevated\n"
    ));
    // An old updater exits with a usage error: no tokens, no chain, and
    // the install falls back to pointing at the release page.
    assert!(!capabilities_cover_elevation(
        b"usage: anterm-updater <command> [args]"
    ));
    assert!(!capabilities_cover_elevation(b""));
    // Two of the three verbs is not the chain.
    assert!(!capabilities_cover_elevation(
        b"elevated-stage\ninstall-elevated\n"
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn an_elevated_plan_names_every_piece_the_chain_needs() {
    let digest = "ab".repeat(32);
    let stage = PathBuf::from(r"C:\Users\someone\AppData\Local\Temp\anterm-update-x");
    let plan = |needs_elevation, expected_sha256: Option<&str>| PendingUpdate {
        version: "27.0.0".into(),
        apply_on_launch: false,
        plan_version: PLAN_VERSION,
        updater: stage.join("anterm-updater.exe"),
        command: "install".into(),
        // The order `prepare_windows_update` pushes: installer, checksums,
        // asset name, install dir, version, log, stage.
        rest: vec![
            stage.join("anterm-27.0.0-windows-x86_64-setup.exe"),
            stage.join("checksums.txt"),
            PathBuf::from("anterm-27.0.0-windows-x86_64-setup.exe"),
            PathBuf::from(r"C:\Program Files\anterm"),
            PathBuf::from("27.0.0"),
            stage.join("update.log"),
            stage.clone(),
        ],
        config_dir: Some(PathBuf::from(r"C:\Users\someone\.config\anterm")),
        stage: stage.clone(),
        needs_elevation,
        expected_sha256: expected_sha256.map(str::to_string),
    };

    let parts = plan(true, Some(&digest))
        .elevated_parts()
        .expect("a complete elevated plan yields its parts");
    assert_eq!(
        parts.installer,
        stage.join("anterm-27.0.0-windows-x86_64-setup.exe")
    );
    assert_eq!(parts.asset_name, "anterm-27.0.0-windows-x86_64-setup.exe");
    assert_eq!(parts.install_dir, PathBuf::from(r"C:\Program Files\anterm"));
    assert_eq!(parts.version, "27.0.0");
    assert_eq!(parts.expected_sha256, digest);

    // Not flagged, or flagged but missing the digest that anchors the
    // chain's trust, is not an elevated plan at all.
    assert!(plan(false, Some(&digest)).elevated_parts().is_none());
    assert!(plan(true, None).elevated_parts().is_none());
    let mut short = plan(true, Some(&digest));
    short.rest.truncate(6);
    assert!(short.elevated_parts().is_none());
}

#[cfg(target_os = "windows")]
#[test]
fn the_elevated_command_line_quotes_every_argument() {
    let parts = ElevatedPlanParts {
        installer: PathBuf::from(r"C:\Users\some one\stage\setup.exe"),
        asset_name: "anterm-27.0.0-windows-x86_64-setup.exe".into(),
        install_dir: PathBuf::from(r"C:\Program Files\anterm"),
        version: "27.0.0".into(),
        log: PathBuf::from(r"C:\Users\some one\stage\update.log"),
        stage: PathBuf::from(r"C:\Users\some one\stage"),
        expected_sha256: "ab".repeat(32),
    };
    let line = elevated_stage_arguments(
        &parts,
        Path::new(r"C:\Users\some one\.config\anterm\update-elevation.status"),
        Path::new(r"C:\Users\some one\.config\anterm\update-outcome.json"),
        Some(Path::new(r"C:\Users\some one\.config\anterm")),
    )
    .into_string()
    .expect("the test paths are valid Unicode");

    assert!(line.starts_with("\"elevated-stage\" "), "{line}");
    assert!(
        line.contains(&format!("\"{}\"", std::process::id())),
        "{line}"
    );
    // A space in a path must never reach the CRT splitter unquoted.
    assert!(line.contains(r#""C:\Program Files\anterm""#), "{line}");
    assert!(
        line.contains(r#""C:\Users\some one\stage\setup.exe""#),
        "{line}"
    );
    assert!(
        line.contains(&format!(
            "\"--expected-sha256\" \"{}\"",
            parts.expected_sha256
        )),
        "{line}"
    );
    assert!(
        line.contains(
            r#""--status-file" "C:\Users\some one\.config\anterm\update-elevation.status""#
        ),
        "{line}"
    );
    assert!(
        line.contains(r#""--config-dir" "C:\Users\some one\.config\anterm""#),
        "{line}"
    );
    assert!(
        line.contains(
            r#""--result-file" "C:\Users\some one\.config\anterm\update-outcome.json""#
        ),
        "{line}"
    );
}

/// The CRT splitter the elevated helper is parsed by treats a backslash
/// as an escape only in front of a quote — so a config directory ending
/// in one would otherwise escape its own closing quote and eat every
/// argument after it, including `--result-file`, which is the file the
/// watcher waits on.
#[cfg(target_os = "windows")]
#[test]
fn a_trailing_backslash_does_not_escape_its_own_closing_quote() {
    let quote = |value: &str| {
        let mut line = std::ffi::OsString::new();
        push_quoted(&mut line, std::ffi::OsStr::new(value));
        line.into_string().expect("valid Unicode in, valid out")
    };

    assert_eq!(
        quote(r"C:\Program Files\anterm"),
        r#""C:\Program Files\anterm""#
    );
    // Doubled only in front of the closing quote.
    assert_eq!(quote(r"D:\anterm\"), r#""D:\anterm\\""#);
    assert_eq!(quote(r"D:\anterm\\"), r#""D:\anterm\\\\""#);
    // An embedded quote is escaped, and the run in front of it doubled.
    assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
    assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
    // Interior backslashes are literal: doubling them would rename paths.
    assert_eq!(quote(r"a\b"), r#""a\b""#);

    let mut line = std::ffi::OsString::new();
    push_quoted(&mut line, std::ffi::OsStr::new("one"));
    push_quoted(&mut line, std::ffi::OsStr::new("two"));
    assert_eq!(line.into_string().unwrap(), r#""one" "two""#);
}
