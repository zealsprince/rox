//! The artwork service's durable half per the components contract: 256px
//! thumbnails generated once per cover and cached in a dedicated SQLite
//! DB, keyed by file identity (path, mtime, size) so a changed file
//! regenerates and an unchanged one never touches the audio file again.
//! Tracks without art cache that answer too, so an artless album costs
//! one cover search ever, not one per launch. Blocking file and DB work;
//! run it off the UI thread.

use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension};

use crate::art;

/// The longest side of a stored thumbnail, per the artwork service
/// contract: enough for a grid tile or a header block at any density,
/// small enough that the decode costs nothing.
pub const SIZE: u32 = 256;

/// Stored thumbnails are JPEG: covers are photographic, and at this size
/// lossless would cost an order of magnitude more disk for no visible
/// gain.
const QUALITY: u8 = 85;

/// Open (creating as needed) a thumbnail DB, the same WAL shape as the
/// library store.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS thumbs (
            path  TEXT PRIMARY KEY,
            mtime INTEGER NOT NULL,
            size  INTEGER NOT NULL,
            image BLOB NOT NULL
        );",
    )?;
    Ok(conn)
}

/// The thumbnail for one track: JPEG bytes, or None when the track has no
/// art anywhere (or no longer stats). A hit is one point lookup; a miss
/// reads the cover, downscales, and persists the result - the no-art
/// answer stored as an empty blob - so the next request never opens the
/// audio file. The connection is shared across workers; the lock is held
/// for the lookups, never the decode.
pub fn thumbnail(conn: &Mutex<Connection>, path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let key = path.to_string_lossy();
    {
        let conn = conn.lock().unwrap();
        let cached: Option<Vec<u8>> = conn
            .prepare_cached("SELECT image FROM thumbs WHERE path = ?1 AND mtime = ?2 AND size = ?3")
            .ok()?
            .query_row(rusqlite::params![key, mtime, size], |r| r.get(0))
            .optional()
            .ok()?;
        if let Some(bytes) = cached {
            return (!bytes.is_empty()).then_some(bytes);
        }
    }
    let thumb = generate(path);
    let conn = conn.lock().unwrap();
    conn.prepare_cached(
        "INSERT OR REPLACE INTO thumbs (path, mtime, size, image) VALUES (?1, ?2, ?3, ?4)",
    )
    .ok()?
    .execute(rusqlite::params![
        key,
        mtime,
        size,
        thumb.as_deref().unwrap_or_default()
    ])
    .ok()?;
    thumb
}

/// Empty the store and give its disk back: every row deleted, then a
/// VACUUM so the file shrinks instead of keeping the pages free.
/// Thumbnails regenerate on demand. Blocking; run off the UI thread.
pub fn clear(conn: &Mutex<Connection>) {
    let conn = conn.lock().unwrap();
    let _ = conn.execute("DELETE FROM thumbs", []);
    let _ = conn.execute_batch("VACUUM;");
}

/// One cover into thumbnail form: decode, downscale to [`SIZE`] on the
/// longest side, re-encode. None when the track has no art or the art
/// won't decode.
fn generate(path: &Path) -> Option<Vec<u8>> {
    let (bytes, _mime) = art::cover_art(path)?;
    let cover = image::load_from_memory(&bytes).ok()?;
    let small = cover.thumbnail(SIZE, SIZE).into_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY)
        .encode(
            small.as_raw(),
            small.width(),
            small.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(out)
}
