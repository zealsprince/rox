//! Last.fm (ws.audioscrobbler.com): the artist lookups behind the
//! biography panel and the track counts behind the metadata panel's
//! global rows. artist.getInfo, artist.getTopTracks, and track.getInfo
//! are unsigned reads, so they go over the shared agent with just an api
//! key (the build's own identity or the settings override, the
//! scrobbler's fallback order), and no account or session enters into
//! it. The wiki text arrives as
//! HTML with a "Read more" anchor and a license sentence after it; both
//! strip here so callers hold plain paragraphs.

use serde::{Deserialize, Serialize};

use rox_core::settings::Settings;

use super::{agent, net_reason, string, ArtCandidate, ArtProvider, TrackQuery};

const API: &str = "https://ws.audioscrobbler.com/2.0/";

/// One artist as Last.fm records them, the biography panel's sheet: the
/// wiki text as plain paragraphs, the listening stats, the genre tags,
/// and the similar names. Serialized as the artist store's cache file;
/// missing fields default, so an old entry still loads after the shape
/// drifts.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistInfo {
    /// The name as Last.fm capitalizes it, not as the tag spelled it.
    pub name: String,
    /// The artist's Last.fm page, the attribution link the panel shows.
    pub url: String,
    /// The full wiki text, HTML stripped, paragraphs separated by blank
    /// lines. Empty when the wiki has no article.
    pub bio: String,
    /// The links the wiki text carried, as byte ranges into `bio` with
    /// their targets, so a panel can make them clickable. None on an
    /// entry written before links were kept, which the store treats as
    /// stale so they fill in without waiting out the TTL.
    pub links: Option<Vec<BioLink>>,
    pub listeners: u64,
    pub playcount: u64,
    /// The top genre tags, most applied first.
    pub tags: Vec<String>,
    /// The artists Last.fm files nearby, for the sheet's foot.
    pub similar: Vec<String>,
    /// The most played tracks, most first, from the second call the store
    /// makes after this one. None until that call has run, which is how a
    /// cache entry written before the list existed gets it filled in
    /// without waiting out the TTL; an empty list is a settled answer.
    pub top_tracks: Option<Vec<TopTrack>>,
}

/// One inline link in the wiki text: where it sits in the stripped text
/// and where it goes.
#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct BioLink {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// One of an artist's top tracks as Last.fm ranks them: the name and the
/// two counts the ranking runs on.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TopTrack {
    pub name: String,
    pub playcount: u64,
    pub listeners: u64,
    /// The track's Last.fm page.
    pub url: String,
}

/// One track as Last.fm counts it, the metadata panel's global rows:
/// how many people have scrobbled it, how often, and what they tagged
/// it. Serialized as the track stats store's cache file; missing fields
/// default, so an old entry still loads after the shape drifts.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackStats {
    /// The name as Last.fm capitalizes it.
    pub name: String,
    /// The track's Last.fm page.
    pub url: String,
    /// The album Last.fm files the track under, for the tag fallback when
    /// the tag itself names none.
    pub album: String,
    pub listeners: u64,
    pub playcount: u64,
    /// The top tags, most applied first. Last.fm holds tags for some
    /// tracks and none for others, well-known ones included, so the store
    /// falls back to the album's and then the artist's; `tags_scope`
    /// says which these are.
    pub tags: Vec<String>,
    /// Where the tags came from: "track", "album", or "artist". Empty
    /// on an entry written before the fallback existed, which reads as
    /// the track's own.
    pub tags_scope: String,
    /// The asking user's own scrobble count, when the lookup went under
    /// a username; None without one.
    pub user_plays: Option<u64>,
    /// Whether the asking user loved the track on Last.fm.
    pub loved: bool,
}

/// How many fallback tags to keep when the track has none of its own: the
/// count a track's own list comes back with.
const FALLBACK_TAGS: usize = 5;

/// The key the lookup calls with: the settings override when the user
/// entered one, the build's own identity otherwise, the scrobbler's
/// order. Empty when neither exists, which reads as the lookup being
/// unavailable rather than as an error.
fn api_key() -> String {
    let key = Settings::load().accounts.lastfm.api_key;
    if key.is_empty() {
        crate::lastfm::keys::API_KEY.to_string()
    } else {
        key
    }
}

/// Fetch an artist's info, blocking: Ok(None) is Last.fm having no such
/// name (or no api key to ask with), Err the network or the API
/// failing. Background executor only.
pub fn artist_info(name: &str, lang: &str) -> Result<Option<ArtistInfo>, String> {
    let key = api_key();
    if key.is_empty() || name.trim().is_empty() {
        return Ok(None);
    }
    // An API error still has a JSON body worth reading, so a status
    // failure parses like a success, the scrobbler's move.
    let request = agent()
        .get(API)
        .query("method", "artist.getinfo")
        .query("artist", name.trim())
        .query("autocorrect", "1")
        // Last.fm keeps per-language wiki text and serves English when a
        // language has none, so this narrows to the reader's language and
        // costs nothing when it doesn't exist. Most artists only have the
        // English text, which is why the caller records which language a
        // cached bio came back in rather than assuming it got one.
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

/// Fetch a track's counts and tags, blocking: Ok(None) is Last.fm having
/// no such track (or no api key to ask with), Err the network or the API
/// failing. Background executor only.
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
    // Naming the user adds their own count and loved flag to the answer,
    // still unsigned: it's public listening data, not the session's.
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
        // 6 is "track not found": a clean miss, not a failure.
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

/// An album's tags, most applied first, capped at the track list's
/// length: the first fallback for a track Last.fm holds no tags on.
/// Empty for an album it doesn't know, or on any failure, since a
/// fallback that errors would fail a lookup that already succeeded.
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

/// An artist's top tags, the second fallback. Same quiet failure as
/// [`album_tags`].
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

/// The artist's most played tracks, most first, capped at `limit`. The
/// same unsigned read as the info lookup, one call. A name Last.fm
/// doesn't know, or no key to ask with, is an empty list rather than an
/// error, since the info lookup already settled whether the artist
/// exists. Background executor only.
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

/// Last.fm's wiki HTML down to plain paragraphs, with the inline links
/// kept as ranges into the result. The "Read more on Last.fm" anchor and
/// the license sentence after it drop first (cut there, not at the first
/// link, since bios have inline links whose text should stay), then tags
/// strip with each anchor's target recorded against the text it wraps,
/// the common entities decode, and runs of blank lines fold to one
/// paragraph break.
fn parse_wiki(html: &str) -> (String, Vec<BioLink>) {
    let cut = html
        .find(">Read more on Last.fm</a>")
        .and_then(|pos| html[..pos].rfind("<a "))
        .unwrap_or(html.len());
    let html = &html[..cut];
    let mut out = String::with_capacity(html.len());
    let mut links = Vec::new();
    // The link whose text is being written: its target and where in
    // `out` it began.
    let mut open: Option<(String, usize)> = None;
    // Line state, the fold: `at_line_start` skips a line's leading
    // blanks, `blank` remembers an empty line between two with text.
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
            } else if lower == "/a" {
                if let Some((url, start)) = open.take() {
                    let end = out.trim_end().len().max(start);
                    if end > start {
                        links.push(BioLink { start, end, url });
                    }
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
            // The line ends: drop its trailing blanks, note an empty one.
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
                // A link that spans the break keeps its start; the text it
                // wraps grows across the join like any other.
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

/// The wiki HTML as plain paragraphs alone, the links dropped: the
/// fixtures' view of the parse.
#[cfg(test)]
fn strip_wiki(html: &str) -> String {
    parse_wiki(html).0
}

/// The `href` of an anchor tag's attributes, unquoted.
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

/// One HTML entity at the head of `rest` decoded, with the bytes it
/// took; a bare ampersand comes back as itself.
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

/// Rewrite Last.fm CDN size path segments (e.g. `/300x300/` or `/174s/`) to
/// full resolution (`/ar0/`).
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
