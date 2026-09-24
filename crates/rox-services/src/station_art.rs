//! A station's logo, fetched once and filed in the thumbnail store under the
//! station's URL, the key every surface already asks art by. The directory
//! add and a played station's homepage both come through this one path, so
//! the download cap and the content-type check can't drift apart.
//!
//! Unlike [`crate::radio_art`]'s per-song guess, a logo is the station's own
//! and worth keeping on disk. Best effort and silent.

use std::io::Read;
use std::sync::Mutex;

use rox_library::rusqlite::Connection;

/// Past this the URL is answering with something that isn't a logo.
pub const MAX_BYTES: u64 = 512 * 1024;

/// Stations publish a homepage in `icy-url`, never a logo, so this is the
/// one guess available.
const FAVICON_PATH: &str = "/favicon.ico";

/// Anything that isn't a plain image answer is dropped: logo URLs are old
/// and often answer with a login page or a parked domain. Blocking.
pub fn fetch(url: &str) -> Option<Vec<u8>> {
    let response = rox_net::providers::agent().get(url).call().ok()?;
    if !(200..300).contains(&response.status()) {
        return None;
    }
    if !response.content_type().starts_with("image/") {
        return None;
    }

    // One byte past the cap, so a body that fills it is over the limit
    // rather than silently truncated into a corrupt image.
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;

    (!bytes.is_empty() && bytes.len() as u64 <= MAX_BYTES).then_some(bytes)
}

/// True when something landed: the caller's cue to forget the key in the
/// texture cache, which cached "no art" as definitive. Blocking, takes the
/// store lock.
pub fn fetch_and_store(image_url: &str, key: &str, conn: &Mutex<Connection>) -> bool {
    let Some(bytes) = fetch(image_url) else {
        return false;
    };
    let Ok(conn) = conn.lock() else {
        return false;
    };

    rox_library::thumbs::store_bytes(&conn, &bytes, key).is_some()
}

/// The origin with [`FAVICON_PATH`] on it, or None for anything that isn't
/// an http URL with a host. Hand-parsed: a URL crate's features are exactly
/// what gets thrown away here.
pub fn favicon_url(homepage: &str) -> Option<String> {
    let homepage = homepage.trim();
    let (scheme, rest) = homepage.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }

    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('.');

    // Never guess a credentialed URL.
    if authority.is_empty() || authority.contains('@') {
        return None;
    }

    Some(format!(
        "{}://{authority}{FAVICON_PATH}",
        scheme.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_homepage_reduces_to_its_origin() {
        assert_eq!(
            favicon_url("https://jazzforever.example/schedule?day=2"),
            Some("https://jazzforever.example/favicon.ico".to_string())
        );
        assert_eq!(
            favicon_url("http://radio.example:8000"),
            Some("http://radio.example:8000/favicon.ico".to_string())
        );
        assert_eq!(
            favicon_url("  HTTPS://Radio.Example/  "),
            Some("https://Radio.Example/favicon.ico".to_string())
        );
    }

    #[test]
    fn anything_that_is_not_a_web_address_is_no_guess_at_all() {
        assert_eq!(favicon_url(""), None);
        assert_eq!(favicon_url("The best jazz on the internet"), None);
        assert_eq!(favicon_url("ftp://files.example/logo.png"), None);
        assert_eq!(favicon_url("https://"), None);
        assert_eq!(favicon_url("https://user:pass@radio.example/"), None);
    }
}
