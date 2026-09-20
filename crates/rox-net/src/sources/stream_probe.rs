//! Ask a URL whether it's a stream, because the URL alone can't say.
//!
//! `rox_library::stations::refusal` catches everything the string gives
//! away: the wrong scheme, an HLS manifest, a playlist that belongs in the
//! importer. What it can't catch is the ordinary mistake, which is pasting
//! the station's web page instead of its mount. Those URLs look exactly
//! like a stream URL, and the only thing that tells them apart is what
//! comes back when you ask.
//!
//! So this asks, and it asks the way the transport will: a GET with
//! `Icy-MetaData: 1` on it, because a Shoutcast server introduces itself
//! in the response headers and a web server ignores the ask. The range
//! header keeps a live mount from starting to push music at a request
//! that only wants to read the top of the answer; a server free to ignore
//! it does, which costs nothing, since the body is dropped unread the
//! moment the headers are in.
//!
//! The bias is deliberately toward letting things through. A station is a
//! URL somebody found on a forum in 2011, and half of them answer 404
//! from an Icecast mount whose source is asleep, or with no content type
//! at all, or with a status line ureq won't parse. None of that means the
//! URL is wrong, and refusing a station that is merely down today would
//! be worse than the problem this solves. Only a positive answer that
//! this is a document gets turned away.
//!
//! Blocking, like everything in this crate. Background executor only.

use crate::providers::{agent, net_reason};

/// How much of the answer is asked for. Enough that a server with a small
/// file to serve sends it in one go rather than chunking, and small
/// enough that a mislabelled URL can't hand back a megabyte before the
/// body is dropped.
const PEEK: &str = "bytes=0-1023";

/// Content types that are a stream without being `audio/`. Ogg's official
/// type is under `application`, and a great many Icecast mounts are
/// configured with no type at all, which their proxies then stamp as
/// `application/octet-stream`.
const STREAM_TYPES: [&str; 2] = ["application/ogg", "application/octet-stream"];

/// What a URL answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Audio, an ICY greeting, or something close enough to either that
    /// turning it away would be a guess.
    Stream,

    /// A document: it answered, and what it answered with is a web page
    /// or a file that isn't audio. The content type it named comes with
    /// it, because "that's not a stream" is unarguable and useless, and
    /// "that's text/html" tells someone they pasted the station's site.
    Document { content_type: String },

    /// Nothing came back worth reading a verdict out of: no connection,
    /// a refusing status, a status line that isn't HTTP. The caller lets
    /// the add through on this and says the check didn't get an answer.
    Unknown { reason: String },
}

/// Ask `url` what it serves.
///
/// Blocking. Background executor only.
pub fn probe(url: &str) -> Probe {
    let asked = agent()
        .get(url)
        .set("Icy-MetaData", "1")
        .set("Range", PEEK)
        .call();

    let response = match asked {
        Ok(response) => response,

        // Through `net_reason` rather than ureq's own Display, which
        // prints the request URL; a station URL can carry a listener
        // token in its query string and this string reaches a panel.
        Err(e) => {
            return Probe::Unknown {
                reason: net_reason(&e),
            };
        }
    };

    // A station that sent any `icy-` header has introduced itself, and
    // that outranks its content type: Shoutcast mounts have been
    // answering `text/html` and then streaming MP3 down the same socket
    // for twenty years.
    let icy = response
        .headers_names()
        .iter()
        .any(|name| name.starts_with("icy-"));

    verdict(response.header("content-type").unwrap_or_default(), icy)
}

/// The rule itself, split out from the request so it can be read and
/// tested as what it is: a decision about one header.
fn verdict(content_type: &str, icy: bool) -> Probe {
    if icy {
        return Probe::Stream;
    }

    // The type without its parameters, so `audio/mpeg; charset=utf-8`
    // (which servers do send, wrongly, and often) is still audio.
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    // No type named is no answer. Plenty of small Icecast installs send
    // none at all, and a guess made from silence would turn them away.
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

    /// The three families that count as audio, plus the parameter a
    /// server has no business putting on them.
    #[test]
    fn the_audio_families_read_as_a_stream() {
        assert_eq!(verdict("audio/mpeg", false), Probe::Stream);
        assert_eq!(verdict("audio/aacp", false), Probe::Stream);
        assert_eq!(verdict("application/ogg", false), Probe::Stream);
        assert_eq!(verdict("application/octet-stream", false), Probe::Stream);
        assert_eq!(verdict("Audio/MPEG; charset=UTF-8", false), Probe::Stream);
    }

    /// A web page is the mistake this exists for, and the verdict carries
    /// the type so the message can name it.
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

    /// An `icy-` header beats the content type, because a Shoutcast mount
    /// that calls itself `text/html` is still a Shoutcast mount.
    #[test]
    fn an_icy_greeting_outranks_whatever_the_type_says() {
        assert_eq!(verdict("text/html", true), Probe::Stream);
        assert_eq!(verdict("", true), Probe::Stream);
    }

    /// A server that named no type is not a server that said no.
    #[test]
    fn no_content_type_is_no_answer_rather_than_a_refusal() {
        assert!(matches!(verdict("", false), Probe::Unknown { .. }));
        assert!(matches!(verdict("   ", false), Probe::Unknown { .. }));
    }
}
