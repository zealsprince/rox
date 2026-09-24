//! The artwork service's durable half: 256px thumbnails cached in their own
//! SQLite DB. A track row is keyed by file identity (path, mtime, size); the
//! JPEG bytes live in a content-addressed pool, so an album's tracks share
//! one image and one decode. No-art answers are cached too. Blocking.

use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, OptionalExtension};

use crate::art;

/// The longest side of a stored thumbnail, per the artwork service contract.
pub const SIZE: u32 = 256;

/// JPEG: covers are photographic, and lossless costs ten times the disk.
const QUALITY: u8 = 85;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    crate::migrate::run(&conn, MIGRATIONS)?;
    // Sweep images a re-keyed row left with nothing pointing at them.
    conn.execute(
        "DELETE FROM images WHERE hash NOT IN
             (SELECT art_hash FROM thumbs WHERE art_hash <> 0)",
        [],
    )?;
    Ok(conn)
}

/// A cache, not a source of truth: a future step that can't cheaply ALTER
/// may drop and let the next scan regenerate. See [`crate::migrate`].
const MIGRATIONS: &[crate::migrate::Migration] = &[
    crate::migrate::Migration {
        name: "baseline",
        up: baseline,
        rescan: false,
    },
    crate::migrate::Migration {
        name: "dedup-images",
        up: dedup_images,
        rescan: false,
    },
];

/// The pre-ladder schema. art_path/art_mtime/art_size pin a folder cover's
/// own identity, re-statted on a hit. Embedded art has an empty art_path;
/// a no-art row records the directory, so a new cover.jpg misses.
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
    // Add the art_* columns to a pre-art cache in place; on a fresh table the
    // ALTER fails harmlessly.
    for column in [
        "ALTER TABLE thumbs ADD COLUMN art_path TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE thumbs ADD COLUMN art_mtime INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE thumbs ADD COLUMN art_size INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = conn.execute(column, []);
    }
    Ok(())
}

/// Move the image bytes into a content-hash pool. Migrated rows key on the
/// encoded bytes, fresh ones on the source; a row only needs to find its own.
fn dedup_images(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS images (
            hash  INTEGER PRIMARY KEY,
            image BLOB NOT NULL
        );
        ALTER TABLE thumbs ADD COLUMN art_hash INTEGER NOT NULL DEFAULT 0;",
    )?;
    // Stream the blobs; only (path, hash) pairs are held.
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

/// FNV-1a forced nonzero: 0 marks a no-art row.
fn content_hash(bytes: &[u8]) -> i64 {
    match crate::hash::fnv1a(bytes) {
        0 => 1,
        hash => hash as i64,
    }
}

/// JPEG bytes, or None for no art anywhere. A path that doesn't stat answers
/// from the pool (see [`stored`]). Only a cover's first sight pays the
/// decode; a cover caught mid-write stores nothing. The lock is held for
/// lookups only, never file reads or the encode.
pub fn thumbnail(conn: &Mutex<Connection>, path: &Path) -> Option<Vec<u8>> {
    // Not a file (a station URL, a server's song id), or a deleted one: the
    // pooled answer is all there is.
    let Ok(meta) = std::fs::metadata(path) else {
        return stored(conn, &path.to_string_lossy());
    };

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
            // The row holds if its cover source is unchanged. Empty art_path is
            // embedded art, covered by the audio identity above.
            if art_path.is_empty() || art::identity(Path::new(&art_path)) == (art_mtime, art_size) {
                return image.filter(|bytes| !bytes.is_empty());
            }
        }
    }
    // Stat the directory before resolving, the same order the folder cover's
    // stat runs in, so a cover dropped in between reads as unseen.
    let (dir, dir_mtime, dir_size) = no_art_identity(path);
    let (art_hash, thumb, art_path, art_mtime, art_size, whole) = match art::cover_art_source(path)
    {
        art::Cover::Found { bytes, source, .. } => {
            let hash = content_hash(&bytes);
            // A cover short of its end marker is still downloading: serve, don't store.
            let whole = art::complete(&bytes);
            let pooled: Option<Vec<u8>> = {
                let conn = conn.lock().unwrap();

                conn.prepare_cached("SELECT image FROM images WHERE hash = ?1")
                    .ok()?
                    .query_row([hash], |r| r.get(0))
                    .optional()
                    .ok()?
            };
            let thumb = match pooled {
                Some(image) => image,
                None => {
                    // Undecodable bytes pool an empty image, so failures cache too.
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
        // A preallocated download: the folder mtime won't move again when the
        // bytes land, so a negative entry now would stick forever. Store nothing.
        art::Cover::Settling => (0, Vec::new(), dir, dir_mtime, dir_size, false),
        // The negative entry keys on the directory, so a new cover misses.
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

/// [`store_bytes`]'s read side: the key alone decides.
fn stored(conn: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    let conn = conn.lock().unwrap();

    let image: Vec<u8> = conn
        .prepare_cached(
            "SELECT i.image FROM thumbs t JOIN images i ON i.hash = t.art_hash
             WHERE t.path = ?1",
        )
        .ok()?
        .query_row([key], |r| r.get(0))
        .optional()
        .ok()??;

    (!image.is_empty()).then_some(image)
}

/// Store a thumbnail from bytes that aren't a file (a server cover, a
/// station favicon), keyed by the row's own key with zero identity columns.
/// Takes the connection directly: a source sync owns it outright.
pub fn store_bytes(conn: &Connection, bytes: &[u8], key: &str) -> Option<Vec<u8>> {
    let hash = content_hash(bytes);

    let pooled: Option<Vec<u8>> = conn
        .prepare_cached("SELECT image FROM images WHERE hash = ?1")
        .ok()?
        .query_row([hash], |r| r.get(0))
        .optional()
        .ok()?;

    let thumb = match pooled {
        Some(image) => image,

        None => {
            // Undecodable bytes pool an empty image, so failures cache too.
            let encoded = encode(bytes).unwrap_or_default();
            conn.prepare_cached("INSERT OR IGNORE INTO images (hash, image) VALUES (?1, ?2)")
                .ok()?
                .execute(rusqlite::params![hash, encoded])
                .ok()?;

            encoded
        }
    };

    conn.prepare_cached(
        "INSERT OR REPLACE INTO thumbs \
         (path, mtime, size, art_path, art_mtime, art_size, art_hash) \
         VALUES (?1, 0, 0, '', 0, 0, ?2)",
    )
    .ok()?
    .execute(rusqlite::params![key, hash])
    .ok()?;

    (!thumb.is_empty()).then_some(thumb)
}

/// Delete every row and image, then VACUUM so the file shrinks. Blocking.
pub fn clear(conn: &Mutex<Connection>) {
    let conn = conn.lock().unwrap();
    let _ = conn.execute("DELETE FROM thumbs", []);
    let _ = conn.execute("DELETE FROM images", []);
    let _ = conn.execute_batch("VACUUM;");
}

/// Empty path for embedded art; the pre-read identity for folder art.
fn source_identity(source: &art::ArtSource) -> (String, i64, i64) {
    match source {
        art::ArtSource::Embedded => (String::new(), 0, 0),
        art::ArtSource::Folder { file, mtime, size } => {
            (file.to_string_lossy().into_owned(), *mtime, *size)
        }
    }
}

/// The parent directory, stored the same way it re-stats.
fn no_art_identity(path: &Path) -> (String, i64, i64) {
    match path.parent() {
        Some(dir) => {
            let (mtime, size) = art::identity(dir);
            (dir.to_string_lossy().into_owned(), mtime, size)
        }
        None => (String::new(), 0, 0),
    }
}

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

    #[test]
    fn migrates_pre_art_columns_and_serves_existing_rows() {
        let dir = std::env::temp_dir().join("rox-thumbs-migrate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("thumbs.db");

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
        assert_eq!(
            thumbnail(&conn, &track).as_deref(),
            Some(b"cached-cover".as_slice())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

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

        std::fs::write(dir.join("cover.jpg"), &cover).unwrap();
        let whole = thumbnail(&conn, &track).expect("a thumbnail");
        assert_eq!(count(&conn, "thumbs"), 1);
        assert_eq!(thumbnail(&conn, &track).as_ref(), Some(&whole));

        let _ = std::fs::remove_dir_all(&dir);
    }

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

        std::fs::write(dir.join("cover.jpg"), jpeg(8, 1)).unwrap();
        assert!(thumbnail(&conn, &track).is_some(), "the cover lands");

        let _ = std::fs::remove_dir_all(&dir);
    }

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

    #[test]
    fn a_non_file_key_reads_back_what_was_stored() {
        let conn = Connection::open_in_memory().unwrap();
        crate::migrate::run(&conn, MIGRATIONS).unwrap();

        let stored = store_bytes(&conn, &jpeg(8, 1), "https://host/jazz").expect("a thumbnail");

        let conn = Mutex::new(conn);
        assert_eq!(
            thumbnail(&conn, Path::new("https://host/jazz")).as_ref(),
            Some(&stored),
            "the station's own key serves its picture"
        );
        assert!(
            thumbnail(&conn, Path::new("https://host/nothing")).is_none(),
            "a key nothing was stored under stays blank"
        );
    }

    #[test]
    fn a_non_file_key_with_undecodable_bytes_reads_as_no_art() {
        let conn = Connection::open_in_memory().unwrap();
        crate::migrate::run(&conn, MIGRATIONS).unwrap();

        assert!(store_bytes(&conn, b"not an image", "sg-1").is_none());

        let conn = Mutex::new(conn);
        assert!(thumbnail(&conn, Path::new("sg-1")).is_none());
    }

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

        // A different size, so the identity misses inside the same mtime second.
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
