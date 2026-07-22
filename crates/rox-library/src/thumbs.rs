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
    crate::migrate::run(&conn, MIGRATIONS)?;
    Ok(conn)
}

/// The thumbnail cache's migration ladder. This is a cache, not a source of
/// truth, so a future step that cannot cheaply ALTER through a shape change is
/// free to drop and let the next scan regenerate, unlike the library store.
/// For now step 1 is the baseline converge. See [`crate::migrate`].
const MIGRATIONS: &[crate::migrate::Migration] = &[crate::migrate::Migration {
    name: "baseline",
    up: baseline,
}];

/// The baseline cache schema, the whole thing as it stood before the version
/// ladder. art_path/art_mtime/art_size pin the cover's own identity so a folder
/// cover that changes without touching the audio file still invalidates: the
/// audio (mtime,size) matches, then the recorded art source is re-stat'd.
/// Embedded art records an empty art_path (the audio file's own identity
/// already covers it); a no-art negative entry records the directory, so a
/// newly dropped cover.jpg bumps the dir mtime and misses.
fn baseline(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS thumbs (
            path      TEXT PRIMARY KEY,
            mtime     INTEGER NOT NULL,
            size      INTEGER NOT NULL,
            art_path  TEXT NOT NULL DEFAULT '',
            art_mtime INTEGER NOT NULL DEFAULT 0,
            art_size  INTEGER NOT NULL DEFAULT 0,
            image     BLOB NOT NULL
        );",
    )?;
    // A cache from before the art_* columns keeps the old four-column shape,
    // and CREATE TABLE IF NOT EXISTS leaves it as is, so every lookup would
    // query columns that aren't there and fail. Add them in place; on a fresh
    // table they already exist and the ALTER is a harmless no-op we ignore.
    for column in [
        "ALTER TABLE thumbs ADD COLUMN art_path TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE thumbs ADD COLUMN art_mtime INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE thumbs ADD COLUMN art_size INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = conn.execute(column, []);
    }
    Ok(())
}

/// The (mtime, size) of a path, both zero when it will not stat. The cache
/// keys art sources on this, so a changed cover reads as a fresh identity.
fn identity_of(path: &Path) -> (i64, i64) {
    match std::fs::metadata(path) {
        Ok(meta) => (
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            meta.len() as i64,
        ),
        Err(_) => (0, 0),
    }
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
        let cached: Option<(Vec<u8>, String, i64, i64)> = conn
            .prepare_cached(
                "SELECT image, art_path, art_mtime, art_size FROM thumbs \
                 WHERE path = ?1 AND mtime = ?2 AND size = ?3",
            )
            .ok()?
            .query_row(rusqlite::params![key, mtime, size], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()
            .ok()?;
        if let Some((bytes, art_path, art_mtime, art_size)) = cached {
            // The audio file is unchanged; the row still holds only if the
            // cover it was built from is too. An embedded source (empty
            // art_path) rode the audio identity above and needs no re-stat.
            if art_path.is_empty()
                || identity_of(Path::new(&art_path)) == (art_mtime, art_size)
            {
                return (!bytes.is_empty()).then_some(bytes);
            }
        }
    }
    let (thumb, art_path, art_mtime, art_size) = generate(path);
    let conn = conn.lock().unwrap();
    conn.prepare_cached(
        "INSERT OR REPLACE INTO thumbs \
         (path, mtime, size, art_path, art_mtime, art_size, image) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .ok()?
    .execute(rusqlite::params![
        key,
        mtime,
        size,
        art_path,
        art_mtime,
        art_size,
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

/// One cover into thumbnail form plus the cover's cache identity: decode,
/// downscale to [`SIZE`] on the longest side, re-encode. The thumb is None
/// when the track has no art or the art won't decode; the identity always
/// comes back so the negative entry keys on the directory and a newly added
/// cover invalidates it.
///
/// The identity is (art_path, art_mtime, art_size): empty path for embedded
/// art (the audio file's own identity covers it), the cover file for folder
/// art, the parent directory for no art at all.
fn generate(path: &Path) -> (Option<Vec<u8>>, String, i64, i64) {
    let (thumb, source) = match art::cover_art_source(path) {
        Some((bytes, _mime, source)) => (encode(&bytes), Some(source)),
        None => (None, None),
    };
    let (art_path, art_mtime, art_size) = match source {
        Some(art::ArtSource::Embedded) => (String::new(), 0, 0),
        Some(art::ArtSource::Folder(file)) => {
            let (m, s) = identity_of(&file);
            (file.to_string_lossy().into_owned(), m, s)
        }
        // No art: key the negative entry on the directory's identity, so a
        // cover dropped in later bumps its mtime and forces a fresh look.
        // Stored the same way it re-stats, so the two compare cleanly.
        None => match path.parent() {
            Some(dir) => {
                let (m, s) = identity_of(dir);
                (dir.to_string_lossy().into_owned(), m, s)
            }
            None => (String::new(), 0, 0),
        },
    };
    (thumb, art_path, art_mtime, art_size)
}

/// One cover's bytes into a downscaled JPEG thumbnail. None when the bytes
/// won't decode as an image.
fn encode(bytes: &[u8]) -> Option<Vec<u8>> {
    let cover = image::load_from_memory(bytes).ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A cache written before the art_* columns must keep working: open()
    /// adds the columns in place, so an existing thumbnail still reads back
    /// instead of every cover going blank. This is the exact shape that
    /// regressed once - the seven-column lookup against a four-column table.
    #[test]
    fn migrates_pre_art_columns_and_serves_existing_rows() {
        let dir = std::env::temp_dir().join("rox-thumbs-migrate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("thumbs.db");

        // A real track file so thumbnail() can stat it for the (mtime, size)
        // half of the key; the bytes need not be audio for a cache hit.
        let track = dir.join("track.mp3");
        std::fs::write(&track, b"stand-in for audio").unwrap();
        let meta = std::fs::metadata(&track).unwrap();
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Seed the old four-column cache with a thumbnail for this track,
        // the way a build from before tonight left it on disk.
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE thumbs (
                    path  TEXT PRIMARY KEY,
                    mtime INTEGER NOT NULL,
                    size  INTEGER NOT NULL,
                    image BLOB NOT NULL
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO thumbs (path, mtime, size, image) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![track.to_string_lossy(), mtime, size, b"cached-cover".as_slice()],
            )
            .unwrap();
        }

        let conn = Mutex::new(open(&db).unwrap());
        // Without the migration this returns None (the lookup fails to
        // prepare against the missing columns) and the cover shows blank.
        assert_eq!(
            thumbnail(&conn, &track).as_deref(),
            Some(b"cached-cover".as_slice())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
