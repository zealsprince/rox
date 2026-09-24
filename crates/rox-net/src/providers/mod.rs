//! Online enrichment providers per ADR 14: per-domain traits implemented
//! by per-service modules, blocking calls, plain data out. A provider never
//! touches a file; what it fetches goes through the existing write paths.
//! Lookups return ranked candidates for the user to confirm.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub mod acoustid;
pub mod deezer;
pub mod itunes;
pub mod lastfm;
pub mod lrclib;
pub mod musicbrainz;
pub mod theaudiodb;

/// MusicBrainz requires a contactable User-Agent.
const USER_AGENT: &str = concat!(
    "rox/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/zealsprince/rox)"
);

/// Short timeouts, so a dead network never parks a background task for long.
pub fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
    })
}

/// The only sanctioned way to stringify a provider's ureq error; never call
/// `.to_string()` on one. ureq's Display prints the full URL, and Last.fm's
/// carries the api key.
pub fn net_reason(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("service returned {code}"),
        ureq::Error::Transport(t) => match t.kind() {
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Io => {
                "no connection".to_string()
            }
            other => other.to_string(),
        },
    }
}

/// Static so render and menu paths can check it without a settings load.
/// Seeded at startup, flipped by the Providers page. Same for the flags below.
static LYRICS_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn lyrics_online() -> bool {
    LYRICS_ONLINE.load(Ordering::Relaxed)
}

pub fn set_lyrics_online(on: bool) {
    LYRICS_ONLINE.store(on, Ordering::Relaxed);
}

static METADATA_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn metadata_online() -> bool {
    METADATA_ONLINE.load(Ordering::Relaxed)
}

pub fn set_metadata_online(on: bool) {
    METADATA_ONLINE.store(on, Ordering::Relaxed);
}

static ACOUSTID_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn acoustid_online() -> bool {
    ACOUSTID_ONLINE.load(Ordering::Relaxed)
}

pub fn set_acoustid_online(on: bool) {
    ACOUSTID_ONLINE.store(on, Ordering::Relaxed);
}

/// AcoustID refuses anonymous callers, so the identify is offered only when
/// some key exists. Reads settings when the build has none, so call it on a
/// click, not in a paint.
pub fn acoustid_available() -> bool {
    acoustid_online() && (!acoustid::CLIENT_KEY.is_empty() || !acoustid::client_key().is_empty())
}

static ITUNES_ONLINE: AtomicBool = AtomicBool::new(true);
static DEEZER_ONLINE: AtomicBool = AtomicBool::new(true);
static LASTFM_ART_ONLINE: AtomicBool = AtomicBool::new(true);

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

pub fn lastfm_art_online() -> bool {
    LASTFM_ART_ONLINE.load(Ordering::Relaxed)
}

pub fn set_lastfm_art_online(on: bool) {
    LASTFM_ART_ONLINE.store(on, Ordering::Relaxed);
}

pub fn art_online() -> bool {
    itunes_online() || deezer_online() || lastfm_art_online()
}

/// The biography panel's domain: Last.fm text and stats plus the deezer portrait.
static ARTIST_ONLINE: AtomicBool = AtomicBool::new(true);

pub fn artist_online() -> bool {
    ARTIST_ONLINE.load(Ordering::Relaxed)
}

pub fn set_artist_online(on: bool) {
    ARTIST_ONLINE.store(on, Ordering::Relaxed);
}

/// The per-session lookup cache, per ADR 14: keyed by query, empty results
/// included, nothing persisted. Errors aren't stored. Concurrent asks for one
/// key share a single compute, so they don't both hit MusicBrainz's ~1.1s
/// throttle.
struct SessionCache<T> {
    entries: Mutex<HashMap<String, T>>,
    inflight: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl<T> Default for SessionCache<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Clone> SessionCache<T> {
    fn get_or_compute(
        &self,
        key: String,
        compute: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if let Some(hit) = self.entries.lock().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        let gate = self
            .inflight
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_default()
            .clone();
        let _held = gate.lock().unwrap();
        if let Some(hit) = self.entries.lock().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        let value = compute()?;
        self.entries.lock().unwrap().insert(key, value.clone());
        Ok(value)
    }
}

/// Keep every candidate, skipping providers that error. The error surfaces
/// only when nothing came back at all.
fn collect_candidates<T>(
    searches: impl IntoIterator<Item = Result<Vec<T>, String>>,
) -> Result<Vec<T>, String> {
    let mut found = Vec::new();
    let mut first_error = None;
    for result in searches {
        match result {
            Ok(candidates) => found.extend(candidates),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    if found.is_empty()
        && let Some(e) = first_error
    {
        return Err(e);
    }
    Ok(found)
}

/// Fields folded and joined with the unit separator; duration rounds to
/// whole seconds so a hair of drift still hits.
fn query_key(query: &TrackQuery) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        normalize(&query.artist),
        normalize(&query.title),
        normalize(&query.album),
        query.duration_secs.map(|s| s.round() as i64).unwrap_or(-1),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackQuery {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: Option<f64>,
}

/// `text` is LRC when synced, plain lines otherwise.
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

/// Returns candidates unscored; the aggregate ranks across providers with one scorer.
pub trait LyricsProvider {
    fn name(&self) -> &'static str;
    fn search(&self, query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String>;
}

/// Ranked by confidence, not by which service answered (ADR 14).
pub fn search_lyrics(query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String> {
    if !lyrics_online() {
        return Ok(Vec::new());
    }
    static CACHE: OnceLock<SessionCache<Vec<LyricsCandidate>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    cache.get_or_compute(query_key(query), || {
        let providers: &[&dyn LyricsProvider] = &[&lrclib::Lrclib];
        let mut found = collect_candidates(providers.iter().map(|p| p.search(query)))?;
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

/// A field the service lacks comes back empty and the compare leaves it
/// alone. Year, track, and disc are strings, the shape the writer takes.
#[derive(Clone)]
pub struct MetadataCandidate {
    pub provider: &'static str,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// Only these two sorts: no service rox queries serves a title or album
    /// sort.
    pub artist_sort: String,
    pub album_artist_sort: String,
    pub year: String,
    pub track_no: String,
    pub disc_no: String,
    pub duration_secs: Option<f64>,
    pub confidence: f32,
}

pub trait MetadataProvider {
    fn name(&self) -> &'static str;
    fn search(&self, query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String>;
}

pub fn search_metadata(query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String> {
    if !metadata_online() {
        return Ok(Vec::new());
    }
    static CACHE: OnceLock<SessionCache<Vec<MetadataCandidate>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    cache.get_or_compute(query_key(query), || {
        let providers: &[&dyn MetadataProvider] = &[&musicbrainz::MusicBrainz];
        let mut found = collect_candidates(providers.iter().map(|p| p.search(query)))?;
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

/// MusicBrainz's one-a-second throttle makes every hit past this another
/// second at the button.
const IDENTIFY_HITS: usize = 5;

/// Identify a track by its sound: AcoustID maps the fingerprint to
/// MusicBrainz recordings, then each is fetched for its tags.
///
/// Confidence is AcoustID's score, never the text scorer's: the tags may be
/// the very thing that's wrong. `query` only picks which release the track
/// and disc numbers come from.
pub fn identify(
    fingerprint: &str,
    duration_secs: u32,
    query: &TrackQuery,
) -> Result<Vec<MetadataCandidate>, String> {
    if !acoustid_available() {
        return Ok(Vec::new());
    }
    static CACHE: OnceLock<SessionCache<Vec<MetadataCandidate>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    // Keyed on the fingerprint, not the tags: same audio, same answer. An
    // edited query re-asked keeps the first run's release pick.
    let key = format!("{fingerprint}\u{1f}{duration_secs}");
    cache.get_or_compute(key, || {
        let hits = acoustid::lookup(fingerprint, duration_secs)?;
        collect_candidates(hits.iter().take(IDENTIFY_HITS).map(|hit| {
            musicbrainz::recording_by_id(&hit.recording_id, query).map(|found| {
                found
                    .into_iter()
                    .map(|mut candidate| {
                        candidate.provider = "acoustid";
                        candidate.confidence = hit.score;
                        candidate
                    })
                    .collect::<Vec<_>>()
            })
        }))
    })
}

/// No stored confidence: the grid judges by eye, and unattended callers use
/// [`art_confidence`].
#[derive(Clone)]
pub struct ArtCandidate {
    pub provider: &'static str,
    pub album: String,
    pub artist: String,
    pub thumb_url: String,
    pub full_url: String,
    pub width: u32,
    pub height: u32,
}

pub trait ArtProvider {
    fn name(&self) -> &'static str;
    fn search(&self, query: &TrackQuery) -> Result<Vec<ArtCandidate>, String>;
}

/// Largest first. A provider that errors is skipped.
pub fn search_art(query: &TrackQuery) -> Result<Vec<ArtCandidate>, String> {
    let (itunes, deezer, lastfm_art) = (itunes_online(), deezer_online(), lastfm_art_online());
    // Which services are on is part of the answer, so it's in the key.
    let key = format!(
        "{}\u{1f}{itunes}\u{1f}{deezer}\u{1f}{lastfm_art}",
        query_key(query)
    );
    static CACHE: OnceLock<SessionCache<Vec<ArtCandidate>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    cache.get_or_compute(key, || {
        let mut providers: Vec<&dyn ArtProvider> = Vec::new();
        if itunes {
            providers.push(&itunes::Itunes);
        }
        if deezer {
            providers.push(&deezer::Deezer);
        }
        if lastfm_art {
            providers.push(&lastfm::LastfmArt);
        }
        let mut found = collect_candidates(providers.iter().map(|p| p.search(query)))?;
        found.sort_by_key(|b| std::cmp::Reverse(b.width * b.height));
        Ok(found)
    })
}

/// 0 to 1: the release title against the album (or the title when there's
/// no album), with the artist weighed in when named. Callers picking without
/// a human filter on this, since a wrong cover reads worse than none.
pub fn art_confidence(query: &TrackQuery, candidate: &ArtCandidate) -> f32 {
    let subject = if query.album.is_empty() {
        &query.title
    } else {
        &query.album
    };
    let album = similarity(subject, &candidate.album);
    if candidate.artist.is_empty() {
        album
    } else {
        (0.6 * album + 0.4 * similarity(&query.artist, &candidate.artist)).clamp(0.0, 1.0)
    }
}

/// Cap on an image download, well past a high-resolution album scan.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

pub fn fetch_image(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let response = agent().get(url).call().map_err(|e| net_reason(&e))?;
    let mut bytes = Vec::new();
    // Read one byte past the cap: truncating silently would cache a corrupt
    // image the exists-gates never refetch.
    response
        .into_reader()
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!("image exceeds the {MAX_IMAGE_BYTES} byte cap"));
    }
    Ok(bytes)
}

/// Title weighs most, album least (reissues rename it freely); an unknown
/// field scores neutral.
fn confidence(query: &TrackQuery, candidate: &LyricsCandidate) -> f32 {
    score_fields(
        query,
        &candidate.title,
        &candidate.artist,
        &candidate.album,
        candidate.duration_secs,
    )
}

/// Field-based so lyrics and metadata candidates share one scorer.
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

/// 1 when equal after normalizing, else Jaccard overlap of the word sets.
/// Empty on either side scores 0.
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

/// Lowercase, runs of non-alphanumerics to one space, trimmed. The artist
/// store keys its cache files on this too.
pub fn normalize(s: &str) -> String {
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

/// [`normalize`] with accents stripped, for exact-name gates ("Beyoncé" vs
/// "Beyonce"). A table rather than ICU: rox-net is a leaf crate, and only
/// Latin-1 Supplement and Latin Extended-A carry marks tags use.
pub fn normalize_folded(s: &str) -> String {
    let normalized = normalize(s);
    if normalized.is_ascii() {
        return normalized;
    }
    let mut out = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match unaccent(ch) {
            Some(base) => out.push_str(base),
            None => out.push(ch),
        }
    }
    out
}

fn unaccent(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{00C0}'..='\u{00C5}' | '\u{00E0}'..='\u{00E5}' => "a",
        '\u{00C6}' | '\u{00E6}' => "ae",
        '\u{00C7}' | '\u{00E7}' => "c",
        '\u{00C8}'..='\u{00CB}' | '\u{00E8}'..='\u{00EB}' => "e",
        '\u{00CC}'..='\u{00CF}' | '\u{00EC}'..='\u{00EF}' => "i",
        '\u{00D0}' | '\u{00F0}' => "d",
        '\u{00D1}' | '\u{00F1}' => "n",
        '\u{00D2}'..='\u{00D6}' | '\u{00D8}' => "o",
        '\u{00F2}'..='\u{00F6}' | '\u{00F8}' => "o",
        '\u{00D9}'..='\u{00DC}' | '\u{00F9}'..='\u{00FC}' => "u",
        '\u{00DD}' | '\u{00FD}' | '\u{00FF}' => "y",
        '\u{00DE}' | '\u{00FE}' => "th",
        '\u{00DF}' => "ss",
        '\u{0100}'..='\u{0105}' => "a",
        '\u{0106}'..='\u{010D}' => "c",
        '\u{010E}'..='\u{0111}' => "d",
        '\u{0112}'..='\u{011B}' => "e",
        '\u{011C}'..='\u{0123}' => "g",
        '\u{0124}'..='\u{0127}' => "h",
        '\u{0128}'..='\u{0131}' => "i",
        '\u{0132}'..='\u{0133}' => "ij",
        '\u{0134}'..='\u{0135}' => "j",
        '\u{0136}'..='\u{0138}' => "k",
        '\u{0139}'..='\u{0142}' => "l",
        '\u{0143}'..='\u{014B}' => "n",
        '\u{014C}'..='\u{0151}' => "o",
        '\u{0152}'..='\u{0153}' => "oe",
        '\u{0154}'..='\u{0159}' => "r",
        '\u{015A}'..='\u{0161}' => "s",
        '\u{0162}'..='\u{0167}' => "t",
        '\u{0168}'..='\u{0173}' => "u",
        '\u{0174}'..='\u{0175}' => "w",
        '\u{0176}'..='\u{0178}' => "y",
        '\u{0179}'..='\u{017E}' => "z",
        '\u{017F}' => "s",
        _ => return None,
    })
}

/// A JSON string field trimmed to an owned String, empty when the value is
/// absent or not a string. The literal "null" some services hand back for a
/// missing field (theaudiodb does this) folds to empty too, so a caller
/// never has to special-case it. Every provider parses its JSON through this.
fn string(value: Option<&serde_json::Value>) -> String {
    let s = value.and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
    if s.eq_ignore_ascii_case("null") {
        String::new()
    } else {
        s.to_string()
    }
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
    fn normalize_folded_takes_the_accents_off_too() {
        assert_eq!(normalize_folded("Beyoncé!"), "beyonce");
        assert_eq!(normalize_folded("Sigur Rós"), "sigur ros");
        assert_eq!(normalize_folded("Motörhead"), "motorhead");
        assert_eq!(normalize_folded("Mylène Farmer"), "mylene farmer");
        assert_eq!(normalize_folded("Straße"), "strasse");
        assert_eq!(normalize_folded("Antonín Dvořák"), "antonin dvorak");
        assert_eq!(normalize_folded("Daft Punk"), "daft punk");
        assert_eq!(normalize_folded("米津玄師"), "米津玄師");
    }

    #[test]
    fn normalize_collapses_and_trims_separators() {
        assert_eq!(normalize("  Air - Talkie Walkie  "), "air talkie walkie");
        assert_eq!(normalize("Sunday!!! (Live)"), "sunday live");
        assert_eq!(normalize("AC/DC"), "ac dc");
        assert_eq!(normalize("Blink-182"), "blink 182");
    }

    /// Symbol-only names fold to empty, and `similarity` scores an empty side
    /// 0, so "!!!" and "+/-" never collide.
    #[test]
    fn punctuation_only_names_fold_empty_and_do_not_collide() {
        assert_eq!(normalize("!!!"), "");
        assert_eq!(normalize("+/-"), "");
        assert_eq!(similarity("!!!", "+/-"), 0.0);
        assert_eq!(similarity("!!!", "!!!"), 0.0);
    }

    #[test]
    fn similarity_is_word_set_overlap() {
        assert_eq!(similarity("Better Harder", "Harder Better"), 1.0);
        let partial = similarity("one more time", "one more");
        assert!((partial - 2.0 / 3.0).abs() < 1e-6);
        assert!(similarity("a b c", "a b c d") > similarity("a b c", "a b c d e f"));
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
    fn confidence_orders_exact_partial_none() {
        let query = TrackQuery {
            artist: "Air".into(),
            title: "La Femme d'Argent".into(),
            album: "Moon Safari".into(),
            duration_secs: Some(430.0),
        };
        let exact = confidence(
            &query,
            &candidate("La Femme d'Argent", "Air", "Moon Safari", Some(430.0)),
        );
        let partial = confidence(
            &query,
            &candidate("La Femme d'Argent", "Nobody", "Wrong", Some(120.0)),
        );
        let none = confidence(
            &query,
            &candidate("Unrelated", "Nobody", "Wrong", Some(10.0)),
        );
        assert!(exact > partial);
        assert!(partial > none);
        assert!(exact > 0.9);
    }

    #[test]
    fn title_outweighs_artist() {
        let query = TrackQuery {
            artist: "Right Artist".into(),
            title: "Right Title".into(),
            album: String::new(),
            duration_secs: None,
        };
        let title_hit = confidence(&query, &candidate("Right Title", "Wrong Artist", "", None));
        let artist_hit = confidence(&query, &candidate("Wrong Title", "Right Artist", "", None));
        assert!(title_hit > artist_hit);
    }

    #[test]
    fn confidence_stays_in_unit_range() {
        let query = TrackQuery {
            artist: "Air".into(),
            title: "Sexy Boy".into(),
            album: "Moon Safari".into(),
            duration_secs: Some(298.0),
        };
        let best = confidence(
            &query,
            &candidate("Sexy Boy", "Air", "Moon Safari", Some(298.0)),
        );
        let worst = confidence(&query, &candidate("", "", "", None));
        assert!((0.0..=1.0).contains(&best));
        assert!((0.0..=1.0).contains(&worst));
    }

    #[test]
    fn cache_computes_once_negatives_included() {
        let cache: SessionCache<Vec<i32>> = SessionCache::default();
        let mut runs = 0;
        let mut run = |value: Vec<i32>| {
            cache.get_or_compute("k".into(), || {
                runs += 1;
                Ok(value)
            })
        };
        assert_eq!(run(Vec::new()).unwrap(), Vec::<i32>::new());
        assert_eq!(run(vec![1, 2, 3]).unwrap(), Vec::<i32>::new());
        assert_eq!(runs, 1);
    }

    #[test]
    fn cache_does_not_store_errors() {
        let cache: SessionCache<Vec<i32>> = SessionCache::default();
        assert!(
            cache
                .get_or_compute("k".into(), || Err("boom".into()))
                .is_err()
        );
        let mut runs = 0;
        let got = cache
            .get_or_compute("k".into(), || {
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
        assert_eq!(query_key(&a), query_key(&b));
        let c = TrackQuery {
            title: "One More Time".into(),
            ..a.clone()
        };
        assert_ne!(query_key(&a), query_key(&c));
    }

    fn art(album: &str, artist: &str) -> ArtCandidate {
        ArtCandidate {
            provider: "test",
            album: album.into(),
            artist: artist.into(),
            thumb_url: String::new(),
            full_url: String::new(),
            width: 0,
            height: 0,
        }
    }

    /// The wrong album by the right artist (issue #79) has to score under the
    /// presence's 0.5 bar.
    #[test]
    fn art_confidence_prefers_the_right_album() {
        let query = TrackQuery {
            artist: "Daft Punk".into(),
            title: "One More Time".into(),
            album: "Discovery".into(),
            duration_secs: None,
        };
        let right = art_confidence(&query, &art("Discovery", "Daft Punk"));
        let reissue = art_confidence(&query, &art("Discovery (Deluxe Edition)", "Daft Punk"));
        let wrong = art_confidence(&query, &art("Human After All", "Daft Punk"));
        assert!(right > reissue);
        assert!(reissue > wrong);
        assert!(right > 0.9);
        assert!(reissue >= 0.5);
        assert!(wrong < 0.5);
    }

    #[test]
    fn art_confidence_handles_missing_fields() {
        let query = TrackQuery {
            artist: "Air".into(),
            title: "Sexy Boy".into(),
            album: String::new(),
            duration_secs: None,
        };
        assert_eq!(art_confidence(&query, &art("Sexy Boy", "")), 1.0);
        assert!(art_confidence(&query, &art("Moon Safari", "Air")) < 0.5);
    }

    #[test]
    fn missing_fields_score_neutral_not_zero() {
        let query = TrackQuery {
            artist: "Boards of Canada".into(),
            title: "Roygbiv".into(),
            album: String::new(),
            duration_secs: None,
        };
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
