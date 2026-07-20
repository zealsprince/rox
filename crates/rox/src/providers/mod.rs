//! Online enrichment providers per ADR 14: per-domain traits implemented
//! by per-service modules, blocking calls run on the background executor,
//! plain data out. A provider never touches a file; whatever it fetches
//! goes through the existing write paths (the metadata writer, the lyrics
//! save). HTTP rides one shared agent that carries the app's User-Agent
//! on every request. The metadata and art traits land with their first
//! services; lyrics is the domain built out so far.
//!
//! A lookup returns ranked candidates rather than one best guess, so a
//! picker can show them and the user confirms before anything is written.
//! Confidence scores each candidate against the track's own tags, the
//! same scorer every domain reuses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub mod deezer;
pub mod itunes;
pub mod lastfm;
pub mod lrclib;
pub mod musicbrainz;
pub mod theaudiodb;

/// The identity every provider request carries; MusicBrainz requires a
/// contactable User-Agent and the other services appreciate one.
const USER_AGENT: &str = concat!(
    "rox/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/zealsprince/rox)"
);

/// The one HTTP agent every provider shares: pooled connections, the app
/// User-Agent, and timeouts short enough that a dead network never parks
/// a background task for long.
pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
    })
}

/// Whether any lyrics provider is enabled, a static like the rating
/// style's: the panel checks it in render and menu paths where a
/// settings-file load has no place. Seeded at startup from the settings,
/// flipped by the settings window's Providers page.
static LYRICS_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn lyrics_online() -> bool {
    LYRICS_ONLINE.load(Ordering::Relaxed)
}

pub fn set_lyrics_online(on: bool) {
    LYRICS_ONLINE.store(on, Ordering::Relaxed);
}

/// Whether any metadata provider is enabled, the lyrics static's twin for
/// the tag lookup. Seeded at startup, flipped by the Providers page.
static METADATA_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn metadata_online() -> bool {
    METADATA_ONLINE.load(Ordering::Relaxed)
}

pub fn set_metadata_online(on: bool) {
    METADATA_ONLINE.store(on, Ordering::Relaxed);
}

/// Whether each cover-art service is enabled. Two providers rather than
/// one domain flag, so a user can lean on whichever service covers their
/// library better. Seeded at startup, flipped by the Providers page.
static ITUNES_ONLINE: AtomicBool = AtomicBool::new(true);
static DEEZER_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn itunes_online() -> bool {
    ITUNES_ONLINE.load(Ordering::Relaxed)
}

pub fn set_itunes_online(on: bool) {
    ITUNES_ONLINE.store(on, Ordering::Relaxed);
}

pub fn deezer_online() -> bool {
    DEEZER_ONLINE.load(Ordering::Relaxed)
}

pub fn set_deezer_online(on: bool) {
    DEEZER_ONLINE.store(on, Ordering::Relaxed);
}

/// Whether any cover-art service is on, the gate for offering the search
/// at all.
pub fn art_online() -> bool {
    itunes_online() || deezer_online()
}

/// Whether the artist lookup is enabled - the biography panel's domain:
/// last.fm's text and stats, with the deezer portrait riding along.
/// Seeded at startup, flipped by the Providers page.
static ARTIST_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn artist_online() -> bool {
    ARTIST_ONLINE.load(Ordering::Relaxed)
}

pub fn set_artist_online(on: bool) {
    ARTIST_ONLINE.store(on, Ordering::Relaxed);
}

/// The per-session lookup cache, per ADR 14: in-memory, keyed by query,
/// negative results included. Every aggregate search stores what it found
/// under the query it ran, so asking again inside a session answers from
/// memory instead of the network. Nothing persists; a restart starts cold,
/// which is the whole invalidation story until bulk operations need more.
/// Answer from the cache under `key`, or run `compute` and store what it
/// returns, empty results and all. An error is not stored: a network blip
/// should not pin a miss for the rest of the session. Cheap results ride a
/// clone out; the entries never move.
fn cached<T: Clone>(
    cache: &Mutex<HashMap<String, T>>,
    key: String,
    compute: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return Ok(hit.clone());
    }
    let value = compute()?;
    cache.lock().unwrap().insert(key, value.clone());
    Ok(value)
}

/// The cache key for a query: its fields folded to one stable string, so
/// the same lookup answers from one entry whatever the casing or spacing.
/// The unit separator between fields keeps a long artist from colliding
/// with a title; duration rounds to whole seconds, so a hair of drift does
/// not miss. Providers that toggle independently append their own state,
/// since a different provider set is a different answer.
fn query_key(query: &TrackQuery) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        normalize(&query.artist),
        normalize(&query.title),
        normalize(&query.album),
        query.duration_secs.map(|s| s.round() as i64).unwrap_or(-1),
    )
}

/// What a lookup matches on: the track's tags, or a hand-edited query
/// standing in for them. The duration narrows the confidence score when
/// known; None still queries.
#[derive(Clone)]
pub struct TrackQuery {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: Option<f64>,
}

/// One lyrics result a provider offered: the tags it carries so a picker
/// can show what it matched, the sheet text, whether that text is timed,
/// which service answered, and how well it scored against the query. The
/// text is the LRC when synced, plain lines otherwise; the parser tells
/// them apart on the re-read.
#[derive(Clone)]
pub struct LyricsCandidate {
    pub provider: &'static str,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: Option<f64>,
    pub synced: bool,
    pub text: String,
    pub confidence: f32,
}

/// A lyrics service. Returns every candidate it found, unscored; the
/// aggregate below scores and ranks them so one scorer decides the order
/// across providers. Blocking, background executor only.
pub trait LyricsProvider {
    fn name(&self) -> &'static str;
    fn search(&self, query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String>;
}

/// Search every enabled lyrics provider, score each candidate against
/// the query, and return them best first. An empty vec is a clean
/// no-match; Err is the network or an API failing. The order is code
/// (ADR 14: providers fixed, toggles in settings), so the ranking is the
/// confidence score, not which service happened to answer.
pub fn search_lyrics(query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String> {
    if !lyrics_online() {
        return Ok(Vec::new());
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<LyricsCandidate>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    cached(cache, query_key(query), || {
        let providers: &[&dyn LyricsProvider] = &[&lrclib::Lrclib];
        let mut found = Vec::new();
        for provider in providers {
            found.extend(provider.search(query)?);
        }
        for candidate in &mut found {
            candidate.confidence = confidence(query, candidate);
        }
        found.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(found)
    })
}

/// One metadata result a provider offered: the tag values it carries so a
/// compare can show them next to the track's own, which service answered,
/// and how well it scored. A field the service does not carry comes back
/// empty, and the compare leaves an empty fetched field alone. Year,
/// track, and disc ride as strings, the shape the writer takes.
#[derive(Clone)]
pub struct MetadataCandidate {
    pub provider: &'static str,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track_no: String,
    pub disc_no: String,
    pub duration_secs: Option<f64>,
    pub confidence: f32,
}

/// A metadata service. Returns every candidate it found, unscored; the
/// aggregate scores and ranks them, the lyrics shape. Blocking,
/// background executor only.
pub trait MetadataProvider {
    fn name(&self) -> &'static str;
    fn search(&self, query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String>;
}

/// Search every enabled metadata provider, score each candidate against
/// the query on the shared scorer, and return them best first. An empty
/// vec is a clean no-match; Err is the network or an API failing.
pub fn search_metadata(query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String> {
    if !metadata_online() {
        return Ok(Vec::new());
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<MetadataCandidate>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    cached(cache, query_key(query), || {
        let providers: &[&dyn MetadataProvider] = &[&musicbrainz::MusicBrainz];
        let mut found = Vec::new();
        for provider in providers {
            found.extend(provider.search(query)?);
        }
        for candidate in &mut found {
            candidate.confidence = score_fields(
                query,
                &candidate.title,
                &candidate.artist,
                &candidate.album,
                candidate.duration_secs,
            );
        }
        found.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(found)
    })
}

/// One cover-art result a provider offered: where to fetch a small
/// preview and the full image, its pixel size for the caption and the
/// quality sort, and the release it belongs to. Art is picked by eye, so
/// there is no confidence score here; the thumbnail is the judge.
#[derive(Clone)]
pub struct ArtCandidate {
    pub provider: &'static str,
    /// The release the cover belongs to, so the grid tells a compilation
    /// or a reissue apart from the album.
    pub album: String,
    pub thumb_url: String,
    pub full_url: String,
    pub width: u32,
    pub height: u32,
}

/// A cover-art service. Returns candidates as URLs, not bytes; the picker
/// fetches the preview and, on apply, the full image. Blocking,
/// background executor only.
pub trait ArtProvider {
    fn name(&self) -> &'static str;
    fn search(&self, query: &TrackQuery) -> Result<Vec<ArtCandidate>, String>;
}

/// Search every enabled art service and return the candidates, largest
/// first, so the crispest covers lead. A provider that errors is skipped
/// rather than failing the lot; only when all fail and nothing came back
/// does the error surface.
pub fn search_art(query: &TrackQuery) -> Result<Vec<ArtCandidate>, String> {
    let (itunes, deezer) = (itunes_online(), deezer_online());
    // Which services are on is part of the answer, so it rides the key: a
    // toggle since the last search is a different result, not a stale hit.
    let key = format!("{}\u{1f}{itunes}\u{1f}{deezer}", query_key(query));
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<ArtCandidate>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    cached(cache, key, || {
        let mut providers: Vec<&dyn ArtProvider> = Vec::new();
        if itunes {
            providers.push(&itunes::Itunes);
        }
        if deezer {
            providers.push(&deezer::Deezer);
        }
        let mut found = Vec::new();
        let mut first_error = None;
        for provider in providers {
            match provider.search(query) {
                Ok(candidates) => found.extend(candidates),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        if found.is_empty() {
            if let Some(e) = first_error {
                return Err(e);
            }
        }
        found.sort_by(|a, b| (b.width * b.height).cmp(&(a.width * a.height)));
        Ok(found)
    })
}

/// The biggest an image download will read, so a bad URL or a hostile
/// server can't stream gigabytes into a cover slot. Comfortably past a
/// high-resolution album scan.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Download an image over the shared agent, capped at [`MAX_IMAGE_BYTES`].
/// Used for both the grid previews and the full picture a save embeds, so
/// all art traffic carries the app User-Agent like the rest.
pub fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let response = agent().get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_IMAGE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// How well a candidate matches the query, 0 to 1: a weighted blend of
/// title, artist, and album similarity with duration proximity. Title
/// carries the most, album the least (compilations and reissues rename
/// it freely), and an unknown field on either side scores neutral rather
/// than punishing. The same scorer the tag lookup will reuse.
fn confidence(query: &TrackQuery, candidate: &LyricsCandidate) -> f32 {
    score_fields(
        query,
        &candidate.title,
        &candidate.artist,
        &candidate.album,
        candidate.duration_secs,
    )
}

/// The scorer both domains share: a candidate's bare tag fields against
/// the query, so lyrics and metadata rank the same way. Kept field-based
/// rather than over a candidate type so one function serves both.
fn score_fields(
    query: &TrackQuery,
    title: &str,
    artist: &str,
    album: &str,
    duration_secs: Option<f64>,
) -> f32 {
    let title = similarity(&query.title, title);
    let artist = similarity(&query.artist, artist);
    let album = if query.album.is_empty() || album.is_empty() {
        0.5
    } else {
        similarity(&query.album, album)
    };
    let duration = match (query.duration_secs, duration_secs) {
        (Some(a), Some(b)) => {
            let delta = (a - b).abs();
            // Dead on within a couple seconds, nothing past a dozen.
            (1.0 - ((delta - 2.0).max(0.0) / 10.0)).clamp(0.0, 1.0) as f32
        }
        _ => 0.5,
    };
    (0.45 * title + 0.30 * artist + 0.10 * album + 0.15 * duration).clamp(0.0, 1.0)
}

/// A rough similarity of two tag strings, 0 to 1: 1 when they normalize
/// equal, otherwise the Jaccard overlap of their word sets, so
/// "Harder, Better" and "harder better" match and word order does not
/// matter. Empty on either side scores 0, there is nothing to compare.
fn similarity(a: &str, b: &str) -> f32 {
    let (a, b) = (normalize(a), normalize(b));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let aw: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let bw: std::collections::HashSet<&str> = b.split_whitespace().collect();
    let intersection = aw.intersection(&bw).count();
    let union = aw.union(&bw).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Fold a tag down to comparable words: lowercase, every run of
/// non-alphanumerics to one space, trimmed. Punctuation and accents in
/// the raw casing stop being the reason two equal titles miss. Crate
/// visible: the artist store keys its cache files on the same folding.
pub(crate) fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(title: &str, artist: &str, album: &str, dur: Option<f64>) -> LyricsCandidate {
        LyricsCandidate {
            provider: "test",
            artist: artist.into(),
            title: title.into(),
            album: album.into(),
            duration_secs: dur,
            synced: false,
            text: String::new(),
            confidence: 0.0,
        }
    }

    #[test]
    fn normalize_folds_punctuation_and_case() {
        assert_eq!(normalize("Harder, Better!"), "harder better");
        assert_eq!(similarity("Harder, Better", "harder better"), 1.0);
    }

    #[test]
    fn exact_match_outranks_a_loose_one() {
        let query = TrackQuery {
            artist: "Daft Punk".into(),
            title: "Harder Better Faster Stronger".into(),
            album: "Discovery".into(),
            duration_secs: Some(224.0),
        };
        let exact = confidence(
            &query,
            &candidate(
                "Harder, Better, Faster, Stronger",
                "Daft Punk",
                "Discovery",
                Some(224.0),
            ),
        );
        let loose = confidence(
            &query,
            &candidate(
                "Harder Better Faster Stronger",
                "Daft Punk",
                "Deep Cuts",
                Some(313.0),
            ),
        );
        assert!(exact > loose);
        assert!(exact > 0.9);
    }

    #[test]
    fn cache_computes_once_negatives_included() {
        let cache: Mutex<HashMap<String, Vec<i32>>> = Mutex::new(HashMap::new());
        let mut runs = 0;
        let mut run = |value: Vec<i32>| {
            cached(&cache, "k".into(), || {
                runs += 1;
                Ok(value)
            })
        };
        // First call computes, even for an empty (negative) result.
        assert_eq!(run(Vec::new()).unwrap(), Vec::<i32>::new());
        // Second call hits the cache: compute never runs, and the stored
        // empty is what comes back, not a fresh compute.
        assert_eq!(run(vec![1, 2, 3]).unwrap(), Vec::<i32>::new());
        assert_eq!(runs, 1);
    }

    #[test]
    fn cache_does_not_store_errors() {
        let cache: Mutex<HashMap<String, Vec<i32>>> = Mutex::new(HashMap::new());
        // A failed compute stores nothing, so a retry runs again and can
        // land the real result instead of a pinned miss.
        assert!(cached(&cache, "k".into(), || Err("boom".into())).is_err());
        let mut runs = 0;
        let got = cached(&cache, "k".into(), || {
            runs += 1;
            Ok(vec![7])
        })
        .unwrap();
        assert_eq!(got, vec![7]);
        assert_eq!(runs, 1);
    }

    #[test]
    fn query_key_folds_casing_and_spacing() {
        let a = TrackQuery {
            artist: "Daft Punk".into(),
            title: "Harder, Better".into(),
            album: "Discovery".into(),
            duration_secs: Some(224.4),
        };
        let b = TrackQuery {
            artist: "  daft   punk ".into(),
            title: "harder better".into(),
            album: "DISCOVERY".into(),
            duration_secs: Some(224.0),
        };
        // Same track, different casing, spacing, and a hair of duration
        // drift: one cache entry.
        assert_eq!(query_key(&a), query_key(&b));
        // A different title is a different key.
        let c = TrackQuery {
            title: "One More Time".into(),
            ..a.clone()
        };
        assert_ne!(query_key(&a), query_key(&c));
    }

    #[test]
    fn missing_fields_score_neutral_not_zero() {
        let query = TrackQuery {
            artist: "Boards of Canada".into(),
            title: "Roygbiv".into(),
            album: String::new(),
            duration_secs: None,
        };
        // Album and duration unknown on the query side, but title and
        // artist land, so the score still clears a useful bar.
        let score = confidence(
            &query,
            &candidate(
                "Roygbiv",
                "Boards of Canada",
                "Music Has the Right",
                Some(151.0),
            ),
        );
        assert!(score > 0.8);
    }
}
