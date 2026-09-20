//! The table a remote track's request is authorized through. A row in the
//! library says where a track's bytes live (`remote_url`) and whether the
//! stream ever ends, but not what makes the request acceptable, because the
//! answer changes per request: a Subsonic token is salted per call, a
//! bearer expires, and a password has no business sitting in a database
//! file the user can copy off the machine.
//!
//! So the store hands back a locator with a bare URL and no headers, and
//! something asks the live source to finish it. That something can't be the
//! store: the library crate knows nothing about a source object, and a
//! source lives a layer above it. The shape is `rox-panel-api`'s `openers`
//! table, for the same reason: the binary fills it once at startup and
//! everything below calls through it, so the lower layer never calls up.
//!
//! Finishing a request is both halves of it, which is why one call does
//! both. A source that authorizes in a header sets one; Subsonic signs the
//! query string, so its answer is a longer URL and no headers at all. A
//! source needs one look at its settings to answer either, and a resolve
//! walks a whole queue, so splitting the two would read those settings
//! twice per track for no gain.
//!
//! Headers and signed URLs are credentials. They're never logged and never
//! written to disk, here or anywhere downstream of here.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use rox_library::locator::Remote;

/// What a source does to one request so the server will serve it: set the
/// headers it needs, finish the URL it needs finished, or both. Called on
/// whatever thread is resolving a queue, so it has to be cheap and it has
/// to be safe to call from more than one at once.
pub type AuthorizeFn = Box<dyn Fn(&mut Remote) + Send + Sync>;

fn table() -> &'static Mutex<HashMap<String, AuthorizeFn>> {
    static TABLE: OnceLock<Mutex<HashMap<String, AuthorizeFn>>> = OnceLock::new();

    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register how one source authorizes. Installed when a source comes up and
/// replaced when its settings change, so a re-login swaps the credentials
/// without anything else noticing.
pub fn install(source: &str, f: AuthorizeFn) {
    if let Ok(mut table) = table().lock() {
        table.insert(source.to_string(), f);
    }
}

/// Forget how a source authorizes, for a source the user turned off. A
/// locator resolved after this goes out bare and the server refuses it,
/// which is the honest outcome.
pub fn forget(source: &str) {
    if let Ok(mut table) = table().lock() {
        table.remove(source);
    }
}

/// Finish a remote locator so the request can actually be served: the
/// headers this source needs set on it and its URL signed, by whatever
/// that source's scheme is.
///
/// A no-op for a source nothing registered, which is not an error: local
/// tracks never come through here, a station's URL is already whole, and a
/// source that needs no authorization has nothing to add.
pub fn authorize(source: &str, remote: &mut Remote) {
    // A row the source pruned out from under a queued track resolves to no
    // URL at all. There's nothing to authorize and nowhere to send it, so
    // it stays empty and the open fails on that rather than on a signed
    // request to nowhere.
    if remote.url.is_empty() {
        return;
    }

    // A poisoned lock means a source's own builder panicked. Nothing here
    // is worth taking the app down over, so the request goes out bare and
    // the server refuses it.
    if let Ok(table) = table().lock()
        && let Some(f) = table.get(source)
    {
        f(remote);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(url: &str) -> Remote {
        Remote {
            url: url.to_string(),
            headers: Vec::new(),
            hint: String::new(),
            live: false,
        }
    }

    #[test]
    fn a_source_reads_back_what_it_installed() {
        install(
            "test:read-back",
            Box::new(|remote| {
                remote
                    .headers
                    .push(("Authorization".to_string(), "Bearer abc".to_string()));
            }),
        );

        let mut locator = remote("https://srv/one");
        authorize("test:read-back", &mut locator);
        assert_eq!(
            locator.headers,
            vec![("Authorization".to_string(), "Bearer abc".to_string())]
        );

        forget("test:read-back");
        let mut locator = remote("https://srv/one");
        authorize("test:read-back", &mut locator);
        assert!(locator.headers.is_empty());
    }

    /// The half Subsonic needs: the source rewrites the URL rather than
    /// setting a header, and the resolve path has to carry that back.
    #[test]
    fn a_source_that_signs_the_url_gets_its_rewrite_back() {
        install(
            "test:signs",
            Box::new(|remote| remote.url = format!("{}&t=tok&s=salt", remote.url)),
        );

        let mut locator = remote("https://srv/rest/stream.view?id=1");
        authorize("test:signs", &mut locator);

        assert_eq!(
            locator.url,
            "https://srv/rest/stream.view?id=1&t=tok&s=salt"
        );
        forget("test:signs");
    }

    /// A queued row the source has since pruned resolves to nothing. Signing
    /// that would build a request out of an empty string; it stays empty so
    /// the failure names the missing row.
    #[test]
    fn an_empty_url_is_left_alone() {
        install(
            "test:pruned",
            Box::new(|remote| remote.url = format!("{}&t=tok", remote.url)),
        );

        let mut locator = remote("");
        authorize("test:pruned", &mut locator);

        assert!(locator.url.is_empty());
        forget("test:pruned");
    }

    #[test]
    fn an_unknown_source_is_left_bare() {
        let mut locator = remote("https://srv/one");
        authorize("test:never-installed", &mut locator);

        assert!(locator.headers.is_empty());
        assert_eq!(locator.url, "https://srv/one");
    }
}
