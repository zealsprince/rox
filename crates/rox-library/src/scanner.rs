//! The scan pipeline: walk folders, read tags through lofty (ADR 4's single
//! metadata layer), and upsert rows in batches. Unchanged files, judged by
//! (mtime, size), are skipped without touching their tags. A file whose tags
//! will not parse still gets indexed under its filename, so the library never
//! silently loses a playable file.
//!
//! On a big library the cost is filesystem stats, not tag reads: adding one
//! file still means confirming the other tens of thousands are unchanged. So
//! the walk leans on the directory entry's kind instead of a stat per file,
//! and each batch stats and reads its files in parallel across cores. exfat
//! and network mounts pay per-stat latency, and that is exactly what the
//! parallelism hides.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use lofty::prelude::*;
use rayon::prelude::*;
use rusqlite::Connection;

use crate::store;
use crate::TrackRow;

/// Formats the playback engine decodes today (ADR 2).
const EXTENSIONS: &[&str] = &["flac", "mp3", "wav"];
const BATCH: usize = 512;

#[derive(Default)]
pub struct ScanSummary {
    /// Files read and upserted this scan.
    pub indexed: usize,
    /// Files skipped because (mtime, size) matched the stored row.
    pub unchanged: usize,
    /// Files indexed by filename because their tags would not read.
    pub untagged: usize,
    /// The scan stopped early because `progress` said to. Everything
    /// counted above is in the store; the rest of the walk never ran.
    pub aborted: bool,
}

/// Scan `root` recursively into the store. Blocking; run it off the UI thread.
/// `progress` is called once per file with (scanned, total, path), from the
/// worker threads and out of walk order, so a UI can report the scan live;
/// returning false stops the scan after flushing what it has. Cancellation
/// lands at batch boundaries, which a parallel batch reaches in a fraction
/// of a serial one.
pub fn scan(
    conn: &mut Connection,
    root: &Path,
    progress: impl Fn(usize, usize, &Path) -> bool + Sync,
) -> rusqlite::Result<ScanSummary> {
    let known = store::local_files(conn)?;
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();
    let total = files.len();

    let mut summary = ScanSummary::default();
    let scanned = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    for chunk in files.chunks(BATCH) {
        // Stat and, where changed, tag-read the whole batch at once. The map
        // only touches the shared `known` set for reads, so it needs no locks.
        // Each worker ticks progress for its own file; a false return raises
        // the flag the batch loop honors after flushing.
        let outcomes: Vec<Outcome> = chunk
            .par_iter()
            .map(|path| {
                let outcome = process_file(path, &known);
                let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                if !progress(done, total, path) {
                    cancelled.store(true, Ordering::Relaxed);
                }
                outcome
            })
            .collect();

        let mut batch: Vec<TrackRow> = Vec::with_capacity(chunk.len());
        for outcome in outcomes {
            match outcome {
                Outcome::Missing => {}
                Outcome::Unchanged => summary.unchanged += 1,
                Outcome::Indexed { row, untagged } => {
                    if untagged {
                        summary.untagged += 1;
                    }
                    summary.indexed += 1;
                    batch.push(*row);
                }
            }
        }
        if !batch.is_empty() {
            store::insert_batch(conn, &batch)?;
        }

        if cancelled.load(Ordering::Relaxed) {
            summary.aborted = true;
            break;
        }
    }
    Ok(summary)
}

/// Re-read exactly these files and upsert their rows, the write-back half
/// of the metadata writer's contract: after a commit the library re-reads
/// the written paths and converges without a rescan. The empty known set
/// forces every read, since the caller only names files it just changed.
/// Files that vanished since are skipped, matching what a scan would do.
/// Blocking; run it off the UI thread.
pub fn reindex(conn: &mut Connection, paths: &[PathBuf]) -> rusqlite::Result<usize> {
    let known = HashMap::new();
    let rows: Vec<TrackRow> = paths
        .par_iter()
        .filter_map(|path| match process_file(path, &known) {
            Outcome::Indexed { row, .. } => Some(*row),
            _ => None,
        })
        .collect();
    if !rows.is_empty() {
        store::insert_batch(conn, &rows)?;
    }
    Ok(rows.len())
}

/// What one file's stat-and-read produced, kept separate from the store write
/// so the read can run in parallel and the write stays a single transaction.
enum Outcome {
    /// The file vanished or would not stat between the walk and the read.
    Missing,
    /// Stored (mtime, size) matched, so the row was left untouched.
    Unchanged,
    /// A row to upsert, boxed so the variant stays near its siblings'
    /// size; `untagged` marks the filename-only fallback.
    Indexed { row: Box<TrackRow>, untagged: bool },
}

/// Stat one file and, if it changed, read its tags into a row. Pure and
/// self-contained so `par_iter` can run it across the batch.
fn process_file(path: &Path, known: &HashMap<String, (i64, u64)>) -> Outcome {
    let Ok(meta) = std::fs::metadata(path) else {
        return Outcome::Missing;
    };
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let path_str = path.to_string_lossy().into_owned();
    if known.get(&path_str) == Some(&(mtime, size)) {
        return Outcome::Unchanged;
    }

    let (row, untagged) = match read_tags(path) {
        Some(tags) => (tags, false),
        None => (fallback_row(path), true),
    };
    Outcome::Indexed {
        row: Box::new(TrackRow {
            path: path_str,
            size,
            mtime,
            ..row
        }),
        untagged,
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // The directory read already carries each entry's kind on most
        // filesystems, so file_type() answers dir-or-file without a stat per
        // file. Symlinks and the rare filesystem that omits the kind fall back
        // to a stat, which still follows linked folders like the old walk did.
        let is_dir = match entry.file_type() {
            Ok(ft) if !ft.is_symlink() => ft.is_dir(),
            _ => path.is_dir(),
        };
        if is_dir {
            collect(&path, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        {
            out.push(path);
        }
    }
}

/// Tag read isolated per file: a malformed file that errors or panics
/// lofty's parser costs that one file its tags, never the scan.
fn read_tags(path: &Path) -> Option<TrackRow> {
    let file = catch_unwind(AssertUnwindSafe(|| {
        lofty::probe::Probe::open(path).and_then(|p| p.options(crate::parse_opts()).read())
    }))
    .ok()?
    .ok()?;
    let mut row = fallback_row(path);
    row.duration_ms = file.properties().duration().as_millis() as u32;
    // The parsed type beats the extension a fallback row guesses from; a
    // format outside the match keeps the guess.
    if let Some(codec) = match file.file_type() {
        lofty::file::FileType::Flac => Some("flac"),
        lofty::file::FileType::Mpeg => Some("mp3"),
        lofty::file::FileType::Wav => Some("wav"),
        _ => None,
    } {
        row.codec = codec.to_string();
    }
    row.bitrate_kbps = file
        .properties()
        .audio_bitrate()
        .or_else(|| file.properties().overall_bitrate())
        .unwrap_or(0) as u16;
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        let text =
            |v: Option<std::borrow::Cow<'_, str>>| v.map(|s| s.into_owned()).unwrap_or_default();
        if let Some(t) = tag.title().filter(|t| !t.is_empty()) {
            row.title = t.into_owned();
        }
        row.artist = text(tag.artist());
        // The credited album artist falls back to the track artist at scan
        // time, so a plain album groups the same whether or not it carries
        // the tag, and only compilations split the two.
        row.album_artist = tag
            .get_string(lofty::tag::ItemKey::AlbumArtist)
            .filter(|s| !s.is_empty())
            .unwrap_or(&row.artist)
            .to_string();
        row.album = text(tag.album());
        row.genre = text(tag.genre());
        row.year = tag.date().map(|d| d.year).unwrap_or(0);
        row.disc_no = tag.disk().unwrap_or(0) as u16;
        row.track_no = tag.track().unwrap_or(0) as u16;
    }
    // The rating reads concretely per format - FMPS lives in TXXX frames
    // and unmapped Vorbis keys, which the generic tag above never carries.
    row.rating = crate::rating::read(path, file.file_type()).unwrap_or(0);
    Some(row)
}

/// The row a file gets when its tags cannot be read: filename as title,
/// the extension as codec, everything else empty. path/size/mtime are
/// filled in by the caller.
fn fallback_row(path: &Path) -> TrackRow {
    TrackRow {
        path: String::new(),
        title: filename_title(path),
        artist: String::new(),
        album_artist: String::new(),
        album: String::new(),
        genre: String::new(),
        year: 0,
        disc_no: 0,
        track_no: 0,
        duration_ms: 0,
        codec: path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default(),
        bitrate_kbps: 0,
        rating: 0,
        size: 0,
        mtime: 0,
    }
}

fn filename_title(path: &Path) -> String {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    name.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{self, Change, Field};

    /// The write-back loop the metadata writer's contract names: commit,
    /// reindex the written path, and the store row converges without a
    /// rescan - even when the row already exists.
    #[test]
    fn reindex_rereads_named_files() {
        let dir = std::env::temp_dir().join("rox-scanner-reindex");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The writer test fixture's minimal MPEG stream, enough for the
        // full tag read this module runs.
        let mut audio = Vec::new();
        for frame in 0..3u32 {
            audio.extend([0xFF, 0xFB, 0x90, 0x00]);
            audio.extend((0..413u32).map(|i| ((frame * 413 + i) * 7 % 251) as u8));
        }
        let path = dir.join("track.mp3");
        std::fs::write(&path, &audio).unwrap();

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();

        let title = |conn: &Connection| {
            store::meta_for_path(conn, path.to_str().unwrap())
                .unwrap()
                .unwrap()
                .title
        };
        let retitle = |value: &str| {
            writer::commit(
                &path,
                &[Change {
                    field: Field::Title,
                    value: Some(value.to_string()),
                }],
            )
            .unwrap();
        };

        retitle("First");
        assert_eq!(reindex(&mut conn, std::slice::from_ref(&path)).unwrap(), 1);
        assert_eq!(title(&conn), "First");

        retitle("Second");
        assert_eq!(reindex(&mut conn, std::slice::from_ref(&path)).unwrap(), 1);
        assert_eq!(title(&conn), "Second");

        // A written rating imports on the re-read, half points intact.
        writer::commit(
            &path,
            &[Change {
                field: Field::Rating,
                value: Some("7.5".into()),
            }],
        )
        .unwrap();
        assert_eq!(reindex(&mut conn, std::slice::from_ref(&path)).unwrap(), 1);
        let rating: i64 = conn
            .query_row(
                "SELECT rating FROM tracks WHERE path = ?1",
                [path.to_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rating, 75);
    }
}
