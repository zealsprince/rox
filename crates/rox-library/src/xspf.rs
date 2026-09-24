//! XSPF read and write for playlist interop (ADR 16). Locations are URIs, so
//! spaces and non-ASCII survive the trip. The writer is hand-rolled; the
//! reader uses `roxmltree`.
//!
//! Deliberately excluded: `<extension>`, per-track `<image>`/`<info>`, and
//! playlist-level metadata. The catalog holds all of that.

use std::path::Path;

use crate::playlists::ExportTrack;

/// `file:` URIs, empty fields omitted, and no duration element when it's
/// unknown. A cue subsong's `#N` rides as the URI fragment.
pub fn to_xspf(rows: &[ExportTrack]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n");
    out.push_str("  <trackList>\n");
    for row in rows {
        out.push_str("    <track>\n");
        out.push_str(&format!(
            "      <location>{}</location>\n",
            escape(&location(&row.path))
        ));
        if !row.title.is_empty() {
            out.push_str(&format!("      <title>{}</title>\n", escape(&row.title)));
        }
        if !row.artist.is_empty() {
            out.push_str(&format!(
                "      <creator>{}</creator>\n",
                escape(&row.artist)
            ));
        }
        if row.duration_secs > 0 {
            out.push_str(&format!(
                "      <duration>{}</duration>\n",
                row.duration_secs * 1000
            ));
        }
        out.push_str("    </track>\n");
    }
    out.push_str("  </trackList>\n");
    out.push_str("</playlist>\n");
    out
}

/// Track locations in document order. A `file:` URI decodes to a path;
/// anything else passes through for the resolver. Matching ignores the
/// namespace, which files in the wild often drop.
pub fn parse(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "track")
        .filter_map(|track| {
            track
                .children()
                .find(|child| child.is_element() && child.tag_name().name() == "location")
                .and_then(|node| node.text())
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(from_location)
        })
        .collect()
}

/// Falls back to the raw string for a relative path, which XSPF allows.
fn location(path: &str) -> String {
    let (base, fragment) = split_fragment(path);
    match url::Url::from_file_path(Path::new(base)) {
        Ok(url) => match fragment {
            Some(sub) => format!("{url}#{sub}"),
            None => url.to_string(),
        },
        Err(()) => path.to_owned(),
    }
}

fn from_location(text: &str) -> String {
    let Ok(url) = url::Url::parse(text) else {
        return text.to_owned();
    };
    if url.scheme() != "file" {
        return text.to_owned();
    }
    let Ok(path) = url.to_file_path() else {
        return text.to_owned();
    };
    let path = path.to_string_lossy().into_owned();
    match url.fragment() {
        Some(sub) if !sub.is_empty() => format!("{path}#{sub}"),
        _ => path,
    }
}

/// Only a positive integer counts as a fragment, so a name ending in `#hits`
/// keeps it.
fn split_fragment(path: &str) -> (&str, Option<u16>) {
    match path.rsplit_once('#') {
        Some((base, sub)) => match sub.parse::<u16>() {
            Ok(sub) if sub > 0 => (base, Some(sub)),
            _ => (path, None),
        },
        None => (path, None),
    }
}

/// Quotes are left alone: nothing here writes an attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, artist: &str, title: &str, secs: i64) -> ExportTrack {
        ExportTrack {
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            duration_secs: secs,
        }
    }

    #[test]
    fn writes_uris_and_escapes_text() {
        let xspf = to_xspf(&[row("/m/Bad & Über/one two.mp3", "A & B", "One", 210)]);
        assert!(
            xspf.contains("<location>file:///m/Bad%20&amp;%20%C3%9Cber/one%20two.mp3</location>"),
            "{xspf}"
        );
        assert!(xspf.contains("<title>One</title>"));
        assert!(xspf.contains("<creator>A &amp; B</creator>"));
        assert!(xspf.contains("<duration>210000</duration>"));
        assert_eq!(parse(&xspf), ["/m/Bad & Über/one two.mp3"]);
    }

    #[test]
    fn empty_fields_write_no_elements() {
        let xspf = to_xspf(&[row("/m/one.mp3", "", "", 0)]);
        assert!(!xspf.contains("<title>"));
        assert!(!xspf.contains("<creator>"));
        assert!(!xspf.contains("<duration>"));
    }

    #[test]
    fn round_trips_paths_in_order() {
        let rows = [
            row("/m/a.flac", "Artist", "A", 5),
            row("/m/b.flac", "Artist", "B", 6),
        ];
        assert_eq!(parse(&to_xspf(&rows)), ["/m/a.flac", "/m/b.flac"]);
    }

    #[test]
    fn a_cue_fragment_round_trips() {
        let xspf = to_xspf(&[row("/m/Album/disc.flac#3", "X", "Three", 180)]);
        assert!(
            xspf.contains("<location>file:///m/Album/disc.flac#3</location>"),
            "{xspf}"
        );
        assert_eq!(parse(&xspf), ["/m/Album/disc.flac#3"]);
    }

    #[test]
    fn a_non_file_location_comes_back_verbatim() {
        let text = "<playlist xmlns=\"http://xspf.org/ns/0/\"><trackList>\
                    <track><location>http://stream.example/live.ogg</location></track>\
                    <track><location>sub/relative.mp3</location></track>\
                    </trackList></playlist>";
        assert_eq!(
            parse(text),
            ["http://stream.example/live.ogg", "sub/relative.mp3"]
        );
    }

    #[test]
    fn a_document_without_the_namespace_still_parses() {
        let text = "<playlist version=\"1\"><trackList>\
                    <track><location>file:///m/one.mp3</location><title>One</title></track>\
                    </trackList></playlist>";
        assert_eq!(parse(text), ["/m/one.mp3"]);
    }

    #[test]
    fn malformed_xml_yields_nothing() {
        assert!(parse("<playlist><trackList>").is_empty());
    }
}
