//! Where a track's bytes come from: a file on disk, or an HTTP stream with
//! the headers its source needs and a container hint (a URL has no
//! extension). Lives here so `rox-playback` and the store share one type.
//! A plugin locator names a stream the plugin serves by source and key, with
//! no URL: the plugin works out where the bytes live (ADR 29, ADR 30). A
//! placeholder holds a queue slot with nothing behind it yet.
//!
//! `path()` returns `Option` on purpose. Every path-only operation (tag
//! writer, rename, convert, ReplayGain, fingerprinting) has to decide what a
//! remote track means before it can reach one.

use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Locator {
    Local(PathBuf),
    Remote(Remote),
    Plugin(PluginStream),
    /// Nothing to open. The engine steps past it quietly, and whoever queued
    /// it swaps in a playable entry before it comes round (ADR 16).
    Placeholder,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Remote {
    pub url: String,
    /// Sent on every request for this track, range retries included.
    pub headers: Vec<(String, String)>,
    /// Container extension for the probe; empty means read Content-Type.
    pub hint: String,
    /// Internet radio: no end, no seek, no duration.
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PluginStream {
    /// `plugin:<id>`.
    pub source: String,
    /// The row's path, opaque to rox.
    pub key: String,
    /// No end, no seek, no duration.
    pub live: bool,
    /// The row's length, the last word on it when neither the container nor
    /// the plugin's open states one.
    pub duration_ms: Option<u32>,
}

impl Locator {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Locator::Local(path) => Some(path.as_path()),

            Locator::Remote(_) | Locator::Plugin(_) | Locator::Placeholder => None,
        }
    }

    /// A stream with no end: no seek, no duration, no position to restore.
    pub fn live(&self) -> bool {
        match self {
            Locator::Local(_) => false,

            Locator::Remote(remote) => remote.live,

            Locator::Plugin(stream) => stream.live,

            Locator::Placeholder => false,
        }
    }

    /// Display fallback when tags are missing: the file name, the last URL
    /// segment (the host when none survives), or a plugin's key.
    pub fn label(&self) -> String {
        match self {
            Locator::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),

            Locator::Remote(remote) => remote_label(&remote.url),

            Locator::Plugin(stream) => stream.key.clone(),

            Locator::Placeholder => String::new(),
        }
    }
}

impl From<PathBuf> for Locator {
    fn from(path: PathBuf) -> Self {
        Locator::Local(path)
    }
}

fn remote_label(url: &str) -> String {
    let trimmed = url.split(['#', '?']).next().unwrap_or(url);

    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);

    let mut segments = after_scheme.split('/').filter(|s| !s.is_empty());
    let host = segments.next().unwrap_or("");

    match segments.next_back() {
        Some(last) => last.to_string(),

        None => host.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(url: &str) -> Locator {
        Locator::Remote(Remote {
            url: url.to_string(),
            headers: Vec::new(),
            hint: String::new(),
            live: false,
        })
    }

    #[test]
    fn path_is_some_for_local_and_none_for_remote() {
        let local = Locator::from(PathBuf::from("/music/a.flac"));
        assert_eq!(local.path(), Some(Path::new("/music/a.flac")));

        assert_eq!(remote("https://host/stream.mp3").path(), None);
    }

    #[test]
    fn label_falls_back_to_the_last_url_segment() {
        assert_eq!(remote("https://host/rest/stream.mp3").label(), "stream.mp3");

        assert_eq!(remote("https://host/radio/jazz/").label(), "jazz");

        assert_eq!(remote("https://host/stream?id=7&fmt=raw").label(), "stream");

        assert_eq!(
            remote("http://stream.example.com").label(),
            "stream.example.com"
        );
    }

    #[test]
    fn a_plugin_stream_has_no_path_and_labels_by_its_key() {
        let stream = Locator::Plugin(PluginStream {
            source: "plugin:demo".into(),
            key: "a.flac".into(),
            live: false,
            duration_ms: None,
        });

        assert_eq!(stream.path(), None);
        assert_eq!(stream.label(), "a.flac");
        assert!(!stream.live());
    }

    #[test]
    fn label_of_a_local_track_is_its_file_name() {
        let local = Locator::from(PathBuf::from("/music/artist/01_song.flac"));
        assert_eq!(local.label(), "01_song.flac");
    }
}
