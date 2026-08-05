//! The artwork service's durable half per the components contract: 256px
//! thumbnails generated once per cover and cached in a dedicated SQLite
//! DB. A track's row is keyed by file identity (path, mtime, size) so a
//! changed file regenerates and an unchanged one never touches the audio
//! file again; the JPEG bytes live in a content-addressed pool shared by
//! every track showing the same cover, so an album's twelve tracks (or
//! the same cover copied across a discography) store one image and pay
//! one decode, not twelve. Tracks without art cache that answer too, so
//! an artless album costs one cover search ever, not one per launch.
//! Blocking file and DB work; run it off the UI thread.

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
    // Track rows replaced since the last open (a changed cover re-keys the
    // row to a new image) may have left their old image behind with nothing
    // pointing at it; one sweep gives that disk back.
    conn.execute(
        "DELETE FROM images WHERE hash NOT IN
             (SELECT art_hash FROM thumbs WHERE art_hash <> 0)",
        [],
    )?;
    Ok(conn)
}

/// The thumbnail cache's migration ladder. This is a cache, not a source of
/// truth, so a future step that cannot cheaply ALTER through a shape change is
/// free to drop and let the next scan regenerate, unlike the library store.
/// Step 1 is the baseline converge; step 2 pools the image bytes by content.
/// See [`crate::migrate`].
const MIGRATIONS: &[crate::migrate::Migration] = &[
    crate::migrate::Migration {
        name: "baseline",
        up: baseline,
    },
    crate::migrate::Migration {
        name: "dedup-images",
        up: dedup_images,
    },
];

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

/// Step 2: the image bytes move out of the track rows into a pool keyed by
/// content hash, so tracks sharing a cover share one row of JPEG instead of
/// carrying a copy each. Existing thumbs are hashed and pooled in place -
/// the encoder is deterministic, so byte-identical covers collapse - then
/// the per-track blob column drops. (Migrated rows key on the encoded
/// bytes, fresh ones on the source bytes; the two never need to agree, a
/// row only has to find its own image, and a migrated row that invalidates
/// re-keys onto the fresh scheme.)
fn dedup_images(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS images (
            hash  INTEGER PRIMARY KEY,
            image BLOB NOT NULL
        );
        ALTER TABLE thumbs ADD COLUMN art_hash INTEGER NOT NULL DEFAULT 0;",
    )?;
    // One streaming pass: pool each row's blob, remember which hash it got.
    // Only (path, hash) pairs are held; the blobs stream through one at a
    // time, so a big cache migrates without loading itself into memory.
    let mut keyed: Vec<(String, i64)> = Vec::new();
    {
        let mut read = conn.prepare("SELECT path, image FROM thumbs WHERE length(image) > 0")?;
        let mut pool =
            conn.prepare("INSERT OR IGNORE INTO images (hash, image) VALUES (?1, ?2)")?;
        let mut rows = read.query([])?;
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            let hash = content_hash(&blob);
            pool.execute(rusqlite::params![hash, blob])?;
            keyed.push((path, hash));
        }
    }
    {
        let mut set = conn.prepare("UPDATE thumbs SET art_hash = ?2 WHERE path = ?1")?;
        for (path, hash) in keyed {
            set.execute(rusqlite::params![path, hash])?;
        }
    }
    conn.execute_batch("ALTER TABLE thumbs DROP COLUMN image;")?;
    Ok(())
}

/// The pool key for one image's bytes: FNV-1a folded to a nonzero value,
/// 0 staying free as a track row's no-art mark. A 64-bit content key over
/// a library's covers has collision odds far below what a regeneratable
/// cache needs to care about, at none of a cryptographic hash's cost.
fn content_hash(bytes: &[u8]) -> i64 {
    match crate::hash::fnv1a(bytes) {
        0 => 1,
        hash => hash as i64,
    }
}

/// The thumbnail for one track: JPEG bytes, or None when the track has no
/// art anywhere (or no longer stats). A hit is one point lookup; a miss
/// resolves the cover's bytes and checks the pool by their hash, so only
/// the first sight of a cover pays the decode and re-encode - the rest of
/// the album, and any other copy of the image, reuse the pooled row. The
/// no-art answer is stored too, so the next request never opens the audio
/// file. A cover caught mid-write stores nothing at all, so the finished
/// file gets a fresh look instead of half an image sticking. The
/// connection is shared across workers; the lock is held for the lookups,
/// never the file reads or the encode.
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
        let cached: Option<(String, i64, i64, Option<Vec<u8>>)> = conn
            .prepare_cached(
                "SELECT t.art_path, t.art_mtime, t.art_size, i.image
                 FROM thumbs t LEFT JOIN images i ON i.hash = t.art_hash
                 WHERE t.path = ?1 AND t.mtime = ?2 AND t.size = ?3",
            )
            .ok()?
            .query_row(rusqlite::params![key, mtime, size], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()
            .ok()?;
        if let Some((art_path, art_mtime, art_size, image)) = cached {
            // The audio file is unchanged; the row still holds only if the
            // cover it was built from is too. An embedded source (empty
            // art_path) rode the audio identity above and needs no re-stat.
            // A no-art row references no image and an undecodable cover an
            // empty one; both answer None.
            if art_path.is_empty() || art::identity(Path::new(&art_path)) == (art_mtime, art_size) {
                return image.filter(|bytes| !bytes.is_empty());
            }
        }
    }
    // A miss: resolve the cover source off the lock, then key its bytes.
    // The directory's identity is taken before the resolve reads it, the
    // same order the folder cover's own stat runs in: a cover dropped in
    // between the two then reads as a directory this row has never seen,
    // rather than being stamped as already accounted for.
    let (dir, dir_mtime, dir_size) = no_art_identity(path);
    let (art_hash, thumb, art_path, art_mtime, art_size, whole) = match art::cover_art_source(path)
    {
        art::Cover::Found { bytes, source, .. } => {
            let hash = content_hash(&bytes);
            // Bytes that stop short of their end marker are a file still
            // landing on disk. Serve what decodes, store nothing: the
            // finished cover deserves the row, not this.
            let whole = art::complete(&bytes);
            // A cover seen before - the rest of this album, the same file
            // in another folder - skips the decode and re-encode whole.
            let pooled: Option<Vec<u8>> = {
                let conn = conn.lock().unwrap();
                let hit = conn
                    .prepare_cached("SELECT image FROM images WHERE hash = ?1")
                    .ok()?
                    .query_row([hash], |r| r.get(0))
                    .optional()
                    .ok()?;
                hit
            };
            let thumb = match pooled {
                Some(image) => image,
                None => {
                    // First sight: encode off the lock, then pool the result.
                    // Bytes that will not decode pool an empty image, so the
                    // failure caches and dedups the same as a success.
                    let encoded = encode(&bytes).unwrap_or_default();
                    if whole {
                        let conn = conn.lock().unwrap();
                        conn.prepare_cached(
                            "INSERT OR IGNORE INTO images (hash, image) VALUES (?1, ?2)",
                        )
                        .ok()?
                        .execute(rusqlite::params![hash, encoded])
                        .ok()?;
                    }
                    encoded
                }
            };
            let (art_path, art_mtime, art_size) = source_identity(&source);
            (hash, thumb, art_path, art_mtime, art_size, whole)
        }
        // A cover file is sitting there whose bytes aren't an image yet: a
        // download that has created the file and not filled it. The folder's
        // mtime moved when the file was created and won't move again when
        // the bytes land, so a negative entry stored now would answer for
        // this album forever. Store nothing, the same as bytes caught short
        // of their end marker.
        art::Cover::Settling => (0, Vec::new(), dir, dir_mtime, dir_size, false),
        // No art: hash 0 references no pooled image, and the negative entry
        // keys on the directory's identity, so a cover dropped in later
        // bumps its mtime and forces a fresh look.
        art::Cover::None => (0, Vec::new(), dir, dir_mtime, dir_size, true),
    };
    if whole {
        let conn = conn.lock().unwrap();
        conn.prepare_cached(
            "INSERT OR REPLACE INTO thumbs \
             (path, mtime, size, art_path, art_mtime, art_size, art_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .ok()?
        .execute(rusqlite::params![
            key, mtime, size, art_path, art_mtime, art_size, art_hash
        ])
        .ok()?;
    }
    (!thumb.is_empty()).then_some(thumb)
}

/// Empty the store and give its disk back: every row and pooled image
/// deleted, then a VACUUM so the file shrinks instead of keeping the
/// pages free. Thumbnails regenerate on demand. Blocking; run off the
/// UI thread.
pub fn clear(conn: &Mutex<Connection>) {
    let conn = conn.lock().unwrap();
    let _ = conn.execute("DELETE FROM thumbs", []);
    let _ = conn.execute("DELETE FROM images", []);
    let _ = conn.execute_batch("VACUUM;");
}

/// A resolved cover's cache identity: empty path for embedded art (the
/// audio file's own identity covers it), the identity the cover file
/// carried going into the read for folder art.
fn source_identity(source: &art::ArtSource) -> (String, i64, i64) {
    match source {
        art::ArtSource::Embedded => (String::new(), 0, 0),
        art::ArtSource::Folder { file, mtime, size } => {
            (file.to_string_lossy().into_owned(), *mtime, *size)
        }
    }
}

/// The negative entry's identity for a track with no art anywhere: its
/// parent directory, stored the same way it re-stats so the two compare
/// cleanly.
fn no_art_identity(path: &Path) -> (String, i64, i64) {
    match path.parent() {
        Some(dir) => {
            let (mtime, size) = art::identity(dir);
            (dir.to_string_lossy().into_owned(), mtime, size)
        }
        None => (String::new(), 0, 0),
    }
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

    fn count(conn: &Mutex<Connection>, table: &str) -> i64 {
        conn.lock()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// A small real JPEG the thumbnail encoder accepts, its pixels seeded
    /// so two calls with different seeds produce different files.
    fn jpeg(side: u32, seed: u8) -> Vec<u8> {
        let pixels: Vec<u8> = (0..side * side * 3)
            .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
            .collect();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode(&pixels, side, side, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

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
                rusqlite::params![
                    track.to_string_lossy(),
                    mtime,
                    size,
                    b"cached-cover".as_slice()
                ],
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

    /// The pooling migration collapses byte-identical blobs to one image
    /// row, re-keys every track row onto the pool, and drops the per-track
    /// blob column.
    #[test]
    fn migration_pools_existing_duplicate_rows() {
        let dir = std::env::temp_dir().join("rox-thumbs-pool-migrate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("thumbs.db");
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
            // Two tracks of one album sharing a cover, one track of another.
            for (path, blob) in [
                ("/m/a/1.mp3", b"cover-a".as_slice()),
                ("/m/a/2.mp3", b"cover-a".as_slice()),
                ("/m/b/1.mp3", b"cover-b".as_slice()),
            ] {
                conn.execute(
                    "INSERT INTO thumbs (path, mtime, size, image) VALUES (?1, 1, 1, ?2)",
                    rusqlite::params![path, blob],
                )
                .unwrap();
            }
        }

        let conn = Mutex::new(open(&db).unwrap());
        assert_eq!(count(&conn, "thumbs"), 3, "every track row survives");
        assert_eq!(count(&conn, "images"), 2, "the shared cover pools to one");
        let has_blob_column = conn
            .lock()
            .unwrap()
            .prepare("SELECT 1 FROM pragma_table_info('thumbs') WHERE name = 'image'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(!has_blob_column, "the per-track blob column is gone");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tracks sharing a cover - the same album, or the same image copied
    /// into another folder - pool one image row and all serve it.
    #[test]
    fn identical_covers_pool_one_image() {
        let dir = std::env::temp_dir().join("rox-thumbs-pool");
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let cover = jpeg(8, 1);
        std::fs::write(a.join("cover.jpg"), &cover).unwrap();
        std::fs::write(b.join("cover.jpg"), &cover).unwrap();
        // Dummy audio: the tags will not read, so the folder cover answers.
        for track in [a.join("1.mp3"), a.join("2.mp3"), b.join("1.mp3")] {
            std::fs::write(track, b"not audio").unwrap();
        }

        let conn = Mutex::new(open(&dir.join("thumbs.db")).unwrap());
        let first = thumbnail(&conn, &a.join("1.mp3")).expect("a thumbnail");
        assert_eq!(thumbnail(&conn, &a.join("2.mp3")).as_ref(), Some(&first));
        assert_eq!(thumbnail(&conn, &b.join("1.mp3")).as_ref(), Some(&first));
        assert_eq!(count(&conn, "thumbs"), 3, "each track keeps its own row");
        assert_eq!(
            count(&conn, "images"),
            1,
            "one pooled image serves all three"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cover still downloading leaves no row behind, so the finished
    /// file is read fresh. Without this the row would pair a thumbnail
    /// built from half an image with the identity the cover settles on,
    /// and the truncated cover would show forever.
    #[test]
    fn a_half_written_cover_caches_nothing() {
        let dir = std::env::temp_dir().join("rox-thumbs-partial");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let track = dir.join("1.mp3");
        std::fs::write(&track, b"not audio").unwrap();
        let cover = jpeg(64, 1);
        std::fs::write(dir.join("cover.jpg"), &cover[..cover.len() / 2]).unwrap();

        let conn = Mutex::new(open(&dir.join("thumbs.db")).unwrap());
        thumbnail(&conn, &track);
        assert_eq!(count(&conn, "thumbs"), 0, "the partial cover keys nothing");
        assert_eq!(count(&conn, "images"), 0, "and pools nothing");

        // The download lands: the next look reads the whole cover and this
        // one does cache.
        std::fs::write(dir.join("cover.jpg"), &cover).unwrap();
        let whole = thumbnail(&conn, &track).expect("a thumbnail");
        assert_eq!(count(&conn, "thumbs"), 1);
        assert_eq!(thumbnail(&conn, &track).as_ref(), Some(&whole));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cover file that exists but holds no image yet leaves no row
    /// behind either. This is the download that preallocates: the folder's
    /// mtime moved when the file was created and never moves again, so a
    /// no-art row keyed on the folder here would outlive the download and
    /// the album would show blank forever.
    #[test]
    fn a_cover_with_no_image_bytes_yet_caches_nothing() {
        let dir = std::env::temp_dir().join("rox-thumbs-preallocated");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let track = dir.join("1.mp3");
        std::fs::write(&track, b"not audio").unwrap();
        std::fs::write(dir.join("cover.jpg"), [0u8; 64]).unwrap();

        let conn = Mutex::new(open(&dir.join("thumbs.db")).unwrap());
        assert!(thumbnail(&conn, &track).is_none());
        assert_eq!(count(&conn, "thumbs"), 0, "the unfilled cover keys nothing");

        // Filling the file leaves the folder's mtime where it was, so only
        // the missing row lets the finished cover through.
        std::fs::write(dir.join("cover.jpg"), jpeg(8, 1)).unwrap();
        assert!(thumbnail(&conn, &track).is_some(), "the cover lands");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An album with nothing to show still caches that answer, so an
    /// artless folder costs one cover search ever rather than one a launch.
    #[test]
    fn an_artless_folder_caches_its_answer() {
        let dir = std::env::temp_dir().join("rox-thumbs-artless");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let track = dir.join("1.mp3");
        std::fs::write(&track, b"not audio").unwrap();

        let conn = Mutex::new(open(&dir.join("thumbs.db")).unwrap());
        assert!(thumbnail(&conn, &track).is_none());
        assert_eq!(count(&conn, "thumbs"), 1, "the no-art answer is stored");
        assert_eq!(count(&conn, "images"), 0, "and references no image");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A changed folder cover regenerates under a new pool key, and the
    /// image nothing references anymore is swept on the next open.
    #[test]
    fn changed_cover_regenerates_and_open_sweeps_orphans() {
        let dir = std::env::temp_dir().join("rox-thumbs-sweep");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("thumbs.db");
        std::fs::write(dir.join("cover.jpg"), jpeg(8, 1)).unwrap();
        let track = dir.join("1.mp3");
        std::fs::write(&track, b"not audio").unwrap();

        let conn = Mutex::new(open(&db).unwrap());
        let old = thumbnail(&conn, &track).expect("a thumbnail");

        // A new cover with a different size, so the art identity misses
        // even inside the same mtime second.
        std::fs::write(dir.join("cover.jpg"), jpeg(16, 2)).unwrap();
        let new = thumbnail(&conn, &track).expect("a regenerated thumbnail");
        assert_ne!(old, new);
        assert_eq!(count(&conn, "images"), 2, "the old image lingers orphaned");

        drop(conn);
        let conn = Mutex::new(open(&db).unwrap());
        assert_eq!(count(&conn, "images"), 1, "reopening sweeps the orphan");
        assert_eq!(
            thumbnail(&conn, &track).as_ref(),
            Some(&new),
            "the live row still serves"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
