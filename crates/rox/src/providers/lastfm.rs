//! Last.fm (ws.audioscrobbler.com): the artist lookup behind the
//! biography panel. artist.getInfo is one of the API's unsigned reads,
//! so it rides the shared agent with just an api key - the build's own
//! identity or the settings override, the scrobbler's fallback order -
//! and no account or session enters into it. The wiki text arrives as
//! HTML with a "Read more" anchor and a license sentence riding its
//! tail; both strip here so callers hold plain paragraphs.

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

use super::agent;

const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// One artist as last.fm knows them, the biography panel's sheet: the
/// wiki text as plain paragraphs, the listening stats, the genre tags,
/// and the similar names. Serialized as the artist store's cache file;
/// missing fields default, so an old entry survives shape drift.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistInfo {
    /// The name as last.fm capitalizes it, not as the tag spelled it.
    pub name: String,
    /// The artist's last.fm page, the attribution link the panel shows.
    pub url: String,
    /// The full wiki text, HTML stripped, paragraphs separated by blank
    /// lines. Empty when the wiki carries no article.
    pub bio: String,
    pub listeners: u64,
    pub playcount: u64,
    /// The top genre tags, most applied first.
    pub tags: Vec<String>,
    /// The artists last.fm files nearby, for the sheet's foot.
    pub similar: Vec<String>,
}

/// The key the lookup calls with: the settings override when the user
/// entered one, the build's own identity otherwise - the scrobbler's
/// order. Empty when neither exists, which reads as the lookup being
/// unavailable rather than as an error.
fn api_key() -> String {
    let key = Settings::load().lastfm.api_key;
    if key.is_empty() {
        crate::lastfm::keys::API_KEY.to_string()
    } else {
        key
    }
}

/// Fetch an artist's info, blocking: Ok(None) is last.fm not knowing
/// the name (or no api key to ask with), Err the network or the API
/// failing. Background executor only.
pub fn artist_info(name: &str) -> Result<Option<ArtistInfo>, String> {
    let key = api_key();
    if key.is_empty() || name.trim().is_empty() {
        return Ok(None);
    }
    // An API error still carries a JSON body worth reading, so a status
    // failure parses like a success, the scrobbler's move.
    let request = agent()
        .get(API)
        .query("method", "artist.getinfo")
        .query("artist", name.trim())
        .query("autocorrect", "1")
        .query("api_key", &key)
        .query("format", "json");
    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(code) = body.get("error").and_then(|e| e.as_i64()) {
        // 6 is "artist not found": a clean miss, not a failure.
        if code == 6 {
            return Ok(None);
        }
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(message.to_string());
    }
    let Some(artist) = body.get("artist") else {
        return Ok(None);
    };
    let stats = artist.get("stats");
    let bio = artist
        .get("bio")
        .and_then(|bio| bio.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    Ok(Some(ArtistInfo {
        name: string(artist.get("name")),
        url: string(artist.get("url")),
        bio: strip_wiki(bio),
        listeners: count(stats.and_then(|s| s.get("listeners"))),
        playcount: count(stats.and_then(|s| s.get("playcount"))),
        tags: names(artist.get("tags"), "tag"),
        similar: names(artist.get("similar"), "artist"),
    }))
}

/// A JSON string field trimmed, or empty when absent.
fn string(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// A count the API sends as a string ("1234"), or 0 when absent or odd.
fn count(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| value.and_then(|v| v.as_u64()))
        .unwrap_or(0)
}

/// The names off one of the API's wrapped lists (`tags.tag[].name`,
/// `similar.artist[].name`), empties dropped.
fn names(wrapper: Option<&serde_json::Value>, key: &str) -> Vec<String> {
    wrapper
        .and_then(|w| w.get(key))
        .and_then(|list| list.as_array())
        .map(|list| {
            list.iter()
                .map(|entry| string(entry.get("name")))
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Last.fm's wiki HTML down to plain paragraphs: the "Read more on
/// Last.fm" anchor and the license sentence riding after it drop first
/// (cut there, not at the first link - bios carry inline links whose
/// text should survive), then tags strip, the common entities decode,
/// and runs of blank lines fold to one paragraph break.
fn strip_wiki(html: &str) -> String {
    let cut = html
        .find(">Read more on Last.fm</a>")
        .and_then(|pos| html[..pos].rfind("<a "))
        .unwrap_or(html.len());
    let mut text = String::with_capacity(cut);
    let mut in_tag = false;
    for ch in html[..cut].chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    let text = text
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    let mut out = String::with_capacity(text.len());
    let mut blank = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        out.push_str(line);
        blank = false;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_tail_drops_but_inline_links_survive() {
        let html = "Formed alongside <a href=\"https://www.last.fm/music/Other\">Other</a> in 1993.\n\nMore text. <a href=\"https://www.last.fm/music/Band\">Read more on Last.fm</a>. User-contributed text is available under the Creative Commons By-SA License.";
        let text = strip_wiki(html);
        assert_eq!(text, "Formed alongside Other in 1993.\n\nMore text.");
    }

    #[test]
    fn entities_decode_and_blanks_fold() {
        let text = strip_wiki("Ben &amp; Jerry&#39;s\n\n\n\nsecond &quot;paragraph&quot;");
        assert_eq!(text, "Ben & Jerry's\n\nsecond \"paragraph\"");
    }

    #[test]
    fn empty_wiki_stays_empty() {
        assert_eq!(strip_wiki(""), "");
        assert_eq!(
            strip_wiki("<a href=\"https://www.last.fm/music/X\">Read more on Last.fm</a>."),
            ""
        );
    }
}
