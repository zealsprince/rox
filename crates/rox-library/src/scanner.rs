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

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use lofty::file::TaggedFile;
use lofty::flac::FlacFile;
use lofty::mpeg::MpegFile;
use lofty::prelude::*;
use rayon::prelude::*;
use rusqlite::Connection;

use crate::store;
use crate::TrackRow;

/// The audio extensions rox recognizes: what the scan indexes and what an
/// external open accepts, one list so the two never drift. Tracks the codec
/// set the engine is built with (ADR 2). Video containers (mp4, webm) stay
/// off the list so a scan never vacuums up a film library, and Opus is out
/// until symphonia ships a decoder for it.
pub const EXTENSIONS: &[&str] = &[
    "flac", "mp3", "wav", "ogg", "oga", "m4a", "m4b", "aac", "aif", "aiff",
    "aifc", "mka", "caf",
];
const BATCH: usize = 512;

#[derive(Default)]
pub struct ScanSummary {
    /// Files read and upserted this scan.
    pub indexed: usize,
    /// Files skipped because (mtime, size) matched the stored row.
    pub unchanged: usize,
    /// Files indexed by filename because their tags would not read.
    pub untagged: usize,
    /// Rows dropped because their files are gone from disk this pass.
    pub removed: usize,
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
    // The walk is the ground truth for what lives under the root this pass:
    // an unreadable file (permissions, transient IO) still lands here from
    // its parent's directory entry, so it never counts as gone. Built before
    // the batch loop consumes `files`, keyed the same way process_file keys a
    // stored row so the two sets compare byte for byte.
    let present: std::collections::HashSet<String> =
        files.iter().map(|p| p.to_string_lossy().into_owned()).collect();

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

    // Diff the stored rows under root against what the walk found and drop
    // the rows whose files are gone. Skipped on two counts, both to keep a
    // bad pass from wiping the library: an aborted scan never finished the
    // walk, and a root that will not even list its entries (unplugged drive,
    // dropped network mount) reads as empty when the files are really still
    // there. A genuinely emptied but readable root still prunes.
    if !summary.aborted && std::fs::read_dir(root).is_ok() {
        summary.removed = store::prune_missing(conn, root, &present)?;
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

/// Read one file that need not live in any scanned root - a drag-drop, a
/// file association, a CLI open. Stats and reads it the same way the scan
/// does, so the row carries real title/artist/album. None only when the
/// file cannot be stat'd; a file with no readable tags still returns a
/// fallback row (filename as title), matching how the scan degrades.
pub fn read_one(path: &Path) -> Option<TrackRow> {
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let row = read_tags(path).unwrap_or_else(|| fallback_row(path));
    Some(TrackRow {
        path: path.to_string_lossy().into_owned(),
        size,
        mtime,
        ..row
    })
}

/// Every audio file under `root`, recursively, the same walk a scan runs
/// but without touching the store: a maintenance pass (the tag repair
/// window) needs the on-disk paths to inspect, indexed or not. Blocking IO;
/// run it off the UI thread.
pub fn audio_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(root, &mut out);
    out
}

/// Whether a path carries one of the audio extensions the scan indexes, the
/// one filter that decides what becomes a track. Same test the walk runs, so
/// a watched change and a full scan agree on what counts.
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    // Seed with the root's real path so a link back up to it stops the walk too.
    if let Ok(canon) = std::fs::canonicalize(dir) {
        seen.insert(canon);
    }
    collect_into(dir, out, &mut seen);
}

fn collect_into(dir: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
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
            // Guard against symlink loops. A linked folder whose real path was
            // already walked is a cycle, so skip it; without this a symlink
            // pointing back up the tree hangs the scan. Canonicalize collapses
            // the two routes to one entry. A path that won't canonicalize (a
            // mutual link loop errors here, so does one that vanished mid-walk)
            // still gets walked, and read_dir short-circuits the loop for it.
            if let Ok(canon) = std::fs::canonicalize(&path) {
                // Already walked this real path: a cycle, skip it.
                if !seen.insert(canon) {
                    continue;
                }
            }
            collect_into(&path, out, seen);
        } else if is_audio(&path) {
            out.push(path);
        }
    }
}

/// Tag read isolated per file: a malformed file that errors or panics
/// lofty's parser costs that one file its tags, never the scan.
///
/// MPEG and FLAC parse to their native file type first, so the rating (in
/// TXXX/POPM frames and unmapped Vorbis keys the generic tag drops) reads
/// off the same parse that fills the row - one open per file, not two. The
/// native file converts to a `TaggedFile` exactly as `Probe::read` does, so
/// the generic tags below match the old probe path byte for byte. Any other
/// format keeps the plain probe; those carry no rating rox reads anyway.
fn read_tags(path: &Path) -> Option<TrackRow> {
    let source = crate::tag_source::open(path).ok()?;
    let (file, rating) = catch_unwind(AssertUnwindSafe(move || {
        let probe = lofty::probe::Probe::new(source)
            .guess_file_type()
            .ok()?
            .options(crate::parse_opts());
        let opts = crate::parse_opts();
        // guess_file_type restores the reader to where it started, so the
        // native read_from below sees the same stream Probe::read would.
        match probe.file_type() {
            Some(lofty::file::FileType::Mpeg) => {
                let mut reader = probe.into_inner();
                let mpeg = MpegFile::read_from(&mut reader, opts).ok()?;
                let rating = mpeg.id3v2().and_then(crate::rating::from_id3v2);
                Some((TaggedFile::from(mpeg), rating))
            }
            Some(lofty::file::FileType::Flac) => {
                let mut reader = probe.into_inner();
                let flac = FlacFile::read_from(&mut reader, opts).ok()?;
                let rating = flac.vorbis_comments().and_then(crate::rating::from_vorbis);
                Some((TaggedFile::from(flac), rating))
            }
            _ => probe.read().ok().map(|f| (f, None)),
        }
    }))
    .ok()??;
    let mut row = fallback_row(path);
    row.duration_ms = file.properties().duration().as_millis() as u32;
    // The parsed type beats the extension a fallback row guesses from; a
    // format outside the match keeps the guess.
    if let Some(codec) = match file.file_type() {
        lofty::file::FileType::Flac => Some("flac"),
        lofty::file::FileType::Mpeg => Some("mp3"),
        lofty::file::FileType::Wav => Some("wav"),
        lofty::file::FileType::Vorbis => Some("vorbis"),
        lofty::file::FileType::Aiff => Some("aiff"),
        lofty::file::FileType::Aac => Some("aac"),
        // Mp4 (m4a/m4b) carries AAC or ALAC and lofty does not split them, so
        // it keeps the extension guess rather than mislabel one as the other.
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
    // The rating read off the same native parse above - FMPS lives in TXXX
    // frames and unmapped Vorbis keys, which this generic tag never carries.
    row.rating = rating.unwrap_or(0);
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

    /// The combined read_tags path and the standalone rating::read agree on
    /// a file's rating: the scanner now pulls the rating out of the same
    /// parse it reads the tags from, so the two must not drift. Half points
    /// survive both ways.
    #[test]
    fn rating_matches_across_read_paths() {
        let dir = std::env::temp_dir().join("rox-scanner-rating-parity");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut audio = Vec::new();
        for frame in 0..3u32 {
            audio.extend([0xFF, 0xFB, 0x90, 0x00]);
            audio.extend((0..413u32).map(|i| ((frame * 413 + i) * 7 % 251) as u8));
        }
        let path = dir.join("track.mp3");
        std::fs::write(&path, &audio).unwrap();
        writer::commit(
            &path,
            &[Change {
                field: Field::Rating,
                value: Some("7.5".into()),
            }],
        )
        .unwrap();

        // read_one runs read_tags, the combined parse; rating::read_path is
        // the standalone reader. Both must land on the same half-point value.
        let combined = read_one(&path).unwrap().rating;
        let standalone = crate::rating::read_path(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(combined, 75);
        assert_eq!(standalone, 75);
        assert_eq!(combined, standalone);
    }

    /// A rescan drops the rows for files deleted from disk, keeps the ones
    /// still there, and never prunes when the root itself cannot be listed.
    #[test]
    fn rescan_prunes_deleted_files() {
        let dir = std::env::temp_dir().join("rox-scanner-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        // Dummy bytes: the tags will not read, so each indexes under its
        // filename. That is enough to exercise the walk-versus-store diff.
        let files = ["a/1.mp3", "a/2.mp3", "b/1.mp3"];
        for name in files {
            std::fs::write(dir.join(name), b"not audio").unwrap();
        }

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        let scan = |conn: &mut Connection| scan(conn, &dir, |_, _, _| true).unwrap();

        let s = scan(&mut conn);
        assert_eq!(s.indexed, 3);
        assert_eq!(s.removed, 0);
        assert_eq!(store::count(&conn).unwrap(), 3);

        // Delete one file, rescan: its row goes, the survivors stay.
        std::fs::remove_file(dir.join("a/2.mp3")).unwrap();
        let s = scan(&mut conn);
        assert_eq!(s.removed, 1);
        assert_eq!(store::count(&conn).unwrap(), 2);
        assert!(store::id_for_path(&conn, dir.join("a/2.mp3").to_str().unwrap())
            .unwrap()
            .is_none());
        assert!(store::id_for_path(&conn, dir.join("a/1.mp3").to_str().unwrap())
            .unwrap()
            .is_some());

        // The whole root gone (unplugged drive, dropped mount): the walk
        // reads empty, but the guard keeps the rows rather than wipe them.
        std::fs::remove_dir_all(&dir).unwrap();
        let s = scan(&mut conn);
        assert_eq!(s.removed, 0);
        assert_eq!(store::count(&conn).unwrap(), 2);
    }

    /// read_one on a loose file returns a row with path/size/mtime filled,
    /// even when the file carries no readable tags - the filename stands in
    /// as the title.
    #[test]
    fn read_one_fills_path_on_loose_file() {
        let dir = std::env::temp_dir().join("rox-scanner-read-one");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("loose track.mp3");
        // Not a valid stream, so the tag read fails and we fall through to
        // the filename-title fallback.
        std::fs::write(&path, b"not audio").unwrap();

        let row = read_one(&path).unwrap();
        assert_eq!(row.path, path.to_string_lossy());
        assert_eq!(row.title, "loose track");
        assert_eq!(row.codec, "mp3");
        assert_eq!(row.size, 9);
        assert!(row.mtime > 0);

        // A path that does not exist cannot be stat'd, so None.
        assert!(read_one(&dir.join("missing.mp3")).is_none());
    }
}
