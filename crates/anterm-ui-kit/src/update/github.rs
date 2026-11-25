//! GitHub Releases API client, nightly manifest resolution, release asset
//! matching, and semantic/nightly version comparison.

use anyhow::{Context as _, Result};

use crate::config::UpdateChannel;

use super::{
    NIGHTLY_MANIFEST, NIGHTLY_TAG, PackageOffer, REPO, UpdateInstallHint, now_secs,
    package_for_current_install,
};

#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct LatestRelease {
    pub(super) tag_name: String,
    pub(super) assets: Vec<GitHubAsset>,
    #[serde(default)]
    pub(super) draft: bool,
    #[serde(default)]
    pub(super) prerelease: bool,
}

impl LatestRelease {
    pub(super) fn is_stable_desktop(&self) -> bool {
        !self.draft
            && !self.prerelease
            && parse_version(&self.tag_name).is_some_and(|version| version.3)
            && self.assets.iter().any(|asset| {
                version_from_assets(std::slice::from_ref(asset))
                    .is_some_and(|version| parse_version(&version) == parse_version(&self.tag_name))
            })
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct GitHubAsset {
    pub(super) name: String,
    pub(super) browser_download_url: String,
}

#[derive(serde::Deserialize)]
pub(super) struct GitHubError {
    message: String,
}

/// `nightly.json`, written by the nightly workflow. Only `version` is read
/// today; the rest is there so a build can be traced back to its commit
/// without cross-referencing the release notes.
#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct NightlyManifest {
    pub(super) version: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) commit: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) published_at: String,
}

/// The GitHub endpoint each channel reads.
///
/// Keeping the two feeds apart is the whole point of the channel: neither can
/// hand the other an update, so an installation only changes channel when the
/// user changes it in Settings.
pub(super) fn release_endpoint(channel: UpdateChannel) -> String {
    match channel {
        // Excludes prereleases by definition, so Stable can never be offered a
        // nightly even though both live in the same repository.
        UpdateChannel::Stable => format!("https://api.github.com/repos/{REPO}/releases/latest"),
        UpdateChannel::Nightly => {
            format!("https://api.github.com/repos/{REPO}/releases/tags/{NIGHTLY_TAG}")
        }
    }
}

/// Recovers the version from a package name such as
/// `anterm-26.8.2-nightly.202608071800-macos-arm64.zip`.
///
/// The platform segment is a closed set, which is what makes the split
/// unambiguous — the version is everything between the `anterm-` prefix and the
/// platform marker. `anterm-server-linux-x86_64-musl` matches that shape too but
/// yields `server`, which `parse_version` rejects, so the remote-server assets
/// sitting in the same release are skipped without special-casing them.
///
/// The highest version wins rather than the first one found. Two nights can be
/// on the release at once: the workflow uploads tonight's packages before
/// pruning yesterday's, and a prune that never ran leaves them there for good.
/// GitHub lists assets oldest first, so taking the first match is taking the
/// older build — which reads as "no update" and stalls the channel silently.
pub(super) fn version_from_assets(assets: &[GitHubAsset]) -> Option<String> {
    assets
        .iter()
        .filter_map(|asset| {
            let rest = asset.name.strip_prefix("anterm-")?;
            let cut = ["-macos-", "-linux-", "-windows-"]
                .iter()
                .find_map(|marker| rest.find(marker))?;
            let version = &rest[..cut];
            Some((parse_version(version)?, version.to_string()))
        })
        .max()
        .map(|(_, version)| version)
}

pub(super) fn build_http_client(manual_proxy: Option<&str>) -> Result<ureq::Agent> {
    let user_agent = concat!("anterm/", env!("CARGO_PKG_VERSION"));
    let mut config = ureq::Agent::config_builder()
        .user_agent(user_agent)
        .http_status_as_error(false);
    // Normalise through the same helper the downloader uses, so a bare
    // `127.0.0.1:7890` proxies the check as well as the download.
    if let Some(proxy) = manual_proxy
        .and_then(anterm_core::daemon::install::proxy::normalize_manual)
        .and_then(|url| ureq::Proxy::new(&url).ok())
    {
        config = config.proxy(Some(proxy));
    }
    Ok(config.build().new_agent())
}

async fn fetch_json<T: serde::de::DeserializeOwned + Send + 'static>(
    client: &ureq::Agent,
    url: &str,
) -> Result<T> {
    let client = client.clone();
    let url = url.to_string();
    smol::unblock(move || {
        let mut response = client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .context("sending the request")?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let rate_remaining = response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let rate_reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response
                .body_mut()
                .with_config()
                .limit(8 * 1024)
                .read_to_vec()
                .unwrap_or_default();
            anyhow::bail!(github_http_error(
                status,
                rate_remaining.as_deref(),
                rate_reset.as_deref(),
                &body,
                now_secs(),
            ));
        }

        let body = response
            .body_mut()
            .read_to_vec()
            .context("reading response body")?;

        serde_json::from_slice(&body).context("parsing JSON")
    })
    .await
}

pub(super) fn github_http_error(
    status: u16,
    rate_remaining: Option<&str>,
    rate_reset: Option<&str>,
    body: &[u8],
    now: u64,
) -> String {
    let message = serde_json::from_slice::<GitHubError>(body)
        .ok()
        .map(|error| sanitize_github_message(&error.message));
    // 403 is what the REST API has always answered a spent quota with; 429 is
    // what it increasingly answers instead, and both carry the same headers.
    let rate_limited = matches!(status, 403 | 429)
        && (rate_remaining == Some("0")
            || message.as_deref().is_some_and(|message| {
                message.to_ascii_lowercase().contains("rate limit exceeded")
            }));

    if rate_limited {
        let retry = rate_reset
            .and_then(|reset| reset.parse::<u64>().ok())
            .filter(|reset| *reset > now)
            .map(|reset| {
                let minutes = (reset - now).div_ceil(60);
                let suffix = if minutes == 1 { "" } else { "s" };
                format!("try again in about {minutes} minute{suffix}")
            })
            .unwrap_or_else(|| "try again shortly".to_string());
        return format!("GitHub API rate limit exceeded; {retry} (HTTP {status})");
    }

    match message.filter(|message| !message.is_empty()) {
        Some(message) => format!("GitHub returned HTTP {status}: {message}"),
        None => format!("GitHub returned HTTP {status}"),
    }
}

pub(super) fn sanitize_github_message(message: &str) -> String {
    let single_line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = single_line.chars();
    let shortened: String = chars.by_ref().take(200).collect();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

/// Returns the release together with the version it advertises, which is not
/// always something the release object states outright — see `resolve_version`.
pub(super) async fn fetch_latest_release(
    channel: UpdateChannel,
    manual_proxy: Option<String>,
) -> Result<(LatestRelease, String)> {
    let client = build_http_client(manual_proxy.as_deref())?;
    let mut release: LatestRelease = fetch_json(&client, &release_endpoint(channel))
        .await
        .context("requesting the release")?;
    if channel == UpdateChannel::Stable && !release.is_stable_desktop() {
        // Mobile releases share this repository and can accidentally be marked
        // latest when published. Walk past those (and drafts/nightlies), rather
        // than trying to install a phone app or stranding the desktop updater.
        release = find_stable_desktop_release(async |page| {
            fetch_json(
                &client,
                &format!("https://api.github.com/repos/{REPO}/releases?per_page=100&page={page}"),
            )
            .await
            .context("requesting desktop releases")
        })
        .await?;
    }
    let version = resolve_version(&client, &release)
        .await
        .with_context(|| format!("release {} advertises no usable version", release.tag_name))?;
    Ok((release, version))
}

pub(super) async fn find_stable_desktop_release(
    mut fetch_page: impl AsyncFnMut(usize) -> Result<Vec<LatestRelease>>,
) -> Result<LatestRelease> {
    let mut page = 1;
    loop {
        let releases = fetch_page(page).await?;
        let last_page = releases.len() < 100;
        if let Some(desktop) = releases.into_iter().find(LatestRelease::is_stable_desktop) {
            return Ok(desktop);
        }
        anyhow::ensure!(!last_page, "no stable desktop release is available");
        page += 1;
    }
}

/// The version a release stands for.
///
/// Stable states it in the tag (`v26.8.1`) and needs nothing else. Nightly
/// cannot: its tag is force-moved to a new commit every night and so is the
/// literal string `nightly`, which carries no version at all. It publishes
/// `nightly.json` beside the packages instead.
///
/// The fall back to asset names covers the two cases where the manifest is
/// absent — a nightly published before it existed, and a night where writing it
/// failed — because neither is a reason to strand the whole channel.
pub(super) async fn resolve_version(client: &ureq::Agent, release: &LatestRelease) -> Option<String> {
    if parse_version(&release.tag_name).is_some() {
        return Some(release.tag_name.trim_start_matches('v').to_string());
    }
    if let Some(asset) = release
        .assets
        .iter()
        .find(|asset| asset.name == NIGHTLY_MANIFEST)
    {
        match fetch_json::<NightlyManifest>(client, &asset.browser_download_url).await {
            Ok(manifest) if parse_version(&manifest.version).is_some() => {
                return Some(manifest.version);
            }
            Ok(manifest) => log::warn!(
                "{NIGHTLY_MANIFEST} declares an unusable version {:?}; falling back to asset names",
                manifest.version
            ),
            Err(e) => log::warn!("could not read {NIGHTLY_MANIFEST}: {e:#}"),
        }
    }
    version_from_assets(&release.assets)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReleaseAsset {
    pub(super) name: String,
    pub(super) url: String,
    pub(super) checksums_url: String,
}

pub(super) struct AssetSelection {
    pub(super) asset: Option<ReleaseAsset>,
    pub(super) reason: Option<UpdateInstallHint>,
    #[cfg(target_os = "windows")]
    pub(super) needs_elevation: bool,
}

pub(super) fn select_release_asset(version: &str, assets: &[GitHubAsset]) -> AssetSelection {
    select_release_asset_for(package_for_current_install(version), assets)
}

pub(super) fn select_release_asset_for(
    package: Result<PackageOffer, UpdateInstallHint>,
    assets: &[GitHubAsset],
) -> AssetSelection {
    let offer = match package {
        Ok(offer) => offer,
        Err(reason) => {
            return AssetSelection {
                asset: None,
                reason: Some(reason),
                #[cfg(target_os = "windows")]
                needs_elevation: false,
            };
        }
    };
    let name = offer.name;
    let Some(asset) = assets.iter().find(|asset| asset.name == name) else {
        return AssetSelection {
            asset: None,
            reason: Some(UpdateInstallHint::MissingPackage(name)),
            #[cfg(target_os = "windows")]
            needs_elevation: false,
        };
    };
    let Some(checksums) = assets.iter().find(|asset| asset.name == "checksums.txt") else {
        return AssetSelection {
            asset: None,
            reason: Some(UpdateInstallHint::MissingChecksums),
            #[cfg(target_os = "windows")]
            needs_elevation: false,
        };
    };
    AssetSelection {
        asset: Some(ReleaseAsset {
            name,
            url: asset.browser_download_url.clone(),
            checksums_url: checksums.browser_download_url.clone(),
        }),
        reason: None,
        #[cfg(target_os = "windows")]
        needs_elevation: offer.needs_elevation,
    }
}

/// `(major, minor, patch, is_release, build)`.
///
/// Ordering the release flag *before* the build number is what lets a stable
/// release supersede the prerelease that carries the same core version: a
/// Nightly stamped `26.7.1-nightly.202607161800` is offered `v26.7.1` and
/// graduates out of the prerelease no matter how recent its build is.
///
/// `build` then orders two prereleases sharing a core version against each
/// other, by the timestamp the nightly workflow stamps. That comparison is the
/// whole reason the Nightly channel can roll forward at all — without it every
/// nightly compares equal to the next one. It is unreachable on Stable, which
/// reads `/releases/latest` and so never sees a prerelease.
///
/// Every numeric identifier in the prerelease is collected, not just the last
/// one, and they are compared left to right the way semver compares them. The
/// nightly stamp is a single number today, but reading only the last segment
/// meant that appending one — a counter for a second build in the same day, a
/// finer clock — would silently *reverse* the ordering (`…20260807.2` scoring
/// 2) instead of failing where someone would notice. Non-numeric identifiers
/// are skipped rather than ranked: `-beta` and `-rc` are not versions this
/// project ships, and guessing an order between them buys nothing. A
/// prerelease with no number at all yields an empty list, which sorts below
/// every stamped build.
pub(super) fn parse_version(s: &str) -> Option<(u64, u64, u64, bool, Vec<u64>)> {
    let trimmed = s.trim();
    let core = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let without_meta = core.split('+').next().unwrap_or(core);
    let is_release = !without_meta.contains('-');
    let build = without_meta
        .split_once('-')
        .map(|(_, pre)| pre.split('.').filter_map(|id| id.parse().ok()).collect())
        .unwrap_or_default();
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, is_release, build))
}

pub(super) fn is_update_available(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

