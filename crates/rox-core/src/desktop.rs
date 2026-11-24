//! The calls that reach past rox's own windows onto the desktop: the native
//! file pickers, the browser, and the file manager. Every caller goes through
//! here so one switch can turn them all off.
//!
//! `ROX_NO_NATIVE_DIALOGS=1` is for automated runs. A picker is raised by the
//! desktop portal, so even a rox on a headless compositor puts it on the real
//! screen. With the switch set, a picker answers as a cancelled one does and
//! the others do nothing, each with one log line naming its caller. Unset,
//! everything here is a straight pass-through. What gpui does on its own, and
//! what rox puts on the session bus (media controls, the tray, taskbar
//! progress), is outside its reach.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::LazyLock;
use std::task::{Context, Poll};

use gpui::{App, PathPromptOptions};

pub const SWITCH: &str = "ROX_NO_NATIVE_DIALOGS";

static REFUSED: LazyLock<bool> = LazyLock::new(|| switched_on(std::env::var(SWITCH).ok()));

/// Set means anything but empty or `0`, so `=1` and `=true` both count.
fn switched_on(value: Option<String>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// Reads the switch and says so in the log when it's on. Call once at start,
/// so the first picker isn't what reads the environment.
pub fn init() {
    if *REFUSED {
        log::info!("{SWITCH} is set: native dialogs, links and reveals are refused");
    }
}

pub fn native_dialogs_refused() -> bool {
    *REFUSED
}

/// The line a refusal logs, or None when the call should go ahead.
fn refusal(refused: bool, kind: &str, what: &str) -> Option<String> {
    refused.then(|| format!("native {kind} refused: {what}"))
}

fn refuse(kind: &str, what: &str) -> bool {
    match refusal(*REFUSED, kind, what) {
        Some(line) => {
            log::warn!("{line}");
            true
        }
        None => false,
    }
}

/// A picker's answer, or a cancelled one without asking. Awaits to the same
/// type gpui's receiver does, so every caller's cancel path is the one it
/// already has.
pub enum Picker<R> {
    Asked(R),
    Refused,
}

impl<R, T, E> Future for Picker<R>
where
    R: Future<Output = Result<gpui::Result<Option<T>>, E>> + Unpin,
{
    type Output = R::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Picker::Asked(rx) => Pin::new(rx).poll(cx),
            Picker::Refused => Poll::Ready(Ok(Ok(None))),
        }
    }
}

/// `cx.prompt_for_paths`. `what` names the caller in the refusal line.
pub fn prompt_for_paths(
    cx: &App,
    what: &str,
    options: PathPromptOptions,
) -> Picker<
    impl Future<Output = Result<gpui::Result<Option<Vec<PathBuf>>>, impl Sized + use<>>> + Unpin + use<>,
> {
    if refuse("dialog", what) {
        return Picker::Refused;
    }

    Picker::Asked(cx.prompt_for_paths(options))
}

/// `cx.prompt_for_new_path`. `what` names the caller in the refusal line.
pub fn prompt_for_new_path(
    cx: &App,
    what: &str,
    directory: &Path,
    suggested_name: Option<&str>,
) -> Picker<
    impl Future<Output = Result<gpui::Result<Option<PathBuf>>, impl Sized + use<>>> + Unpin + use<>,
> {
    if refuse("dialog", what) {
        return Picker::Refused;
    }

    Picker::Asked(cx.prompt_for_new_path(directory, suggested_name))
}

/// `cx.open_url`: the user's browser.
pub fn open_url(cx: &App, what: &str, url: &str) {
    if refuse("link", what) {
        return;
    }

    cx.open_url(url);
}

/// Whether a link from outside rox (an enrichment provider, a plugin) is one
/// rox hands to the browser: `https://` to a plain ASCII host with an optional
/// port. Refused: any other scheme (`file:`, `javascript:`, a custom handler),
/// credentials before the host, an IP literal in brackets, and a space, a
/// control character or anything non-ASCII anywhere in the link. The OS opens
/// whatever it's given, so this check is the only one.
pub fn plain_https(url: &str) -> bool {
    if url.len() > LINK_MAX || !url.bytes().all(|b| b.is_ascii_graphic()) {
        return false;
    }

    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };

    let plain_host = !host.is_empty()
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let plain_port = port.is_none_or(|port| {
        (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit())
    });

    plain_host && plain_port
}

/// Past this a link is refused rather than handed on.
const LINK_MAX: usize = 2048;

/// `cx.reveal_path`: the file manager, at the path.
pub fn reveal_path(cx: &App, what: &str, path: &Path) {
    if refuse("reveal", what) {
        return;
    }

    cx.reveal_path(path);
}

/// `cx.open_with_system`: whatever program the desktop opens the path with.
pub fn open_with_system(cx: &App, what: &str, path: &Path) {
    if refuse("open", what) {
        return;
    }

    cx.open_with_system(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_on_for_anything_but_empty_or_zero() {
        assert!(!switched_on(None));
        assert!(!switched_on(Some(String::new())));
        assert!(!switched_on(Some("0".into())));
        assert!(switched_on(Some("1".into())));
        assert!(switched_on(Some("true".into())));
    }

    #[test]
    fn a_refusal_names_its_caller_and_an_allowed_call_logs_nothing() {
        assert_eq!(
            refusal(true, "dialog", "library: add folder").as_deref(),
            Some("native dialog refused: library: add folder")
        );
        assert_eq!(refusal(false, "dialog", "library: add folder"), None);
    }

    #[test]
    fn only_a_plain_https_link_is_handed_on() {
        for good in [
            "https://www.last.fm/music/Other",
            "https://en.wikipedia.org/wiki/Other_%28band%29",
            "https://host:8443/a?b=c#d",
        ] {
            assert!(plain_https(good), "{good}");
        }

        for bad in [
            "http://www.last.fm/music/Other",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "https://user:pass@host/",
            "https://[::1]/",
            "https:///path",
            "https://-host/",
            "https://host:99999999/",
            "https://host/a b",
            "https://h\u{f6}st/",
            "steam://run/1",
            "",
        ] {
            assert!(!plain_https(bad), "{bad}");
        }

        let long = format!("https://host/{}", "a".repeat(LINK_MAX));
        assert!(!plain_https(&long));
    }

    fn poll_once<F: Future + Unpin>(mut future: F) -> Poll<F::Output> {
        let mut cx = Context::from_waker(std::task::Waker::noop());
        Pin::new(&mut future).poll(&mut cx)
    }

    #[test]
    fn a_refused_picker_answers_as_a_cancelled_one() {
        type Answer = Result<gpui::Result<Option<Vec<PathBuf>>>, ()>;
        let refused: Picker<std::future::Ready<Answer>> = Picker::Refused;

        match poll_once(refused) {
            Poll::Ready(Ok(Ok(None))) => {}
            _ => panic!("a refused picker should answer Ok(Ok(None)) at once"),
        }
    }

    #[test]
    fn an_asked_picker_passes_the_answer_through() {
        let picked = vec![PathBuf::from("/m/a.flac")];
        let answer: Result<gpui::Result<Option<Vec<PathBuf>>>, ()> = Ok(Ok(Some(picked.clone())));
        let asked = Picker::Asked(std::future::ready(answer));

        match poll_once(asked) {
            Poll::Ready(Ok(Ok(Some(paths)))) => assert_eq!(paths, picked),
            _ => panic!("an asked picker should hand back what the picker said"),
        }
    }
}
