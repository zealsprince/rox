//! The update check: ask GitHub for the newest published release, weigh its
//! tag against the running build, and cache the result in settings. A launch
//! runs it at most once a day when the toggle allows; the About window's
//! button checks regardless. [`updater`](crate::startup::updater) acts on
//! the answer.
//!
//! ## Release candidates
//!
//! A candidate is a GitHub prerelease tagged with a semver suffix
//! (`v1.25.0-rc.1`), ordered the way semver says: above every `1.24.x`,
//! below `1.25.0`. Candidates stay out of the check unless the user opts in,
//! except that a candidate build always sees them, so `rc.1` learns about
//! `rc.2` and then the stable release.

use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::Deserialize;

use rox_core::settings::{Settings, UpdateCache};
use rox_net::providers::agent;

use crate::startup::updater;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The list rather than the "latest" endpoint, which hides prereleases.
const RELEASES: &str = "https://api.github.com/repos/zealsprince/rox/releases?per_page=10";

const CHECK_INTERVAL: u64 = 24 * 60 * 60;

/// The version the menubar chip announces, a live static because the
/// menubar reads it per frame. Shown even to installs that can't update
/// themselves.
static AVAILABLE: RwLock<Option<String>> = RwLock::new(None);

pub fn available() -> Option<String> {
    AVAILABLE.read().unwrap().clone()
}

/// Runs when the cache or the dismissal moves, never per frame.
pub fn refresh_available(settings: &Settings) {
    let version = settings
        .session
        .update_cache
        .as_ref()
        .filter(|cache| {
            let release = Release {
                version: cache.latest.clone(),
                url: cache.url.clone(),
                assets: Vec::new(),
            };
            release.offered(settings)
                && settings.session.update_dismissed.as_deref() != Some(cache.latest.as_str())
        })
        .map(|cache| cache.latest.clone());
    *AVAILABLE.write().unwrap() = version;
}

/// Remembered across restarts; a newer release brings the chip back.
pub fn dismiss(version: String) {
    Settings::update(move |s| s.session.update_dismissed = Some(version));
    *AVAILABLE.write().unwrap() = None;
}

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    /// Empty on a release rebuilt from the settings cache; the updater
    /// refetches.
    pub assets: Vec<Asset>,
}

#[derive(Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub bytes: u64,
}

impl Release {
    /// A tag that doesn't parse reads as not newer, so a bad cache never
    /// prompts an update.
    pub fn is_new(&self) -> bool {
        is_newer(&self.version, CURRENT).unwrap_or(false)
    }

    pub fn is_prerelease(&self) -> bool {
        is_prerelease(&self.version)
    }

    /// The cache can hold a candidate from a check made with the toggle on, so
    /// the chip asks this rather than [`Self::is_new`].
    pub fn offered(&self, settings: &Settings) -> bool {
        self.is_new() && (!self.is_prerelease() || wants_prereleases(settings))
    }
}

/// A candidate build always wants candidates, or it would sit on `rc.1`
/// while `rc.2` fixed its bugs.
pub fn wants_prereleases(settings: &Settings) -> bool {
    settings.prerelease_updates || is_prerelease(CURRENT)
}

#[derive(Deserialize)]
struct Api {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Err when nothing published parses as a version, so callers never cache
/// a junk tag. Blocks: background executor only.
pub fn fetch_latest() -> Result<Release, String> {
    let text = agent()
        .get(RELEASES)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let listed: Vec<Api> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let (version, api) = pick(listed, wants_prereleases(&Settings::load()))
        .ok_or_else(|| "no published release carries a version tag".to_string())?;
    Ok(Release {
        version: version.to_string(),
        url: api.html_url,
        assets: api
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                bytes: a.size,
            })
            .collect(),
    })
}

/// GitHub's prerelease flag and the tag's suffix both count, so a release
/// flagged by hand hides with the candidates.
fn pick(listed: Vec<Api>, include_prereleases: bool) -> Option<(Version, Api)> {
    listed
        .into_iter()
        .filter(|api| !api.draft)
        .filter_map(|api| {
            let version = Version::parse(api.tag_name.trim_start_matches('v')).ok()?;
            let prerelease = api.prerelease || !version.pre.is_empty();
            (include_prereleases || !prerelease).then_some((version, api))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
}

/// A failed fetch leaves the old cache and its timestamp alone, so the next
/// launch retries. With auto-download on, a newer release rolls straight
/// into the updater where the install can update itself.
pub fn check_on_launch(cx: &mut gpui::App) {
    let settings = Settings::load();
    // Seed the chip even when no check is due.
    refresh_available(&settings);
    if !auto_check_due(&settings) {
        return;
    }
    let auto_download = settings.download_updates;
    let check = cx.background_executor().spawn(async move {
        match fetch_latest() {
            Ok(release) => {
                Settings::update(|s| s.session.update_cache = Some(cache(&release)));
                refresh_available(&Settings::load());
                if auto_download
                    && release.is_new()
                    && updater::can_update()
                    && let Some(job) = updater::begin(&release)
                {
                    job();
                }
            }
            Err(e) => log::warn!("update check: {e}"),
        }
    });
    // The chip's static is outside gpui's reactivity, so repaint by hand.
    cx.spawn(async move |cx| {
        check.await;
        cx.refresh().ok();
    })
    .detach();
}

pub fn cache(release: &Release) -> UpdateCache {
    UpdateCache {
        checked_at: now(),
        latest: release.version.clone(),
        url: release.url.clone(),
    }
}

fn auto_check_due(settings: &Settings) -> bool {
    settings.check_updates
        && settings
            .session
            .update_cache
            .as_ref()
            .is_none_or(|c| now().saturating_sub(c.checked_at) >= CHECK_INTERVAL)
}

/// Zero before the epoch, which just makes the next check due.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// None when either doesn't parse, so "nightly" doesn't sort as zero.
fn is_newer(latest: &str, current: &str) -> Option<bool> {
    Some(Version::parse(latest).ok()? > Version::parse(current).ok()?)
}

fn is_prerelease(version: &str) -> bool {
    Version::parse(version).is_ok_and(|v| !v.pre.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_versions() {
        assert_eq!(is_newer("1.2.0", "1.1.9"), Some(true));
        assert_eq!(is_newer("1.1.10", "1.1.9"), Some(true));
        assert_eq!(is_newer("1.1.2", "1.1.2"), Some(false));
        assert_eq!(is_newer("1.0.0", "1.1.0"), Some(false));
        assert_eq!(is_newer("nightly", "1.1.2"), None);
    }

    #[test]
    fn candidates_sort_below_their_release_and_above_the_last_one() {
        assert_eq!(is_newer("1.25.0-rc.1", "1.24.9"), Some(true));
        assert_eq!(is_newer("1.25.0-rc.1", "1.25.0"), Some(false));
        assert_eq!(is_newer("1.25.0", "1.25.0-rc.1"), Some(true));
        assert_eq!(is_newer("1.25.0-rc.2", "1.25.0-rc.1"), Some(true));
        assert_eq!(is_newer("1.25.0-rc.10", "1.25.0-rc.9"), Some(true));
        assert!(is_prerelease("1.25.0-rc.1"));
        assert!(!is_prerelease("1.25.0"));
        assert!(!is_prerelease("nightly"));
    }

    fn listed(tag: &str, draft: bool, prerelease: bool) -> Api {
        Api {
            tag_name: tag.to_string(),
            html_url: format!("https://github.com/zealsprince/rox/releases/tag/{tag}"),
            draft,
            prerelease,
            assets: Vec::new(),
        }
    }

    #[test]
    fn picks_by_version_and_toggle() {
        let releases = || {
            vec![
                listed("v1.25.0-rc.1", false, true),
                listed("v1.24.0", false, false),
                listed("v1.26.0", true, false),
                listed("v1.23.6", false, false),
                listed("nightly", false, false),
            ]
        };
        let (stable, _) = pick(releases(), false).unwrap();
        assert_eq!(stable.to_string(), "1.24.0");
        let (candidate, api) = pick(releases(), true).unwrap();
        assert_eq!(candidate.to_string(), "1.25.0-rc.1");
        assert!(api.html_url.ends_with("v1.25.0-rc.1"));
        let flagged = vec![
            listed("v1.24.1", false, true),
            listed("v1.24.0", false, false),
        ];
        assert_eq!(pick(flagged, false).unwrap().0.to_string(), "1.24.0");
        assert!(pick(vec![listed("nightly", false, false)], true).is_none());
    }

    /// A real API answer from 2026-09-05, so a renamed field fails here first.
    #[test]
    fn parses_the_listing_as_github_sends_it() {
        let text = r#"[
          {
            "tag_name": "v1.24.0",
            "html_url": "https://github.com/zealsprince/rox/releases/tag/v1.24.0",
            "draft": false,
            "prerelease": false,
            "assets": [
              {
                "name": "rox-v1.24.0-linux-x86_64.tar.gz",
                "browser_download_url": "https://github.com/zealsprince/rox/releases/download/v1.24.0/rox-v1.24.0-linux-x86_64.tar.gz",
                "size": 46561633
              },
              {
                "name": "SHA256SUMS.txt",
                "browser_download_url": "https://github.com/zealsprince/rox/releases/download/v1.24.0/SHA256SUMS.txt",
                "size": 481
              }
            ]
          },
          {
            "tag_name": "v1.23.6",
            "html_url": "https://github.com/zealsprince/rox/releases/tag/v1.23.6",
            "draft": false,
            "prerelease": false,
            "assets": []
          }
        ]"#;
        let listed: Vec<Api> = serde_json::from_str(text).unwrap();
        let (version, api) = pick(listed, false).unwrap();
        assert_eq!(version.to_string(), "1.24.0");
        assert_eq!(api.assets.len(), 2);
        assert_eq!(api.assets[0].name, "rox-v1.24.0-linux-x86_64.tar.gz");
        assert_eq!(api.assets[0].size, 46561633);
    }
}
