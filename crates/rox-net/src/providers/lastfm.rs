//! Last.fm (ws.audioscrobbler.com): the unsigned artist and track reads
//! behind the biography and metadata panels, with just an api key (the
//! settings override or the build's own). Wiki HTML is stripped to plain
//! paragraphs here.

use serde::{Deserialize, Serialize};

use rox_core::settings::Settings;

use super::{ArtCandidate, ArtProvider, TrackQuery, agent, net_reason, string};

const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// The artist store's cache file format; missing fields default so old
/// entries still load.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistInfo {
    /// As Last.fm capitalizes it, not as the tag spelled it.
    pub name: String,
    pub url: String,
    /// Plain paragraphs separated by blank lines.
    pub bio: String,
    /// Byte ranges into `bio`. None on an old entry, which the store treats as
    /// stale so links fill in without waiting out the TTL.
    pub links: Option<Vec<BioLink>>,
    pub listeners: u64,
    pub playcount: u64,
    pub tags: Vec<String>,
    pub similar: Vec<String>,
    /// From a second call. None until it ran (an old entry refetches before
    /// the TTL); an empty list is a settled answer.
    pub top_tracks: Option<Vec<TopTrack>>,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct BioLink {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopTrack {
    pub name: String,
    pub playcount: u64,
    pub listeners: u64,
    pub url: String,
}

/// The track stats store's cache file format; missing fields default so old
/// entries still load.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackStats {
    pub name: String,
    pub url: String,
    /// For the tag fallback when the tag names no album.
    pub album: String,
    pub listeners: u64,
    pub playcount: u64,
    /// Many tracks have no tags of their own, so the store falls back to the
    /// album's, then the artist's; `tags_scope` says which.
    pub tags: Vec<String>,
    /// "track", "album", or "artist". Empty on an old entry, which reads as
    /// the track's own.
    pub tags_scope: String,
    /// Only when the lookup named a user.
    pub user_plays: Option<u64>,
    pub loved: bool,
}

/// Matches the length of a track's own tag list.
const FALLBACK_TAGS: usize = 5;

/// The settings override, else the build's key. Empty means unavailable.
fn api_key() -> String {
    let key = Settings::load().accounts.lastfm.api_key;
    if key.is_empty() {
        crate::lastfm::keys::API_KEY.to_string()
    } else {
        key
    }
}

/// Ok(None) for an unknown name or no api key.
pub fn artist_info(name: &str, lang: &str) -> Result<Option<ArtistInfo>, String> {
    let key = api_key();
    if key.is_empty() || name.trim().is_empty() {
        return Ok(None);
    }
    // A status failure still carries a JSON error body.
    let request = agent()
        .get(API)
        .query("method", "artist.getinfo")
        .query("artist", name.trim())
        .query("autocorrect", "1")
        // Serves English when the language has no text; the caller records
        // which language came back.
        .query("lang", lang)
        .query("api_key", &key)
        .query("format", "json");
    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(code) = body.get("error").and_then(|e| e.as_i64()) {
        // 6: not found, a clean miss.
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
    let html = artist
        .get("bio")
        .and_then(|bio| bio.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let (bio, links) = parse_wiki(html);
    Ok(Some(ArtistInfo {
        name: string(artist.get("name")),
        url: string(artist.get("url")),
        bio,
        links: Some(links),
        listeners: count(stats.and_then(|s| s.get("listeners"))),
        playcount: count(stats.and_then(|s| s.get("playcount"))),
        tags: names(artist.get("tags"), "tag"),
        similar: names(artist.get("similar"), "artist"),
        top_tracks: None,
    }))
}

/// Ok(None) for an unknown track or no api key.
pub fn track_info(
    artist: &str,
    title: &str,
    username: Option<&str>,
) -> Result<Option<TrackStats>, String> {
    let key = api_key();
    if key.is_empty() || artist.trim().is_empty() || title.trim().is_empty() {
        return Ok(None);
    }
    let mut request = agent()
        .get(API)
        .query("method", "track.getinfo")
        .query("artist", artist.trim())
        .query("track", title.trim())
        .query("autocorrect", "1")
        .query("api_key", &key)
        .query("format", "json");
    // Naming the user adds their count and loved flag, still unsigned.
    if let Some(user) = username.map(str::trim).filter(|u| !u.is_empty()) {
        request = request.query("username", user);
    }
    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(code) = body.get("error").and_then(|e| e.as_i64()) {
        // 6: not found, a clean miss.
        if code == 6 {
            return Ok(None);
        }
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(message.to_string());
    }
    let Some(track) = body.get("track") else {
        return Ok(None);
    };
    let tags = names(track.get("toptags"), "tag");
    Ok(Some(TrackStats {
        name: string(track.get("name")),
        url: string(track.get("url")),
        album: string(track.get("album").and_then(|a| a.get("title"))),
        listeners: count(track.get("listeners")),
        playcount: count(track.get("playcount")),
        tags_scope: if tags.is_empty() {
            String::new()
        } else {
            "track".to_string()
        },
        tags,
        user_plays: track
            .get("userplaycount")
            .map(|v| count(Some(v)))
            .filter(|_| username.is_some()),
        loved: track
            .get("userloved")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "1"),
    }))
}

/// The first tag fallback. Fails quietly to empty: a fallback that errors
/// would fail a lookup that already succeeded.
pub fn album_tags(artist: &str, album: &str) -> Vec<String> {
    let key = api_key();
    if key.is_empty() || artist.trim().is_empty() || album.trim().is_empty() {
        return Vec::new();
    }
    let body = agent()
        .get(API)
        .query("method", "album.getinfo")
        .query("artist", artist.trim())
        .query("album", album.trim())
        .query("autocorrect", "1")
        .query("api_key", &key)
        .query("format", "json")
        .call()
        .ok()
        .and_then(|response| response.into_string().ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let mut tags = names(
        body.as_ref().and_then(|b| b.get("album")?.get("tags")),
        "tag",
    );
    tags.truncate(FALLBACK_TAGS);
    tags
}

/// The second tag fallback, same quiet failure.
pub fn artist_tags(artist: &str) -> Vec<String> {
    let key = api_key();
    if key.is_empty() || artist.trim().is_empty() {
        return Vec::new();
    }
    let body = agent()
        .get(API)
        .query("method", "artist.gettoptags")
        .query("artist", artist.trim())
        .query("autocorrect", "1")
        .query("api_key", &key)
        .query("format", "json")
        .call()
        .ok()
        .and_then(|response| response.into_string().ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let mut tags = names(body.as_ref().and_then(|b| b.get("toptags")), "tag");
    tags.truncate(FALLBACK_TAGS);
    tags
}

/// An unknown name is an empty list, not an error: the info lookup already
/// settled whether the artist exists.
pub fn top_tracks(name: &str, limit: usize) -> Result<Vec<TopTrack>, String> {
    let key = api_key();
    if key.is_empty() || name.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let request = agent()
        .get(API)
        .query("method", "artist.gettoptracks")
        .query("artist", name.trim())
        .query("autocorrect", "1")
        .query("limit", &limit.to_string())
        .query("api_key", &key)
        .query("format", "json");
    let text = match request.call() {
        Ok(response) => response.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, response)) => {
            response.into_string().map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };
    let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(code) = body.get("error").and_then(|e| e.as_i64()) {
        if code == 6 {
            return Ok(Vec::new());
        }
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(message.to_string());
    }
    let tracks = body
        .get("toptracks")
        .and_then(|t| t.get("track"))
        .and_then(|list| list.as_array())
        .map(|list| {
            list.iter()
                .map(|track| TopTrack {
                    name: string(track.get("name")),
                    playcount: count(track.get("playcount")),
                    listeners: count(track.get("listeners")),
                    url: string(track.get("url")),
                })
                .filter(|track| !track.name.is_empty())
                .take(limit)
                .collect()
        })
        .unwrap_or_default();
    Ok(tracks)
}

fn count(value: Option<&serde_json::Value>) -> u64 {
    value
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| value.and_then(|v| v.as_u64()))
        .unwrap_or(0)
}

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

/// Wiki HTML to plain paragraphs, inline links kept as ranges. Cuts at the
/// "Read more on Last.fm" anchor, not the first link: bios have inline links
/// whose text should stay.
fn parse_wiki(html: &str) -> (String, Vec<BioLink>) {
    let cut = html
        .find(">Read more on Last.fm</a>")
        .and_then(|pos| html[..pos].rfind("<a "))
        .unwrap_or(html.len());
    let html = &html[..cut];
    let mut out = String::with_capacity(html.len());
    let mut links = Vec::new();
    let mut open: Option<(String, usize)> = None;
    let mut at_line_start = true;
    let mut blank = false;
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(tag_end) = rest.strip_prefix('<').and_then(|r| r.find('>')) {
            let tag = &rest[1..1 + tag_end];
            rest = &rest[tag_end + 2..];
            let lower = tag.to_ascii_lowercase();
            if lower.starts_with("a ") || lower == "a" {
                open = href(tag).map(|url| (url, out.len()));
            } else if lower == "/a"
                && let Some((url, start)) = open.take()
            {
                let end = out.trim_end().len().max(start);
                if end > start {
                    links.push(BioLink { start, end, url });
                }
            }
            continue;
        }
        let (ch, len) = match rest.chars().next() {
            Some('&') => {
                let (decoded, len) = entity(rest);
                (decoded, len)
            }
            Some(c) => (c, c.len_utf8()),
            None => break,
        };
        rest = &rest[len..];
        if ch == '\n' {
            let trimmed = out.trim_end().len();
            if trimmed < out.len() {
                out.truncate(trimmed);
                for link in &mut links {
                    link.end = link.end.min(trimmed);
                }
            }
            blank |= at_line_start && !out.is_empty();
            at_line_start = true;
            continue;
        }
        if at_line_start {
            if ch.is_whitespace() {
                continue;
            }
            if !out.is_empty() {
                out.push_str(if blank { "\n\n" } else { "\n" });
            }
            blank = false;
            at_line_start = false;
        }
        out.push(ch);
    }
    if let Some((url, start)) = open.take() {
        let end = out.trim_end().len();
        if end > start {
            links.push(BioLink { start, end, url });
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    links.retain(|link| link.end > link.start && link.end <= out.len());
    (out, links)
}

#[cfg(test)]
fn strip_wiki(html: &str) -> String {
    parse_wiki(html).0
}

fn href(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find("href=")?;
    let value = &tag[at + 5..];
    let (quote, value) = match value.chars().next()? {
        c @ ('"' | '\'') => (Some(c), &value[1..]),
        _ => (None, value),
    };
    let end = match quote {
        Some(q) => value.find(q)?,
        None => value.find(char::is_whitespace).unwrap_or(value.len()),
    };
    let url = value[..end].trim();
    (!url.is_empty()).then(|| url.to_string())
}

fn entity(rest: &str) -> (char, usize) {
    const ENTITIES: [(&str, char); 6] = [
        ("&quot;", '"'),
        ("&#39;", '\''),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&nbsp;", ' '),
        ("&amp;", '&'),
    ];
    for (name, ch) in ENTITIES {
        if rest.starts_with(name) {
            return (ch, name.len());
        }
    }
    ('&', 1)
}

pub struct LastfmArt;

impl ArtProvider for LastfmArt {
    fn name(&self) -> &'static str {
        "lastfm"
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<ArtCandidate>, String> {
        let key = api_key();
        if key.is_empty() || query.artist.trim().is_empty() {
            return Ok(Vec::new());
        }
        let album_name = if query.album.is_empty() {
            &query.title
        } else {
            &query.album
        };
        if album_name.trim().is_empty() {
            return Ok(Vec::new());
        }

        let request = agent()
            .get(API)
            .query("method", "album.getinfo")
            .query("artist", query.artist.trim())
            .query("album", album_name.trim())
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
        if body.get("error").is_some() {
            return Ok(Vec::new());
        }

        let Some(album) = body.get("album") else {
            return Ok(Vec::new());
        };

        let title = string(album.get("name"));
        let images = album.get("image").and_then(|i| i.as_array());

        let mut best_url = String::new();
        if let Some(imgs) = images {
            for img in imgs.iter().rev() {
                let url = string(img.get("#text"));
                if !url.is_empty() {
                    best_url = url;
                    break;
                }
            }
        }

        if best_url.is_empty() {
            return Ok(Vec::new());
        }

        let full = full_size_url(&best_url);
        Ok(vec![ArtCandidate {
            provider: self.name(),
            album: title,
            artist: string(album.get("artist")),
            thumb_url: best_url,
            full_url: full,
            width: 1000,
            height: 1000,
        }])
    }
}

fn full_size_url(url: &str) -> String {
    for segment in &["/300x300/", "/174s/", "/64s/", "/34s/", "/mega/"] {
        if url.contains(segment) {
            return url.replace(segment, "/ar0/");
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_tail_drops_but_inline_links_survive() {
        let html = "Formed alongside <a href=\"https://www.Last.fm/music/Other\">Other</a> in 1993.\n\nMore text. <a href=\"https://www.Last.fm/music/Band\">Read more on Last.fm</a>. User-contributed text is available under the Creative Commons By-SA License.";
        let text = strip_wiki(html);
        assert_eq!(text, "Formed alongside Other in 1993.\n\nMore text.");
    }

    #[test]
    fn entities_decode_and_blanks_fold() {
        let text = strip_wiki("Ben &amp; Jerry&#39;s\n\n\n\nsecond &quot;paragraph&quot;");
        assert_eq!(text, "Ben & Jerry's\n\nsecond \"paragraph\"");
    }

    #[test]
    fn links_keep_their_place_in_the_stripped_text() {
        let html = "Formed alongside <a href=\"https://www.last.fm/music/Other\">Other</a> in 1993.\n\nSee <a href='https://x/y'>Ben &amp; Jerry</a>.";
        let (text, links) = parse_wiki(html);
        assert_eq!(text, "Formed alongside Other in 1993.\n\nSee Ben & Jerry.");
        assert_eq!(links.len(), 2);
        assert_eq!(&text[links[0].start..links[0].end], "Other");
        assert_eq!(links[0].url, "https://www.last.fm/music/Other");
        assert_eq!(&text[links[1].start..links[1].end], "Ben & Jerry");
        assert_eq!(links[1].url, "https://x/y");
    }

    #[test]
    fn empty_wiki_stays_empty() {
        assert_eq!(strip_wiki(""), "");
        assert_eq!(
            strip_wiki("<a href=\"https://www.Last.fm/music/X\">Read more on Last.fm</a>."),
            ""
        );
    }
}
