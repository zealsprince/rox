//! The update check: ask GitHub for the newest published release and weigh
//! its tag against the running build. The check itself only reports what it
//! found (a newer release, its page, its artifacts) and caches the result in
//! settings; a launch runs it at most once a day, and only when the
//! settings toggle leaves it on. The About page's button checks now
//! regardless. [`updater`](crate::startup::updater) acts on the answer,
//! called from the About page or, opted in, straight from the launch check
//! here.

use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use rox_core::settings::{Settings, UpdateCache};
use rox_net::providers::agent;

use crate::startup::updater;

/// The build's own version, the left side of every comparison.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// GitHub's "latest" endpoint points at the newest published, non-draft,
/// non-prerelease release, which is exactly what a stable tag push publishes.
const LATEST: &str = "https://api.github.com/repos/zealsprince/rox/releases/latest";

/// How long a cached check is good for before a launch runs another: a day.
const CHECK_INTERVAL: u64 = 24 * 60 * 60;

/// The version the menubar's "Update Available" chip announces, a live
/// static like the palette's flags: the menubar reads it per frame, where a
/// settings-file load has no place. Some only when the cached check found a
/// newer release that hasn't been dismissed; installs that can't replace
/// themselves see it too, since knowing a release exists doesn't need the
/// updater. Seeded at launch and refreshed when a check lands or the chip
/// is dismissed.
static AVAILABLE: RwLock<Option<String>> = RwLock::new(None);

/// What the menubar chip shows, if anything.
pub fn available() -> Option<String> {
    AVAILABLE.read().unwrap().clone()
}

/// Recompute the chip's static from the settings on hand: the cached
/// release against the running build and the dismissal. Runs when the
/// cache or the dismissal moves, never per frame.
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
            release.is_new()
                && settings.session.update_dismissed.as_deref() != Some(cache.latest.as_str())
        })
        .map(|cache| cache.latest.clone());
    *AVAILABLE.write().unwrap() = version;
}

/// Put the chip away for this release: remember the version so it stays
/// dismissed across restarts, and clear the live static. A newer release
/// brings the chip back on its own.
pub fn dismiss(version: String) {
    Settings::update(move |s| s.session.update_dismissed = Some(version));
    *AVAILABLE.write().unwrap() = None;
}

/// A published release as the check reads it: the version its tag names,
/// the page a user opens to get it, and the files attached to it for the
/// updater to resolve against.
#[derive(Clone)]
pub struct Release {
    /// The tag's version, the leading v stripped: "1.2.0".
    pub version: String,
    /// The release page on GitHub, where the artifacts are published.
    pub url: String,
    /// The release's files. Empty on a release rebuilt from the settings
    /// cache, which stores none; the updater refetches when it needs them.
    pub assets: Vec<Asset>,
}

/// One file attached to a release.
#[derive(Clone)]
pub struct Asset {
    pub name: String,
    /// The direct download URL.
    pub url: String,
    pub bytes: u64,
}

impl Release {
    /// Whether this release is newer than the running build. A tag that
    /// somehow doesn't parse reads as not newer, so a bad cache never
    /// prompts an update.
    pub fn is_new(&self) -> bool {
        is_newer(&self.version, CURRENT).unwrap_or(false)
    }
}

/// Ask GitHub for the latest release. Err is the network or the API
/// failing, or a tag that doesn't parse as a version, so callers never
/// cache a junk tag. Background executor only, it blocks.
pub fn fetch_latest() -> Result<Release, String> {
    #[derive(Deserialize)]
    struct Api {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        assets: Vec<ApiAsset>,
    }
    #[derive(Deserialize)]
    struct ApiAsset {
        name: String,
        browser_download_url: String,
        size: u64,
    }
    // The shared agent already sets the app User-Agent the API requires;
    // the Accept header pins the versioned media type GitHub documents.
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

/// Run the daily check at launch if it's due, off the UI thread, caching
/// the result in settings. The toggle and the one-day spacing both gate
/// it, so a normal start usually does nothing. A failed fetch leaves the
/// old cache and its timestamp alone, so the next launch just retries.
///
/// With the download toggle opted in, a check that finds a newer release
/// rolls straight into the updater on the same background task, but only
/// where the install can update itself. A distro package or a read-only
/// home stays notify-only whatever the toggle says.
pub fn check_on_launch(cx: &mut gpui::App) {
    let settings = Settings::load();
    // Seed the menubar chip from the cache whether or not a check is due,
    // so a launch inside the one-day window still announces what the last
    // check found.
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
                if auto_download && release.is_new() && updater::can_update() {
                    if let Some(job) = updater::begin(&release) {
                        job();
                    }
                }
            }
            Err(e) => log::warn!("update check: {e}"),
        }
    });
    // Back on the foreground once the check settles: repaint the open
    // windows, since the chip's static is outside gpui's reactivity and
    // nothing else would wake an idle menubar.
    cx.spawn(async move |cx| {
        check.await;
        cx.refresh().ok();
    })
    .detach();
}

/// The cache entry a finished check writes: the release stamped with now.
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
            .session
            .update_cache
            .as_ref()
            .is_none_or(|c| now().saturating_sub(c.checked_at) >= CHECK_INTERVAL)
}

/// Now as unix seconds, the cache's clock. Zero if the system clock is set
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
