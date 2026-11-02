//! The control socket's track answers (ADR 22): the one stored-row shape
//! every track payload carries, `library.track` with its opt-in sections, and
//! the paged `library.history`.
//!
//! Rows come from SQLite through [`rox_library::track_row`], not the
//! projection, which holds only what browsing needs. Anything that reads more
//! than one row, a tag, or a file runs on the background executor over its
//! own connection; the UI thread only gathers what the player alone knows.
//! Nothing here fetches: every section is what rox already has stored.

use std::path::{Path, PathBuf};

use gpui::App;
use serde_json::{Map, Value, json};

use rox_ipc::{Request, RpcError};
use rox_library::cue::TrackKey;
use rox_library::listens::{self, Cursor};
use rox_library::lyrics::{self, Subject};
use rox_library::rusqlite::Connection;
use rox_library::track_row::{self, StoredTrack};
use rox_library::{embeddings, replaygain, store, tempo, writer};
use rox_net::providers::TrackQuery;
use rox_panel_api::panel::AppState;
use rox_playback::IcyTitle;
use rox_services::{artists, release_facts};

const HISTORY_LIMIT_DEFAULT: usize = 50;
const HISTORY_LIMIT_MAX: usize = 500;

const NEIGHBOURS_DEFAULT: usize = 10;
const NEIGHBOURS_MAX: usize = 50;

/// Hundredths for gains and tempo, six places for peaks: the precision the
/// tags carry, without the f32 noise a straight widen prints.
fn rounded(value: f32, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (f64::from(value) * scale).round() / scale
}

fn text(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

fn nonzero<T: Default + PartialEq + Copy>(n: T) -> Option<T> {
    (n != T::default()).then_some(n)
}

/// The stored row as every track payload carries it. Every key is always
/// present: null where the row has no value, and every column null but the
/// key's own when the track isn't in the library. The title falls back to
/// the file name, as a player shows an untagged file.
pub fn row_json(key: &TrackKey, row: Option<&StoredTrack>) -> Value {
    let fallback_title = || {
        key.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    };
    let gain = row.map(|r| r.replay_gain).unwrap_or_default();
    let has_gain = gain.track_db.is_some()
        || gain.track_peak.is_some()
        || gain.album_db.is_some()
        || gain.album_peak.is_some();
    let replaygain_source = row.filter(|_| has_gain).map(|r| match r.rg_source {
        replaygain::Source::Tags => "tag",
        replaygain::Source::Measured => "analyzed",
    });
    let (bpm, bpm_source) = match row.and_then(|r| r.bpm.map(|bpm| (bpm, r.bpm_source))) {
        Some((bpm, tempo::Source::Tags)) => (Some(rounded(bpm, 2)), Some("tag")),
        Some((bpm, tempo::Source::Measured)) => (Some(rounded(bpm, 2)), Some("analyzed")),
        // A refused row holds no tempo, so there's nothing to attribute.
        Some((_, tempo::Source::Refused)) | None => (None, None),
    };
    let radio = key.source.as_ref() == rox_library::stations::SOURCE;

    json!({
        "id": row.map(|r| r.id),
        "path": key.path,
        "sub": key.sub,
        "source": key.source.as_ref(),
        "key": key.to_fragment(),
        "title": row.and_then(|r| text(&r.title)).or_else(fallback_title),
        "artist": row.and_then(|r| text(&r.artist)),
        "album_artist": row.and_then(|r| text(&r.album_artist)),
        "album": row.and_then(|r| text(&r.album)),
        "genre": row.and_then(|r| text(&r.genre)),
        "year": row.and_then(|r| nonzero(r.year)),
        "date": row.and_then(|r| {
            rox_library::year::Date {
                year: r.year,
                month: r.month,
                day: r.day,
            }
            .iso()
        }),
        "disc_no": row.and_then(|r| nonzero(r.disc_no)),
        "track_no": row.and_then(|r| nonzero(r.track_no)),
        "duration_ms": row.and_then(|r| nonzero(r.duration_ms)),
        "codec": row.and_then(|r| text(&r.codec)),
        "bitrate_kbps": row.and_then(|r| nonzero(r.bitrate_kbps)),
        "sample_rate_hz": row.and_then(|r| nonzero(r.sample_rate_hz)),
        "bit_depth": row.and_then(|r| nonzero(r.bit_depth)),
        // Zero is unrated, not a rating.
        "rating": row.and_then(|r| nonzero(r.rating)),
        // Zero plays is a real count, so only a missing row is null.
        "plays": row.map(|r| r.plays),
        "added": row.and_then(|r| nonzero(r.added)),
        "size": row.and_then(|r| nonzero(r.size)),
        "comment": row.and_then(|r| text(&r.comment)),
        "title_sort": row.and_then(|r| text(&r.title_sort)),
        "artist_sort": row.and_then(|r| text(&r.artist_sort)),
        "album_artist_sort": row.and_then(|r| text(&r.album_artist_sort)),
        "album_sort": row.and_then(|r| text(&r.album_sort)),
        "remote_url": row.and_then(|r| text(&r.remote_url)),
        "remote_live": row.filter(|_| radio).map(|r| r.remote_live),
        "track_gain_db": gain.track_db.map(|v| rounded(v, 2)),
        "track_peak": gain.track_peak.map(|v| rounded(v, 6)),
        "album_gain_db": gain.album_db.map(|v| rounded(v, 2)),
        "album_peak": gain.album_peak.map(|v| rounded(v, 6)),
        "replaygain_source": replaygain_source,
        "bpm": bpm,
        "bpm_source": bpm_source,
        "start_ms": row.and_then(|r| r.span).map(|(start, _)| start),
        "end_ms": row.and_then(|r| r.span).and_then(|(_, end)| end),
    })
}

/// A station names the song it's playing in its stream, not its row: the
/// song's artist and title over the row, the station's name as the album,
/// and no length. The same rule as [`rox_services::radio::live_tags`].
pub fn overlay_live(track: &mut Value, title: &IcyTitle) {
    let station = track["title"].take();
    track["album"] = station;
    track["title"] = json!(text(&title.title));
    track["artist"] = json!(text(&title.artist));
    track["duration_ms"] = Value::Null;
}

/// Its own connection, so a long read never holds the catalog's.
fn open(db_path: &Path) -> Result<Connection, RpcError> {
    store::open(db_path).map_err(|e| RpcError::app(format!("the library couldn't be read: {e}")))
}

fn read<T>(result: rox_library::rusqlite::Result<T>) -> Result<T, RpcError> {
    result.map_err(|e| RpcError::app(format!("the library couldn't be read: {e}")))
}

fn unknown_key(fragment: &str) -> RpcError {
    RpcError::app(format!(
        "nothing in the library under {fragment}; see the key field of library.search"
    ))
}

/// The opt-in sections of `library.track`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Include {
    Tags,
    Lyrics,
    Acoustic,
    Enrichment,
    Stats,
}

impl Include {
    pub const ALL: [Include; 5] = [
        Include::Tags,
        Include::Lyrics,
        Include::Acoustic,
        Include::Enrichment,
        Include::Stats,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Include::Tags => "tags",
            Include::Lyrics => "lyrics",
            Include::Acoustic => "acoustic",
            Include::Enrichment => "enrichment",
            Include::Stats => "stats",
        }
    }
}

/// The `include` param: absent is none, anything else must be an array of
/// known names. Repeats collapse.
pub fn includes(params: &Value) -> Result<Vec<Include>, String> {
    let valid = || {
        Include::ALL
            .iter()
            .map(|include| include.name())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let Some(raw) = params.get("include").filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let Some(names) = raw.as_array() else {
        return Err(format!("include is an array of names: {}", valid()));
    };

    let mut out = Vec::new();
    for name in names {
        let found = name
            .as_str()
            .and_then(|name| Include::ALL.into_iter().find(|i| i.name() == name));
        let Some(include) = found else {
            return Err(format!(
                "unknown include {name}; the valid ones are {}",
                valid()
            ));
        };

        if !out.contains(&include) {
            out.push(include);
        }
    }
    Ok(out)
}

/// What `library.track` needs from the UI thread before it goes to the
/// background: the player's view of a playing station, the live model, and
/// where things are kept.
struct TrackAsk {
    key: TrackKey,
    fragment: String,
    include: Vec<Include>,
    limit: usize,
    db_path: PathBuf,
    lyrics: Option<Subject>,
    lyrics_dir: PathBuf,
    model: String,
}

pub fn track(state: &AppState, request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let Some(fragment) = params.get("key").and_then(Value::as_str) else {
        responder.respond(Err(RpcError::invalid_params(
            "track takes {\"key\": \"..\", \"include\"?: [..], \"limit\"?: n}",
        )));
        return;
    };
    let include = match includes(&params) {
        Ok(include) => include,
        Err(reason) => {
            responder.respond(Err(RpcError::invalid_params(reason)));
            return;
        }
    };

    let key = super::ipc::key_for(fragment);
    let player = state.player.read(cx);
    // The playing station's song, so its sheet is the song's, not the stream's.
    let live = player
        .now_playing()
        .filter(|now| now.key == key)
        .and_then(|_| player.live_title());
    let ask = TrackAsk {
        lyrics: rox_services::lyrics::subject_for(&key, live.as_ref()),
        key,
        fragment: fragment.to_owned(),
        include,
        limit: params
            .get("limit")
            .and_then(Value::as_u64)
            .map(|l| l as usize)
            .unwrap_or(NEIGHBOURS_DEFAULT)
            .clamp(1, NEIGHBOURS_MAX),
        db_path: state.library.read(cx).db_path(),
        lyrics_dir: rox_core::settings::lyrics_dir(),
        model: rox_services::acoustic::acoustic_source().id().to_owned(),
    };

    cx.background_executor()
        .spawn(async move { responder.respond(answer_track(&ask)) })
        .detach();
}

/// A section that can't be answered is null, with the reason under
/// `reasons`.
fn answer_track(ask: &TrackAsk) -> Result<Value, RpcError> {
    let conn = open(&ask.db_path)?;
    let row =
        read(track_row::by_key(&conn, &ask.key))?.ok_or_else(|| unknown_key(&ask.fragment))?;

    let mut out = Map::new();
    let mut reasons = Map::new();
    out.insert("track".into(), row_json(&ask.key, Some(&row)));

    for include in &ask.include {
        let section = match include {
            Include::Tags => tags_section(&row),
            Include::Lyrics => lyrics_section(ask.lyrics.as_ref(), &ask.lyrics_dir),
            Include::Acoustic => acoustic_section(&conn, &row, &ask.model, ask.limit),
            Include::Enrichment => Ok(enrichment_section(&conn, &row)),
            Include::Stats => Ok(json!({
                "plays": row.plays,
                "last_played": row.last_played,
            })),
        };
        match section {
            Ok(value) => {
                out.insert(include.name().into(), value);
            }
            Err(reason) => {
                out.insert(include.name().into(), Value::Null);
                reasons.insert(include.name().into(), json!(reason));
            }
        }
    }

    out.insert("reasons".into(), Value::Object(reasons));
    Ok(Value::Object(out))
}

fn field_name(field: &writer::Field) -> Option<&'static str> {
    use writer::Field;

    Some(match field {
        Field::Title => "title",
        Field::Artist => "artist",
        Field::Album => "album",
        Field::AlbumArtist => "album_artist",
        Field::Genre => "genre",
        Field::Year => "year",
        Field::TrackNo => "track_no",
        Field::DiscNo => "disc_no",
        Field::Comment => "comment",
        Field::Composer => "composer",
        Field::TitleSort => "title_sort",
        Field::ArtistSort => "artist_sort",
        Field::AlbumArtistSort => "album_artist_sort",
        Field::AlbumSort => "album_sort",
        Field::Lyrics => "lyrics",
        Field::Rating => "rating",
        // Customs come through the unknown read, under their own keys.
        Field::ReplayGain(_) | Field::Tempo | Field::Custom(_) | Field::Unknown(_) => return None,
    })
}

/// Every tag in the file: the fields the editor has names for, then
/// everything else under the key the format writes it with. ReplayGain is in
/// the row, not here. Both reads run under the writer's panic isolation.
fn tags_section(row: &StoredTrack) -> Result<Value, String> {
    if row.source != rox_library::cue::LOCAL {
        return Err("a remote track has no file to read".into());
    }
    if row.sub > 0 {
        return Err("a cue track shares its file, whose tags describe the whole disc".into());
    }
    let path = Path::new(&row.path);
    if !writer::supported(path) {
        return Err("rox's tag reader doesn't handle this file's format".into());
    }

    let mut tags = Vec::new();
    for (field, value) in writer::read(path)? {
        if let Some(name) = field_name(&field) {
            tags.push(json!({ "key": name, "value": value }));
        }
    }
    for (key, value) in writer::read_unknown(path)? {
        tags.push(match value {
            writer::UnknownValue::Text(text) => json!({ "key": key, "value": text }),
            // Pictures and private frames are sized, never decoded.
            writer::UnknownValue::Binary(bytes) => {
                json!({ "key": key, "value": null, "binary_bytes": bytes })
            }
        });
    }
    Ok(Value::Array(tags))
}

/// The sheet the Lyrics panel would show, resolved the same way: sidecar,
/// then the store, then the file's tag. A sheet a plugin or provider found
/// reads from wherever it was saved.
fn lyrics_section(subject: Option<&Subject>, dir: &Path) -> Result<Value, String> {
    let Some(subject) = subject else {
        return Err("the station hasn't named a song, so there's no sheet to look for".into());
    };
    let Some(sheet) = lyrics::load(subject, Some(dir)) else {
        return Err("rox has no lyrics for this track".into());
    };

    let (source, file) = match &sheet.source {
        lyrics::Source::Tag => ("embedded", None),
        lyrics::Source::Sidecar(path) => ("sidecar", Some(path.clone())),
        lyrics::Source::Store(path) => ("store", Some(path.clone())),
    };
    let words = sheet.lines.iter().any(|line| !line.words.is_empty());
    let synced = match (sheet.synced, words) {
        (false, _) => "none",
        (true, false) => "line",
        (true, true) => "word",
    };
    let lines: Option<Vec<Value>> = sheet.synced.then(|| {
        sheet
            .lines
            .iter()
            .map(|line| {
                let words: Vec<Value> = line
                    .words
                    .iter()
                    .map(|word| {
                        json!({
                            "at": word.at,
                            "text": line.text.get(word.range.clone()).unwrap_or_default(),
                        })
                    })
                    .collect();
                json!({ "at": line.at, "end": line.end, "text": line.text, "words": words })
            })
            .collect()
    });

    Ok(json!({
        "source": source,
        "file": file,
        "synced": synced,
        "text": sheet.text,
        "lines": lines,
    }))
}

/// Raw cosine against the live model, the number the Similar column prints;
/// distance is one minus it, so 0 is identical.
fn acoustic_section(
    conn: &Connection,
    row: &StoredTrack,
    model: &str,
    limit: usize,
) -> Result<Value, String> {
    let embedded = embeddings::has(conn, row.id, model).map_err(|e| e.to_string())?;
    if !embedded {
        return Ok(json!({ "model": model, "embedded": false, "neighbours": [] }));
    }

    let near = embeddings::nearest(conn, row.id, model, limit).map_err(|e| e.to_string())?;
    let ids: Vec<i64> = near.iter().map(|(id, _)| *id).collect();
    let rows = track_row::by_ids(conn, &ids).map_err(|e| e.to_string())?;
    let neighbours: Vec<Value> = near
        .iter()
        .filter_map(|(id, score)| {
            let near = rows.get(id)?;
            Some(json!({
                "key": near.key().to_fragment(),
                "title": text(&near.title),
                "artist": text(&near.artist),
                "distance": rounded(1.0 - score, 4),
            }))
        })
        .collect();
    Ok(json!({ "model": model, "embedded": true, "neighbours": neighbours }))
}

/// Stored data only: the meta tables' sort names, the biography store's
/// cached artist sheet, the cached release facts, and the genre table. A
/// plugin's sheet laid over the biography is flagged with `plugin_text`.
fn enrichment_section(conn: &Connection, row: &StoredTrack) -> Value {
    let album_artist = (!row.album_artist.is_empty() && row.album_artist != row.artist)
        .then(|| artist_json(conn, &row.album_artist))
        .flatten();
    let genres: Vec<Value> = rox_library::genre::split(&row.genre)
        .map(|name| {
            let meta = track_row::genre_meta(conn, name).ok().flatten();
            json!({
                "name": name,
                "folds_into": meta.as_ref().and_then(|m| text(&m.fold_into)),
                "display": meta.as_ref().and_then(|m| text(&m.display)),
                "art": meta.as_ref().and_then(|m| text(&m.art)),
            })
        })
        .collect();

    json!({
        "artist": artist_json(conn, &row.artist),
        "album_artist": album_artist,
        "album": album_json(conn, row),
        "genres": genres,
    })
}

fn artist_json(conn: &Connection, name: &str) -> Option<Value> {
    if name.trim().is_empty() {
        return None;
    }

    let meta = track_row::artist_meta(conn, name).ok().flatten();
    let stored = artists::stored(name);
    let info = stored.as_ref().map(|s| &s.info);
    let profile = stored.as_ref().map(|s| &s.profile);
    Some(json!({
        "name": name,
        "sort_name": meta.as_ref().map(|m| &m.sort_name),
        "sort_source": meta.as_ref().map(|m| &m.source),
        "sort_fetched": meta.as_ref().map(|m| m.fetched),
        "bio": info.and_then(|i| text(&i.bio)),
        "url": info.and_then(|i| text(&i.url)),
        "tags": info.map(|i| &i.tags),
        "similar": info.map(|i| &i.similar),
        "listeners": info.and_then(|i| nonzero(i.listeners)),
        "playcount": info.and_then(|i| nonzero(i.playcount)),
        "country": profile.and_then(|p| text(&p.country)),
        "formed": profile.and_then(|p| p.formed),
        "ended": profile.and_then(|p| p.ended),
        "from_plugin": stored.as_ref().and_then(|s| s.from_plugin.clone()),
        "plugin_text": stored.as_ref().map(|s| s.plugin_text),
        "fetched": stored.as_ref().and_then(|s| nonzero(s.fetched)),
        "images": stored.as_ref().map(|s| json!({
            "portrait": s.portrait,
            "banner": s.banner,
            "background": s.background,
            "fanarts": s.fanarts,
        })),
    }))
}

fn album_json(conn: &Connection, row: &StoredTrack) -> Option<Value> {
    if row.album.trim().is_empty() {
        return None;
    }

    let meta = track_row::album_meta(conn, &row.album).ok().flatten();
    // Keyed the way the metadata panel caches them, by the track's own tags.
    let release = release_facts::stored(&TrackQuery {
        artist: row.artist.clone(),
        title: row.title.clone(),
        album: row.album.clone(),
        duration_secs: None,
    });
    Some(json!({
        "name": row.album,
        "sort_name": meta.as_ref().map(|m| &m.sort_name),
        "sort_source": meta.as_ref().map(|m| &m.source),
        "sort_fetched": meta.as_ref().map(|m| m.fetched),
        "release": release.as_ref().and_then(|(facts, _)| serde_json::to_value(facts).ok()),
        "release_fetched": release.as_ref().map(|(_, fetched)| *fetched),
    }))
}

/// Out over the wire as `played_at:id`, so two listens inside one second
/// still page apart.
pub fn cursor_text(cursor: Cursor) -> String {
    format!("{}:{}", cursor.played_at(), cursor.id())
}

/// `before` takes a cursor a page handed out, or a bare unix timestamp for
/// everything strictly earlier than that second. Absent starts at the newest.
pub fn parse_cursor(before: Option<&Value>) -> Result<Cursor, String> {
    let bad = || "before is a next cursor from library.history or a unix timestamp".to_string();

    match before {
        None | Some(Value::Null) => Ok(Cursor::START),
        Some(Value::Number(n)) => n.as_i64().map(|at| Cursor::new(at, 0)).ok_or_else(bad),
        Some(Value::String(s)) => {
            if let Ok(at) = s.parse::<i64>() {
                return Ok(Cursor::new(at, 0));
            }

            let (at, id) = s.split_once(':').ok_or_else(bad)?;
            match (at.parse(), id.parse()) {
                (Ok(at), Ok(id)) => Ok(Cursor::new(at, id)),
                _ => Err(bad()),
            }
        }
        Some(_) => Err(bad()),
    }
}

pub fn history(state: &AppState, request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let before = match parse_cursor(params.get("before")) {
        Ok(before) => before,
        Err(reason) => {
            responder.respond(Err(RpcError::invalid_params(reason)));
            return;
        }
    };
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|l| l as usize)
        .unwrap_or(HISTORY_LIMIT_DEFAULT)
        .clamp(1, HISTORY_LIMIT_MAX);
    let filter = params
        .get("key")
        .and_then(Value::as_str)
        .map(|fragment| (super::ipc::key_for(fragment), fragment.to_owned()));
    let db_path = state.library.read(cx).db_path();

    cx.background_executor()
        .spawn(async move {
            responder.respond(answer_history(&db_path, filter.as_ref(), before, limit))
        })
        .detach();
}

/// A listen whose track is gone keeps its snapshot key and tags, with a null
/// track.
fn answer_history(
    db_path: &Path,
    filter: Option<&(TrackKey, String)>,
    before: Cursor,
    limit: usize,
) -> Result<Value, RpcError> {
    let conn = open(db_path)?;
    let track_id = match filter {
        Some((key, fragment)) => Some(
            read(track_row::by_key(&conn, key))?
                .ok_or_else(|| unknown_key(fragment))?
                .id,
        ),
        None => None,
    };

    let (heard, next) = read(listens::page(&conn, before, limit, track_id))?;
    let ids: Vec<i64> = heard.iter().map(|h| h.track_id).collect();
    let rows = read(track_row::by_ids(&conn, &ids))?;
    let entries: Vec<Value> = heard
        .iter()
        .map(|h| {
            let row = rows.get(&h.track_id);
            json!({
                "played_at": h.played_at,
                "key": row.map(|r| r.key().to_fragment()).or_else(|| text(&h.path)),
                "heard": {
                    "title": text(&h.title),
                    "artist": text(&h.artist),
                    "album": text(&h.album),
                },
                "track": row.map(|r| row_json(&r.key(), Some(r))),
            })
        })
        .collect();

    Ok(json!({ "entries": entries, "next": next.map(cursor_text) }))
}

/// One keyed read per entry, off the UI thread. `id` stays the queue's
/// handle, so the row goes under `track`.
pub fn queue_list(state: &AppState, request: Request, cx: &mut App) {
    let player = state.player.read(cx);
    let queue_rev = player.queue_rev();
    let Some((entries, cursor)) = player.play_order() else {
        request.respond(Ok(
            json!({ "entries": [], "cursor": null, "queue_rev": queue_rev }),
        ));
        return;
    };
    let entries: Vec<(u64, TrackKey, bool)> = entries
        .iter()
        .map(|(id, key, explicit)| (*id, key.clone(), *explicit))
        .collect();
    let db_path = state.library.read(cx).db_path();
    let (_, _, responder) = request.into_parts();

    cx.background_executor()
        .spawn(async move {
            // An unreadable library still lists the queue, just without rows.
            let conn = open(&db_path).ok();
            let entries: Vec<Value> = entries
                .iter()
                .enumerate()
                .map(|(i, (id, key, explicit))| {
                    let row = conn
                        .as_ref()
                        .and_then(|conn| track_row::by_key(conn, key).ok().flatten());
                    json!({
                        "id": id,
                        "path": key.path,
                        "sub": key.sub,
                        "explicit": explicit,
                        "current": i == cursor,
                        "track": row_json(key, row.as_ref()),
                    })
                })
                .collect();
            responder.respond(Ok(json!({
                "entries": entries,
                "cursor": cursor,
                "queue_rev": queue_rev,
            })));
        })
        .detach();
}

/// The search hits' rows by projection id, read off the UI thread. A hit
/// whose row went away between the scan and the read is dropped.
pub fn search_rows(db_path: &Path, ids: &[i64]) -> Result<Vec<Value>, RpcError> {
    let conn = open(db_path)?;
    let rows = read(track_row::by_ids(&conn, ids))?;
    Ok(ids
        .iter()
        .filter_map(|id| rows.get(id))
        .map(|row| row_json(&row.key(), Some(row)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key the shared row carries, in one place for the shape tests.
    const ROW_KEYS: &[&str] = &[
        "id",
        "path",
        "sub",
        "source",
        "key",
        "title",
        "artist",
        "album_artist",
        "album",
        "genre",
        "year",
        "date",
        "disc_no",
        "track_no",
        "duration_ms",
        "codec",
        "bitrate_kbps",
        "sample_rate_hz",
        "bit_depth",
        "rating",
        "plays",
        "added",
        "size",
        "comment",
        "title_sort",
        "artist_sort",
        "album_artist_sort",
        "album_sort",
        "remote_url",
        "remote_live",
        "track_gain_db",
        "track_peak",
        "album_gain_db",
        "album_peak",
        "replaygain_source",
        "bpm",
        "bpm_source",
        "start_ms",
        "end_ms",
    ];

    fn assert_shape(row: &Value) {
        let object = row.as_object().expect("a row is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        let mut want = ROW_KEYS.to_vec();
        keys.sort_unstable();
        want.sort_unstable();
        assert_eq!(keys, want);
    }

    fn local(path: &str, sub: u16) -> TrackKey {
        TrackKey {
            source: rox_library::cue::local(),
            path: PathBuf::from(path),
            sub,
        }
    }

    #[test]
    fn a_full_row_fills_every_field_and_rounds_the_floats() {
        let key = local("/m/disc.flac", 2);
        let row = StoredTrack {
            id: 7,
            source: "local".into(),
            path: "/m/disc.flac".into(),
            sub: 2,
            title: "Joga".into(),
            artist: "Björk".into(),
            album_artist: "Björk".into(),
            album: "Homogenic".into(),
            genre: "Electronic".into(),
            year: 1997,
            month: 9,
            day: 22,
            disc_no: 1,
            track_no: 2,
            duration_ms: 305000,
            codec: "FLAC".into(),
            bitrate_kbps: 900,
            sample_rate_hz: 44100,
            bit_depth: 16,
            rating: 8,
            plays: 3,
            last_played: Some(1_700_000_000),
            added: 1_600_000_000,
            size: 40_000_000,
            comment: "rip".into(),
            title_sort: "Joga".into(),
            artist_sort: "Bjork".into(),
            album_artist_sort: "Bjork".into(),
            album_sort: "Homogenic".into(),
            replay_gain: replaygain::ReplayGain {
                track_db: Some(-7.35),
                track_peak: Some(0.987654),
                album_db: Some(-8.1),
                album_peak: Some(1.0),
            },
            rg_source: replaygain::Source::Measured,
            bpm: Some(127.84),
            bpm_source: tempo::Source::Measured,
            span: Some((61000, Some(366000))),
            ..StoredTrack::default()
        };

        let json = row_json(&key, Some(&row));
        assert_shape(&json);
        assert_eq!(json["key"], json!("/m/disc.flac#2"));
        assert_eq!(json["artist"], json!("Björk"));
        assert_eq!(json["bitrate_kbps"], json!(900));
        assert_eq!(json["track_gain_db"], json!(-7.35));
        assert_eq!(json["track_peak"], json!(0.987654));
        assert_eq!(json["replaygain_source"], json!("analyzed"));
        assert_eq!(json["bpm"], json!(127.84));
        assert_eq!(json["bpm_source"], json!("analyzed"));
        assert_eq!(
            (json["start_ms"].clone(), json["end_ms"].clone()),
            (json!(61000), json!(366000))
        );
        assert!(
            json["remote_live"].is_null(),
            "remote_live is a radio row's"
        );
        assert!(json["remote_url"].is_null());
    }

    #[test]
    fn absent_values_are_null_never_a_sentinel() {
        let key = local("/m/untagged.mp3", 0);
        let bare = StoredTrack {
            id: 1,
            source: "local".into(),
            path: "/m/untagged.mp3".into(),
            bpm: None,
            bpm_source: tempo::Source::Refused,
            ..StoredTrack::default()
        };

        let json = row_json(&key, Some(&bare));
        assert_shape(&json);
        for field in [
            "artist",
            "year",
            "date",
            "disc_no",
            "track_no",
            "duration_ms",
            "codec",
            "rating",
            "added",
            "size",
            "comment",
            "track_gain_db",
            "replaygain_source",
            "bpm",
            "bpm_source",
            "start_ms",
            "end_ms",
        ] {
            assert!(
                json[field].is_null(),
                "{field} should be null, got {}",
                json[field]
            );
        }
        assert_eq!(json["plays"], json!(0), "zero plays is a count");
        assert_eq!(json["title"], json!("untagged"), "the file name stands in");

        let tagged_tempo = StoredTrack {
            bpm: Some(174.0),
            bpm_source: tempo::Source::Tags,
            ..bare
        };
        let json = row_json(&key, Some(&tagged_tempo));
        assert_eq!(
            (json["bpm"].clone(), json["bpm_source"].clone()),
            (json!(174.0), json!("tag"))
        );
    }

    #[test]
    fn a_track_outside_the_library_keeps_the_shape_with_its_key_alone() {
        let key = local("/tmp/dropped.flac", 0);
        let json = row_json(&key, None);
        assert_shape(&json);
        assert!(json["id"].is_null());
        assert!(json["plays"].is_null());
        assert_eq!(json["path"], json!("/tmp/dropped.flac"));
        assert_eq!(json["title"], json!("dropped"));
    }

    #[test]
    fn a_station_row_says_whether_it_is_live_and_takes_the_announced_song() {
        let key = TrackKey {
            source: rox_library::cue::source_id(rox_library::stations::SOURCE),
            path: PathBuf::from("https://host/jazz"),
            sub: 0,
        };
        let row = StoredTrack {
            source: rox_library::stations::SOURCE.into(),
            path: "https://host/jazz".into(),
            title: "Jazz FM".into(),
            remote_url: "https://host/jazz".into(),
            remote_live: true,
            ..StoredTrack::default()
        };

        let mut json = row_json(&key, Some(&row));
        assert_eq!(json["remote_live"], json!(true));
        overlay_live(
            &mut json,
            &IcyTitle {
                artist: "Miles Davis".into(),
                title: "So What".into(),
            },
        );
        assert_eq!(json["title"], json!("So What"));
        assert_eq!(json["artist"], json!("Miles Davis"));
        assert_eq!(json["album"], json!("Jazz FM"));
        assert_shape(&json);
    }

    #[test]
    fn includes_take_known_names_and_refuse_the_rest_by_name() {
        assert_eq!(includes(&json!({})).unwrap(), Vec::new());
        assert_eq!(
            includes(&json!({ "include": ["stats", "tags", "stats"] })).unwrap(),
            vec![Include::Stats, Include::Tags]
        );
        assert_eq!(
            includes(&json!({ "include": Include::ALL.map(Include::name) })).unwrap(),
            Include::ALL.to_vec()
        );

        let err = includes(&json!({ "include": ["tags", "waveform"] })).unwrap_err();
        assert!(err.contains("\"waveform\""), "{err}");
        assert!(
            err.contains("tags, lyrics, acoustic, enrichment, stats"),
            "the refusal lists the valid names: {err}"
        );
        assert!(includes(&json!({ "include": "tags" })).is_err());
        assert!(includes(&json!({ "include": [3] })).is_err());
    }

    #[test]
    fn a_history_cursor_round_trips_and_a_timestamp_means_strictly_before() {
        let cursor = Cursor::new(1_700_000_000, 42);
        assert_eq!(
            parse_cursor(Some(&json!(cursor_text(cursor)))).unwrap(),
            cursor
        );
        assert_eq!(parse_cursor(None).unwrap(), Cursor::START);
        assert_eq!(parse_cursor(Some(&Value::Null)).unwrap(), Cursor::START);
        assert_eq!(
            parse_cursor(Some(&json!(1_700_000_000))).unwrap(),
            Cursor::new(1_700_000_000, 0)
        );
        assert_eq!(
            parse_cursor(Some(&json!("1700000000"))).unwrap(),
            Cursor::new(1_700_000_000, 0)
        );
        assert!(parse_cursor(Some(&json!("soon"))).is_err());
        assert!(parse_cursor(Some(&json!("1:two"))).is_err());
        assert!(parse_cursor(Some(&json!([1]))).is_err());
    }

    #[test]
    fn a_remote_or_cue_track_skips_the_tag_read_with_a_reason() {
        let remote = StoredTrack {
            source: "subsonic:home".into(),
            path: "song-1".into(),
            ..StoredTrack::default()
        };
        assert!(tags_section(&remote).unwrap_err().contains("remote"));

        let cue = StoredTrack {
            source: "local".into(),
            path: "/m/disc.flac".into(),
            sub: 3,
            ..StoredTrack::default()
        };
        assert!(tags_section(&cue).unwrap_err().contains("cue"));
    }

    /// A library file on disk with two tracks and some listens, for the
    /// answers that open their own connection.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-ipc-track-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let conn = store::open(&db).unwrap();
        store::init_schema(&conn).unwrap();
        for (path, title) in [("/m/1.flac", "One"), ("/m/2.flac", "Two")] {
            conn.execute(
                "INSERT INTO tracks (source, path, sub, title, artist, album, genre, year,
                    track_no, duration_ms, size, mtime)
                 VALUES ('local', ?1, 0, ?2, 'A', 'Al', 'rock', 2001, 1, 200000, 10, 0)",
                [path, title],
            )
            .unwrap();
        }
        for (track_id, played_at) in [(1, 100), (2, 200), (1, 200), (2, 300), (1, 400)] {
            conn.execute(
                "INSERT INTO listens (track_id, played_at, title, artist, album, genre, path)
                 VALUES (?1, ?2, 'heard', 'A', 'Al', 'rock', '')",
                [track_id, played_at],
            )
            .unwrap();
        }
        db
    }

    #[test]
    fn history_pages_through_the_socket_answer_and_narrows_by_key() {
        let db = fixture("history");

        let mut played = Vec::new();
        let mut before = Cursor::START;
        loop {
            let page = answer_history(&db, None, before, 2).unwrap();
            for entry in page["entries"].as_array().unwrap() {
                assert_shape(&entry["track"]);
                played.push(entry["played_at"].as_i64().unwrap());
            }
            match page["next"].as_str() {
                Some(next) => before = parse_cursor(Some(&json!(next))).unwrap(),
                None => break,
            }
        }
        assert_eq!(played, vec![400, 300, 200, 200, 100]);

        let one = (local("/m/1.flac", 0), "/m/1.flac".to_string());
        let page = answer_history(&db, Some(&one), Cursor::START, 50).unwrap();
        let keys: Vec<&str> = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, vec!["/m/1.flac"; 3]);
        assert!(page["next"].is_null());

        let gone = (local("/m/gone.flac", 0), "/m/gone.flac".to_string());
        assert!(answer_history(&db, Some(&gone), Cursor::START, 50).is_err());
    }

    #[test]
    fn a_track_answer_carries_the_row_its_sections_and_their_reasons() {
        let db = fixture("track");
        let ask = |fragment: &str, include: Vec<Include>| TrackAsk {
            key: local(fragment, 0),
            fragment: fragment.to_owned(),
            include,
            limit: NEIGHBOURS_DEFAULT,
            db_path: db.clone(),
            lyrics: Some(Subject::File(PathBuf::from(fragment))),
            lyrics_dir: db.with_file_name("lyrics"),
            model: "builtin-v1".into(),
        };

        let answer = answer_track(&ask(
            "/m/1.flac",
            vec![
                Include::Stats,
                Include::Acoustic,
                Include::Lyrics,
                Include::Tags,
            ],
        ))
        .unwrap();
        assert_shape(&answer["track"]);
        assert_eq!(answer["track"]["plays"], json!(3));
        assert_eq!(answer["stats"], json!({ "plays": 3, "last_played": 400 }));
        assert_eq!(answer["acoustic"]["embedded"], json!(false));
        // No sheet and no file on disk: both null, each with its reason.
        assert!(answer["lyrics"].is_null() && answer["reasons"]["lyrics"].is_string());
        assert!(answer["tags"].is_null() && answer["reasons"]["tags"].is_string());
        assert!(
            answer.get("enrichment").is_none(),
            "only asked-for sections appear"
        );

        let err = answer_track(&ask("/m/nowhere.flac", vec![])).unwrap_err();
        assert!(format!("{err:?}").contains("nothing in the library"));
    }
}
