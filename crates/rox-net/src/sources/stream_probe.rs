//! Ask a URL whether it's a stream. `rox_library::stations::refusal` catches
//! what the string gives away; this catches the common mistake of pasting the
//! station's web page instead of its mount.
//!
//! A GET with `Icy-MetaData: 1`, the way the transport asks, and a range
//! header so a live mount doesn't start pushing audio. The bias is toward
//! letting things through: plenty of real stations answer 404 while asleep or
//! send no content type, so only a positive "this is a document" is refused.

use crate::providers::{agent, net_reason};

const PEEK: &str = "bytes=0-1023";

/// Stream types outside `audio/`: Ogg's official type, and what proxies
/// stamp on Icecast mounts configured with no type.
const STREAM_TYPES: [&str; 2] = ["application/ogg", "application/octet-stream"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Audio, an ICY greeting, or close enough that refusing would be a guess.
    Stream,

    /// Carries the type so the message can say "that's text/html".
    Document { content_type: String },

    /// No verdict. The caller lets the add through.
    Unknown { reason: String },
}

pub fn probe(url: &str) -> Probe {
    let asked = agent()
        .get(url)
        .set("Icy-MetaData", "1")
        .set("Range", PEEK)
        .call();

    let response = match asked {
        Ok(response) => response,

        // Never ureq's Display: a station URL can carry a listener token.
        Err(e) => {
            return Probe::Unknown {
                reason: net_reason(&e),
            };
        }
    };

    // Any `icy-` header outranks the content type: Shoutcast mounts answer
    // `text/html` and then stream MP3.
    let icy = response
        .headers_names()
        .iter()
        .any(|name| name.starts_with("icy-"));

    verdict(response.header("content-type").unwrap_or_default(), icy)
}

fn verdict(content_type: &str, icy: bool) -> Probe {
    if icy {
        return Probe::Stream;
    }

    // Servers often put parameters like `charset` on audio types.
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    // Plenty of small Icecast installs send no type; don't guess from silence.
    if kind.is_empty() {
        return Probe::Unknown {
            reason: "no content type".to_string(),
        };
    }

    if kind.starts_with("audio/") || STREAM_TYPES.contains(&kind.as_str()) {
        return Probe::Stream;
    }

    Probe::Document { content_type: kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audio_families_read_as_a_stream() {
        assert_eq!(verdict("audio/mpeg", false), Probe::Stream);
        assert_eq!(verdict("audio/aacp", false), Probe::Stream);
        assert_eq!(verdict("application/ogg", false), Probe::Stream);
        assert_eq!(verdict("application/octet-stream", false), Probe::Stream);
        assert_eq!(verdict("Audio/MPEG; charset=UTF-8", false), Probe::Stream);
    }

    #[test]
    fn a_web_page_reads_as_a_document() {
        assert_eq!(
            verdict("text/html; charset=utf-8", false),
            Probe::Document {
                content_type: "text/html".to_string()
            }
        );
        assert_eq!(
            verdict("application/vnd.apple.mpegurl", false),
            Probe::Document {
                content_type: "application/vnd.apple.mpegurl".to_string()
            }
        );
    }

    #[test]
    fn an_icy_greeting_outranks_whatever_the_type_says() {
        assert_eq!(verdict("text/html", true), Probe::Stream);
        assert_eq!(verdict("", true), Probe::Stream);
    }

    #[test]
    fn no_content_type_is_no_answer_rather_than_a_refusal() {
        assert!(matches!(verdict("", false), Probe::Unknown { .. }));
        assert!(matches!(verdict("   ", false), Probe::Unknown { .. }));
    }
}
