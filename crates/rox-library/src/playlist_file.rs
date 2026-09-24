//! One door in front of the playlist formats (ADR 16): M3U/M3U8, PLS, XSPF.
//!
//! Reading sniffs the content, since a `.m3u` holding a PLS body exists in
//! the wild. Writing takes the extension, the only signal on export.

use std::path::Path;

use crate::playlists::ExportTrack;

/// ASX and WPL are left out: mostly dead formats, a parser each.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    M3u,
    Pls,
    Xspf,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::M3u, Format::Pls, Format::Xspf];

    pub fn label(self) -> &'static str {
        match self {
            Format::M3u => "M3U",
            Format::Pls => "PLS",
            Format::Xspf => "XSPF",
        }
    }

    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?;
        if ext.eq_ignore_ascii_case("m3u") || ext.eq_ignore_ascii_case("m3u8") {
            Some(Format::M3u)
        } else if ext.eq_ignore_ascii_case("pls") {
            Some(Format::Pls)
        } else if ext.eq_ignore_ascii_case("xspf") {
            Some(Format::Xspf)
        } else {
            None
        }
    }

    /// `[playlist]` is PLS, `<` is XSPF, and everything else is M3U, which
    /// accepts a bare path list.
    pub fn sniff(text: &str) -> Format {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let first = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default();
        if first
            .get(..9)
            .is_some_and(|head| head.eq_ignore_ascii_case("[playlist"))
        {
            Format::Pls
        } else if first.starts_with('<') {
            Format::Xspf
        } else {
            Format::M3u
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::M3u => "m3u8",
            Format::Pls => "pls",
            Format::Xspf => "xspf",
        }
    }
}

pub const EXTENSIONS: &[&str] = &["m3u", "m3u8", "pls", "xspf"];

pub fn is_playlist_file(path: &Path) -> bool {
    Format::from_path(path).is_some()
}

pub fn parse(text: &str) -> Vec<String> {
    match Format::sniff(text) {
        Format::M3u => crate::m3u::parse(text),
        Format::Pls => crate::pls::parse(text),
        Format::Xspf => crate::xspf::parse(text),
    }
}

pub fn write(format: Format, rows: &[ExportTrack]) -> String {
    match format {
        Format::M3u => crate::m3u::to_m3u8(rows),
        Format::Pls => crate::pls::to_pls(rows),
        Format::Xspf => crate::xspf::to_xspf(rows),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn row(path: &str) -> ExportTrack {
        ExportTrack {
            path: path.into(),
            title: "One".into(),
            artist: "A".into(),
            duration_secs: 210,
        }
    }

    #[test]
    fn extensions_map_to_formats_case_insensitively() {
        let of = |name: &str| Format::from_path(&PathBuf::from(name));
        assert_eq!(of("a.m3u"), Some(Format::M3u));
        assert_eq!(of("a.M3U8"), Some(Format::M3u));
        assert_eq!(of("a.PLS"), Some(Format::Pls));
        assert_eq!(of("a.xspf"), Some(Format::Xspf));
        assert_eq!(of("a.txt"), None);
        assert_eq!(of("bare"), None);
        assert!(is_playlist_file(&PathBuf::from("/m/set.pls")));
        assert!(!is_playlist_file(&PathBuf::from("/m/song.flac")));
    }

    #[test]
    fn sniff_reads_the_body_not_the_name() {
        assert_eq!(Format::sniff("[playlist]\nFile1=/m/a.mp3\n"), Format::Pls);
        assert_eq!(
            Format::sniff("\u{feff}\n\n[PLAYLIST]\nfile1=/m/a.mp3\n"),
            Format::Pls
        );
        assert_eq!(
            Format::sniff("<?xml version=\"1.0\"?>\n<playlist/>"),
            Format::Xspf
        );
        assert_eq!(Format::sniff("<playlist version=\"1\"/>"), Format::Xspf);
        assert_eq!(Format::sniff("#EXTM3U\n/m/a.mp3\n"), Format::M3u);
        assert_eq!(Format::sniff("/m/a.mp3\n"), Format::M3u);
        assert_eq!(Format::sniff(""), Format::M3u);
    }

    #[test]
    fn parse_dispatches_on_the_sniffed_format() {
        let entries = parse("[playlist]\nFile1=/m/a.mp3\nNumberOfEntries=1\n");
        assert_eq!(entries, ["/m/a.mp3"]);
    }

    #[test]
    fn every_format_round_trips_through_the_door() {
        for format in [Format::M3u, Format::Pls, Format::Xspf] {
            let text = write(format, &[row("/m/a.mp3"), row("/m/b.mp3")]);
            assert_eq!(Format::sniff(&text), format, "{format:?} sniffs as itself");
            assert_eq!(parse(&text), ["/m/a.mp3", "/m/b.mp3"], "{format:?}");
        }
    }

    #[test]
    fn extensions_cover_the_constant() {
        for ext in EXTENSIONS {
            assert!(Format::from_path(&PathBuf::from(format!("a.{ext}"))).is_some());
        }
    }
}
