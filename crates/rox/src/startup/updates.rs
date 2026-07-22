//! The update check, notify only: ask GitHub for the newest published
//! release and weigh its tag against the running build. It never downloads
//! or installs - it reports a newer release and links to its page. The
//! result caches in settings; a launch runs the check at most once a day,
//! and only when the About page's toggle leaves it on. The button on that
//! page checks now regardless.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::providers::agent;
use crate::settings::{Settings, UpdateCache};

/// The build's own version, the left side of every comparison.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// GitHub's "latest" endpoint points at the newest published, non-draft,
/// non-prerelease release, which is exactly what a stable tag push lands.
const LATEST: &str = "https://api.github.com/repos/zealsprince/rox/releases/latest";

/// How long a cached check stands before a launch runs another: a day.
const CHECK_INTERVAL: u64 = 24 * 60 * 60;

/// A published release as the check reads it: the version its tag names
/// and the page a user opens to get it.
#[derive(Clone)]
pub struct Release {
    /// The tag's version, the leading v stripped: "1.2.0".
    pub version: String,
    /// The release page on GitHub, where the artifacts hang.
    pub url: String,
}

impl Release {
    /// Whether this release is newer than the running build. A tag that
    /// somehow doesn't parse reads as not newer, so a bad cache never
    /// nags.
    pub fn is_new(&self) -> bool {
        is_newer(&self.version, CURRENT).unwrap_or(false)
    }
}

/// Ask GitHub for the latest release. Err is the network or the API
/// failing, or a tag that doesn't parse as a version - callers never
/// cache a junk tag. Background executor only, it blocks.
pub fn fetch_latest() -> Result<Release, String> {
    #[derive(Deserialize)]
    struct Api {
        tag_name: String,
        html_url: String,
    }
    // The shared agent already carries the app User-Agent the API wants;
    // the Accept header pins the versioned media type GitHub asks for.
    let text = agent()
        .get(LATEST)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let api: Api = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let version = api.tag_name.trim_start_matches('v').to_string();
    if parts(&version).is_none() {
        return Err(format!("release tag {:?} isn't a version", api.tag_name));
    }
    Ok(Release {
        version,
        url: api.html_url,
    })
}

/// Run the daily check at launch if it's due, off the UI thread, caching
/// the result in settings. The toggle and the one-day spacing both gate
/// it, so a normal start usually does nothing. A failed fetch leaves the
/// old cache and its timestamp alone, so the next launch simply retries.
pub fn check_on_launch(cx: &mut gpui::App) {
    if !auto_check_due(&Settings::load()) {
        return;
    }
    cx.background_executor()
        .spawn(async {
            match fetch_latest() {
                Ok(release) => Settings::update(|s| s.update_cache = Some(cache(&release))),
                Err(e) => eprintln!("update check: {e}"),
            }
        })
        .detach();
}

/// The cache entry a landed check writes: the release stamped with now.
pub fn cache(release: &Release) -> UpdateCache {
    UpdateCache {
        checked_at: now(),
        latest: release.version.clone(),
        url: release.url.clone(),
    }
}

/// Whether a launch should run the check: the toggle is on and either
/// nothing has been checked or the last check is over a day old.
fn auto_check_due(settings: &Settings) -> bool {
    settings.check_updates
        && settings
            .update_cache
            .as_ref()
            .is_none_or(|c| now().saturating_sub(c.checked_at) >= CHECK_INTERVAL)
}

/// Now as unix seconds, the cache's clock. Zero if the system clock sits
/// before the epoch, which just makes the next check read as due.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether `latest` is a higher version than `current`, both plain
/// major.minor.patch. None when either doesn't parse. Tags and the build
/// version are always three parts, so the lists compare segment by
/// segment without padding.
fn is_newer(latest: &str, current: &str) -> Option<bool> {
    Some(parts(latest)? > parts(current)?)
}

/// A version string as a comparable list of numbers. None when a segment
/// isn't a number, so a tag like "nightly" reads as unparseable rather
/// than sorting as zero.
fn parts(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|n| n.parse().ok()).collect()
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
}
