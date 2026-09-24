//! The table a remote track's request is authorized through. A library row
//! stores a bare URL; the live source finishes it per request (a salted
//! Subsonic token, a bearer that expires), because credentials never belong
//! in a database file. The binary fills this at startup, like
//! `rox-panel-api`'s `openers`, so the lower layer never calls up.
//!
//! Headers and signed URLs are credentials: never log them or write them to
//! disk, here or anywhere downstream.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use rox_library::locator::Remote;

/// Sets headers, finishes the URL, or both. Called on whatever thread is
/// resolving a queue, possibly several at once, so it has to be cheap.
pub type AuthorizeFn = Box<dyn Fn(&mut Remote) + Send + Sync>;

fn table() -> &'static Mutex<HashMap<String, AuthorizeFn>> {
    static TABLE: OnceLock<Mutex<HashMap<String, AuthorizeFn>>> = OnceLock::new();

    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn install(source: &str, f: AuthorizeFn) {
    if let Ok(mut table) = table().lock() {
        table.insert(source.to_string(), f);
    }
}

/// A locator resolved after this goes out bare and the server refuses it.
pub fn forget(source: &str) {
    if let Ok(mut table) = table().lock() {
        table.remove(source);
    }
}

/// A no-op for a source nothing registered.
pub fn authorize(source: &str, remote: &mut Remote) {
    // A row pruned under a queued track has no URL. Leave it empty so the
    // open fails on that rather than on a signed request to nowhere.
    if remote.url.is_empty() {
        return;
    }

    // A poisoned lock means a source's builder panicked; send it bare.
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
