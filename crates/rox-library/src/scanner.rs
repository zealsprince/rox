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
    "flac", "mp3", "wav", "ogg", "oga", "m4a", "m4b", "aac", "aif", "aiff", "aifc", "mka", "caf",
];

/// Cue sheets are deliberately not in [`EXTENSIONS`]: a sheet is not audio,
/// and an external open handed one must not try to play it. The walk notices
/// them separately, and only to split the image files they point at.
pub const CUE_EXTENSION: &str = "cue";
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
    let mut known = store::local_files(conn)?;
    // Which images the store currently holds as cue tracks, so this pass can
    // tell what a sheet used to produce from what it produces now.
    let stored_cues = store::cue_subs(conn)?;
    let mut walk = Walk::default();
    collect(root, &mut walk);
    walk.audio.sort();
    // The walk is the ground truth for what lives under the root this pass:
    // an unreadable file (permissions, transient IO) still lands here from
    // its parent's directory entry, so it never counts as gone. Built before
    // the batch loop consumes the list, keyed the same way process_file keys
    // a stored row so the two sets compare byte for byte. Claimed images are
    // in here too: the sheet changes what rows they get, not whether the file
    // is there.
    let present: std::collections::HashSet<String> = walk
        .audio
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let claimed = claims(&walk.cues, &walk.audio);
    // An image a sheet no longer claims has to be re-read even if its own
    // (mtime, size) never moved: its cue rows are about to go, and dropping
    // them without emitting the plain row would lose the file. Forgetting it
    // here is what makes process_file read it again.
    for path in stored_cues.keys() {
        if !claimed.contains_key(Path::new(path)) {
            known.remove(path);
        }
    }
    // Claimed images get their rows from the cue pass below, never a plain
    // row of their own.
    let files: Vec<PathBuf> = walk
        .audio
        .iter()
        .filter(|path| !claimed.contains_key(*path))
        .cloned()
        .collect();
    let total = files.len() + claimed.len();

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

    // The cue pass. One image at a time rather than in file-sized batches:
    // there are as many claimed images as there are ripped discs, and each
    // one is a single probe that fans out into a handful of rows.
    if !summary.aborted {
        let images: Vec<&PathBuf> = claimed.keys().collect();
        for chunk in images.chunks(BATCH) {
            let outcomes: Vec<(&PathBuf, CueOutcome)> = chunk
                .par_iter()
                .map(|image| {
                    let outcome = process_cue(image, &claimed[*image], &known, &stored_cues);
                    let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                    if !progress(done, total, image) {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                    (*image, outcome)
                })
                .collect();

            let mut batch: Vec<TrackRow> = Vec::new();
            for (image, outcome) in outcomes {
                match outcome {
                    CueOutcome::Missing => {}
                    CueOutcome::Unchanged(tracks) => summary.unchanged += tracks,
                    CueOutcome::Indexed(rows) => {
                        summary.indexed += rows.len();
                        batch.extend(rows);
                    }
                }
                // The sheet's word on which subsongs the image has, run
                // whatever the outcome: it retires the plain sub 0 row an
                // image carried before a sheet claimed it, and the cue rows
                // for tracks a re-edited sheet dropped.
                let keep: Vec<u16> = claimed[image].tracks.iter().map(|t| t.number).collect();
                store::retain_subs(conn, &image.to_string_lossy(), &keep)?;
            }
            if !batch.is_empty() {
                store::insert_batch(conn, &batch)?;
            }
            if cancelled.load(Ordering::Relaxed) {
                summary.aborted = true;
                break;
            }
        }
    }

    // The other direction: an image whose sheet is gone keeps only the plain
    // row the walk just re-emitted for it, so its cue rows go here.
    if !summary.aborted {
        for path in stored_cues.keys() {
            if !claimed.contains_key(Path::new(path)) {
                store::retain_subs(conn, path, &[0])?;
            }
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
    // With the store squared against disk, match playlist members and listens
    // whose track id died with an earlier prune back to files this pass
    // brought back. One indexed sweep each; a pass with nothing dangling
    // costs next to nothing.
    crate::playlists::reattach(conn)?;
    crate::listens::reattach(conn)?;
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
    // A cue sheet in the list is not a file to index, it's an instruction to
    // re-cut the images it names. Sheets sitting beside a named audio file
    // count too: without them a touched image would land as one plain row and
    // wipe the cue rows it should have kept.
    let mut dirs: Vec<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
    dirs.sort();
    dirs.dedup();
    let mut cues: Vec<PathBuf> = paths.iter().filter(|p| is_cue(p)).cloned().collect();
    let mut nearby: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_cue(&path) {
                cues.push(path);
            } else if is_audio(&path) {
                nearby.push(path);
            }
        }
    }
    cues.sort();
    cues.dedup();
    let claimed = claims(&cues, &nearby);

    // Everything to re-read: the audio files named, plus every image a named
    // sheet claims. Claimed images go through the cue path, the rest plain.
    let mut targets: Vec<PathBuf> = paths.iter().filter(|p| is_audio(p)).cloned().collect();
    for image in claimed.keys() {
        targets.push(image.clone());
    }
    targets.sort();
    targets.dedup();

    let rows: Vec<TrackRow> = targets
        .par_iter()
        .flat_map(|path| match claimed.get(path) {
            // An empty stored-subs map forces the read: the caller only names
            // files it just changed, so nothing here is unchanged by
            // definition.
            Some(claim) => match process_cue(path, claim, &known, &HashMap::new()) {
                CueOutcome::Indexed(rows) => rows,
                _ => Vec::new(),
            },
            None => match process_file(path, &known) {
                Outcome::Indexed { row, .. } => vec![*row],
                _ => Vec::new(),
            },
        })
        .collect();
    if !rows.is_empty() {
        store::insert_batch(conn, &rows)?;
        // Square the stored subsongs with what the sheets say now, the same
        // bookkeeping a full scan runs and in the same order: rows land
        // first, then the ones they replaced go. A claimed image loses the
        // plain row it used to have, an image whose sheet went away loses its
        // cue rows.
        for (image, claim) in &claimed {
            let keep: Vec<u16> = claim.tracks.iter().map(|t| t.number).collect();
            store::retain_subs(conn, &image.to_string_lossy(), &keep)?;
        }
        for path in targets.iter().filter(|p| !claimed.contains_key(*p)) {
            store::retain_subs(conn, &path.to_string_lossy(), &[0])?;
        }
        // A watched file coming back lands here as a fresh row; give any
        // playlist members and listens still pointing at its old id the
        // same reattach a full scan runs.
        crate::playlists::reattach(conn)?;
        crate::listens::reattach(conn)?;
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
    let mut walk = Walk::default();
    collect(root, &mut walk);
    walk.audio
}

/// Whether a path carries one of the audio extensions the scan indexes, the
/// one filter that decides what becomes a track. Same test the walk runs, so
/// a watched change and a full scan agree on what counts.
pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether a path is a cue sheet. Deliberately not part of [`is_audio`]: a
/// sheet is never playable and never a row, it only decides how the image
/// beside it is cut up.
pub fn is_cue(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(CUE_EXTENSION))
}

/// Whether a changed path is worth handing to [`reindex`]: audio, or a cue
/// sheet whose edit re-cuts the image it points at. What a watcher's
/// relevance filter should ask, since filtering on [`is_audio`] alone drops
/// every sheet edit on the floor.
pub fn is_relevant(path: &Path) -> bool {
    is_audio(path) || is_cue(path)
}

/// What one walk found: the audio files that become rows, and the cue sheets
/// that decide how some of them are cut. Kept apart because a sheet is not a
/// track and must never be indexed as one.
#[derive(Default)]
struct Walk {
    audio: Vec<PathBuf>,
    cues: Vec<PathBuf>,
}

fn collect(dir: &Path, out: &mut Walk) {
    let mut seen = HashSet::new();
    // Seed with the root's real path so a link back up to it stops the walk too.
    if let Ok(canon) = std::fs::canonicalize(dir) {
        seen.insert(canon);
    }
    collect_into(dir, out, &mut seen);
}

fn collect_into(dir: &Path, out: &mut Walk, seen: &mut HashSet<PathBuf>) {
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
            out.audio.push(path);
        } else if is_cue(&path) {
            out.cues.push(path);
        }
    }
}

/// One cue sheet's claim on one image file: which sheet made it, when that
/// sheet was last written, the album-level tags off it, and the tracks it
/// cuts the image into.
struct Claim {
    cue_path: PathBuf,
    /// The sheet's own mtime, folded into the row's so an edit to the sheet
    /// reindexes the image even though the image itself never changed.
    cue_mtime: i64,
    album: String,
    album_artist: String,
    genre: String,
    year: u16,
    tracks: Vec<crate::cue::CueTrack>,
}

/// Read the cue sheets a walk found and work out which images they claim,
/// keyed by the resolved image path. A sheet that will not parse, or whose
/// FILE lines point at nothing indexable, claims nothing and leaves those
/// files to be indexed plain. Two sheets naming one image is a broken
/// library either way; the last one read wins.
fn claims(cues: &[PathBuf], audio: &[PathBuf]) -> HashMap<PathBuf, Claim> {
    if cues.is_empty() {
        return HashMap::new();
    }
    // The walked audio grouped by directory, so resolving a FILE line is a
    // lookup rather than a read_dir per sheet or a sweep of the whole walk.
    let mut by_dir: HashMap<&Path, Vec<&Path>> = HashMap::new();
    for path in audio {
        if let Some(dir) = path.parent() {
            by_dir.entry(dir).or_default().push(path.as_path());
        }
    }
    let mut out: HashMap<PathBuf, Claim> = HashMap::new();
    for cue_path in cues {
        let Some(dir) = cue_path.parent() else {
            continue;
        };
        let Ok(bytes) = std::fs::read(cue_path) else {
            continue;
        };
        let Some(sheet) = crate::cue::parse(&bytes) else {
            continue;
        };
        let cue_mtime = std::fs::metadata(cue_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for file in sheet.files {
            let Some(image) = resolve_image(dir, &file.path, &by_dir) else {
                continue;
            };
            out.insert(
                image,
                Claim {
                    cue_path: cue_path.clone(),
                    cue_mtime,
                    album: sheet.title.clone(),
                    album_artist: sheet.performer.clone(),
                    genre: sheet.genre.clone(),
                    year: sheet.year,
                    tracks: file.tracks,
                },
            );
        }
    }
    out
}

/// Turn a sheet's FILE argument into a path the scan actually found. Three
/// tries, loosening as they go: the name as written, the same name in any
/// casing (a sheet from Windows beside files on a case-sensitive disk), then
/// the same stem under any audio extension, because rippers habitually write
/// `.wav` in the sheet and leave a `.flac` on disk. Only files the walk
/// already indexed can match, so a sheet pointing at a `.bin` image nothing
/// here decodes resolves to nothing and its tracks are skipped.
fn resolve_image(
    cue_dir: &Path,
    arg: &str,
    by_dir: &HashMap<&Path, Vec<&Path>>,
) -> Option<PathBuf> {
    // Sheets written on Windows use backslashes even for a bare name, and a
    // multi-disc sheet may reach into a subdirectory.
    let arg = arg.replace('\\', "/");
    let rel = Path::new(&arg);
    let name = rel.file_name()?.to_str()?;
    let dir = match rel.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => cue_dir.join(parent),
        _ => cue_dir.to_path_buf(),
    };
    let siblings = by_dir.get(dir.as_path())?;
    fn named<'a>(path: &&'a Path) -> Option<&'a str> {
        path.file_name().and_then(|n| n.to_str())
    }
    if let Some(hit) = siblings.iter().find(|p| named(p) == Some(name)) {
        return Some(hit.to_path_buf());
    }
    if let Some(hit) = siblings
        .iter()
        .find(|p| named(p).is_some_and(|n| n.eq_ignore_ascii_case(name)))
    {
        return Some(hit.to_path_buf());
    }
    let stem = Path::new(name).file_stem()?.to_str()?;
    siblings
        .iter()
        .find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(stem))
        })
        .map(|hit| hit.to_path_buf())
}

/// What one claimed image's pass produced.
enum CueOutcome {
    /// The image vanished or would not stat between the walk and the read.
    Missing,
    /// The image and its sheet are both unchanged and the store already
    /// holds exactly these subsongs, so nothing was read. Carries how many
    /// tracks were skipped, for the summary.
    Unchanged(usize),
    /// The rows to upsert, one per track of the sheet.
    Indexed(Vec<TrackRow>),
}

/// Stat a claimed image and, if anything moved, probe it once and cut it into
/// its cue tracks. The unchanged check keys off the combined mtime and the
/// subsongs already stored, so editing the sheet alone still re-emits.
fn process_cue(
    image: &Path,
    claim: &Claim,
    known: &HashMap<String, (i64, u64)>,
    stored_cues: &HashMap<String, Vec<u16>>,
) -> CueOutcome {
    let Ok(meta) = std::fs::metadata(image) else {
        return CueOutcome::Missing;
    };
    let size = meta.len();
    let image_mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // A cue row's mtime is the later of the two files it comes from, so a
    // touched sheet counts as a changed track even when the image is
    // untouched, which is the usual way a cue rip gets corrected.
    let mtime = image_mtime.max(claim.cue_mtime);
    let path_str = image.to_string_lossy().into_owned();
    let subs: Vec<u16> = claim.tracks.iter().map(|t| t.number).collect();
    if known.get(&path_str) == Some(&(mtime, size)) && stored_cues.get(&path_str) == Some(&subs) {
        return CueOutcome::Unchanged(subs.len());
    }

    // One probe for the whole disc: codec, stream numbers, and the tags the
    // sheet leaves out all come off the image, and reading it per track would
    // reopen the same gigabyte a dozen times.
    let image_tags = read_tags(image).unwrap_or_else(|| fallback_row(image));
    CueOutcome::Indexed(cue_rows(image, claim, &image_tags, size, mtime))
}

/// Cut one image's tags into a row per cue track. The sheet wins on the
/// album-level fields where it says anything, since it was written about
/// this disc; the image's own tags fill the gaps.
fn cue_rows(
    image: &Path,
    claim: &Claim,
    image_tags: &TrackRow,
    size: u64,
    mtime: i64,
) -> Vec<TrackRow> {
    let path = image.to_string_lossy().into_owned();
    let cue_path = claim.cue_path.to_string_lossy().into_owned();
    claim
        .tracks
        .iter()
        .map(|track| {
            // The last track of an image has no end, so it runs from its
            // start to whatever the file's own length turned out to be. A
            // start past the end (a sheet that outlived its image) floors at
            // zero rather than wrap.
            let duration_ms = track
                .span
                .len_ms()
                .unwrap_or_else(|| image_tags.duration_ms.saturating_sub(track.span.start_ms));
            let artist = track.performer.clone();
            TrackRow {
                path: path.clone(),
                sub: track.number,
                // A sheet that named no title leaves the row the same
                // filename fallback an unreadable file would get.
                title: if track.title.is_empty() {
                    image_tags.title.clone()
                } else {
                    track.title.clone()
                },
                album_artist: if claim.album_artist.is_empty() {
                    artist.clone()
                } else {
                    claim.album_artist.clone()
                },
                artist,
                album: if claim.album.is_empty() {
                    image_tags.album.clone()
                } else {
                    claim.album.clone()
                },
                genre: if claim.genre.is_empty() {
                    image_tags.genre.clone()
                } else {
                    claim.genre.clone()
                },
                year: if claim.year == 0 {
                    image_tags.year
                } else {
                    claim.year
                },
                // The sheet knows nothing about discs, so a multi-disc rip
                // gets its disc number off the image's own tag.
                disc_no: image_tags.disc_no,
                track_no: track.number,
                duration_ms,
                codec: image_tags.codec.clone(),
                bitrate_kbps: image_tags.bitrate_kbps,
                sample_rate_hz: image_tags.sample_rate_hz,
                bit_depth: image_tags.bit_depth,
                rating: 0,
                // Only the album figures. A whole-disc rip's track gain
                // describes the whole image, so handing it to each track
                // would level every one of them by the disc's average; the
                // album pair means exactly what it says either way.
                replay_gain: crate::replaygain::ReplayGain {
                    track_db: None,
                    track_peak: None,
                    album_db: image_tags.replay_gain.album_db,
                    album_peak: image_tags.replay_gain.album_peak,
                },
                // No tempo, even where the image's tag carries one: that
                // number describes a whole disc, and handing it to every
                // track would claim they all run at it. Each subsong lands
                // on the analysis pass's list instead, which takes them.
                bpm: None,
                cue: Some(crate::CueSlice {
                    cue_path: cue_path.clone(),
                    span: track.span,
                }),
                size,
                mtime,
            }
        })
        .collect()
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
    // lofty takes an MP4's length off the sample tables, which a fragmented
    // file leaves empty, so a whole album of them scans in at zero. The
    // movie header still states it (see [`crate::mp4`]), and this only
    // opens the file again for the rows that came back with nothing.
    if row.duration_ms == 0 {
        if let Some(secs) = crate::mp4::fragment_duration_secs(path) {
            row.duration_ms = (secs * 1000.0).round() as u32;
        }
    }
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
    row.sample_rate_hz = file.properties().sample_rate().unwrap_or(0);
    // Lossy formats report no depth, which is right: there are no bits per
    // sample to name once the stream is coefficients. Zero reads as blank.
    row.bit_depth = file.properties().bit_depth().unwrap_or(0);
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
        // Every genre item, joined to the "; " list form: Vorbis carries
        // multiples as repeated GENRE comments, ID3v2.4 as one
        // null-separated TCON, and lofty hands both over as separate items.
        row.genre = crate::genre::join(tag.get_strings(lofty::tag::ItemKey::Genre));
        row.year = tag.date().map(|d| d.year).unwrap_or(0);
        row.disc_no = tag.disk().unwrap_or(0) as u16;
        row.track_no = tag.track().unwrap_or(0) as u16;
        // What the file says it runs at, off the primary tag like everything
        // else here. A file carrying nothing believable lands on the
        // analysis pass's list instead (see [`crate::tempo`]).
        row.bpm = crate::tempo::read(tag);
    }
    // What an analysis pass measured, whoever ran it. Read across every tag
    // on the file rather than off the primary one: mp3gain writes its four
    // values into an APEv2 tag that sits beside an ID3v2 tag carrying none,
    // and lofty calls ID3v2 the primary on MPEG, so a primary-only read
    // misses them.
    row.replay_gain = replay_gain_across_tags(&file);
    // The rating read off the same native parse above - FMPS lives in TXXX
    // frames and unmapped Vorbis keys, which this generic tag never carries.
    row.rating = rating.unwrap_or(0);
    Some(row)
}

/// ReplayGain gathered from every tag the file carries, primary first, the
/// rest in the order lofty parsed them. First tag holding a given key wins,
/// per key: a file can carry the track pair in one tag and the album pair in
/// another, and taking whichever tag answers first per field beats picking
/// one tag and ignoring the other three values.
///
/// Only ReplayGain reads this wide. Everything else on the row comes off the
/// primary tag, where a second tag disagreeing about the title is a conflict
/// to resolve, not a gap to fill.
fn replay_gain_across_tags(file: &TaggedFile) -> crate::replaygain::ReplayGain {
    let mut rg = crate::replaygain::ReplayGain::default();
    let primary = file.primary_tag();
    for tag in primary.into_iter().chain(file.tags()) {
        if rg.track_db.is_some()
            && rg.track_peak.is_some()
            && rg.album_db.is_some()
            && rg.album_peak.is_some()
        {
            break;
        }
        let found = crate::replaygain::read(tag);
        rg.track_db = rg.track_db.or(found.track_db);
        rg.track_peak = rg.track_peak.or(found.track_peak);
        rg.album_db = rg.album_db.or(found.album_db);
        rg.album_peak = rg.album_peak.or(found.album_peak);
    }
    rg
}

/// The row a file gets when its tags cannot be read: filename as title,
/// the extension as codec, everything else empty. path/size/mtime are
/// filled in by the caller.
fn fallback_row(path: &Path) -> TrackRow {
    TrackRow {
        path: String::new(),
        sub: 0,
        cue: None,
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
        sample_rate_hz: 0,
        bit_depth: 0,
        rating: 0,
        replay_gain: crate::replaygain::ReplayGain::default(),
        bpm: None,
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

    /// A file carrying multiple genre values - the writer lays them down
    /// as native multiples - scans as the one "; " list, not just the
    /// first value.
    #[test]
    fn multi_genre_scans_joined() {
        let dir = std::env::temp_dir().join("rox-scanner-multi-genre");
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
                field: Field::Genre,
                value: Some("Electronic; Ambient".into()),
            }],
        )
        .unwrap();

        let row = read_one(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(row.genre, "Electronic; Ambient");
    }

    /// The ReplayGain a tagger left in the file comes off the same parse
    /// the rest of the row does, TXXX frames and all.
    #[test]
    fn replaygain_scans_off_the_tags() {
        use lofty::config::WriteOptions;
        use lofty::prelude::TagExt;
        use lofty::tag::{ItemKey, Tag, TagType};

        let dir = std::env::temp_dir().join("rox-scanner-replaygain");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut audio = Vec::new();
        for frame in 0..3u32 {
            audio.extend([0xFF, 0xFB, 0x90, 0x00]);
            audio.extend((0..413u32).map(|i| ((frame * 413 + i) * 7 % 251) as u8));
        }
        let path = dir.join("track.mp3");
        std::fs::write(&path, &audio).unwrap();

        // Written the way an analysis tool writes them, as TXXX frames.
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-7.35 dB".into());
        tag.insert_text(ItemKey::ReplayGainTrackPeak, "0.987654".into());
        tag.insert_text(ItemKey::ReplayGainAlbumGain, "-8.10 dB".into());
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        let row = read_one(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(row.replay_gain.track_db, Some(-7.35));
        assert_eq!(row.replay_gain.track_peak, Some(0.987654));
        assert_eq!(row.replay_gain.album_db, Some(-8.10));
        // Nothing invented for the value the file doesn't carry.
        assert_eq!(row.replay_gain.album_peak, None);
    }

    /// mp3gain's habit: the numbers land in an APEv2 tag while the ID3v2 tag
    /// beside it carries only the usual fields. The scan has to read both or
    /// every mp3gain'd file scans as unlevelled.
    #[test]
    fn replaygain_reads_out_of_an_ape_tag_beside_id3v2() {
        use lofty::config::WriteOptions;
        use lofty::prelude::TagExt;
        use lofty::tag::{ItemKey, Tag, TagType};

        let dir = std::env::temp_dir().join("rox-scanner-replaygain-ape");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut audio = Vec::new();
        for frame in 0..3u32 {
            audio.extend([0xFF, 0xFB, 0x90, 0x00]);
            audio.extend((0..413u32).map(|i| ((frame * 413 + i) * 7 % 251) as u8));
        }
        let path = dir.join("track.mp3");
        std::fs::write(&path, &audio).unwrap();

        // The primary tag on an MPEG, and it knows nothing about levels.
        let mut id3 = Tag::new(TagType::Id3v2);
        id3.insert_text(ItemKey::TrackTitle, "Levelled".into());
        id3.save_to_path(&path, WriteOptions::default()).unwrap();

        let mut ape = Tag::new(TagType::Ape);
        ape.insert_text(ItemKey::ReplayGainTrackGain, "-4.20 dB".into());
        ape.insert_text(ItemKey::ReplayGainTrackPeak, "0.912".into());
        ape.save_to_path(&path, WriteOptions::default()).unwrap();

        let row = read_one(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(row.title, "Levelled", "the primary tag still fills the row");
        assert_eq!(row.replay_gain.track_db, Some(-4.20));
        assert_eq!(row.replay_gain.track_peak, Some(0.912));
        assert_eq!(row.replay_gain.album_db, None);
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
        assert!(
            store::id_for_path(&conn, dir.join("a/2.mp3").to_str().unwrap())
                .unwrap()
                .is_none()
        );
        assert!(
            store::id_for_path(&conn, dir.join("a/1.mp3").to_str().unwrap())
                .unwrap()
                .is_some()
        );

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

    /// The stream's sample rate and bit depth come off the parsed
    /// properties, not the tags, so a file with no tag at all still
    /// reports what it is. A hand-built PCM wav is the cheapest real
    /// stream to assert against.
    #[test]
    fn read_one_reads_sample_rate_and_depth() {
        let dir = std::env::temp_dir().join("rox-scanner-properties");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        std::fs::write(&path, pcm_wav(44100, 16, 2, 1)).unwrap();

        let row = read_one(&path).unwrap();
        assert_eq!(row.codec, "wav");
        assert_eq!(row.sample_rate_hz, 44100);
        assert_eq!(row.bit_depth, 16);
    }

    /// A twelve second image and a sheet cutting it into three, the shape a
    /// whole-disc rip takes. Three seconds each for the first two tracks, the
    /// rest of the file for the last.
    const SHEET: &str = r#"REM GENRE "Post Rock"
REM DATE 2003
PERFORMER "The Band"
TITLE "The Album"
FILE "disc.wav" WAVE
  TRACK 01 AUDIO
    TITLE "One"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Two"
    PERFORMER "Guest"
    INDEX 01 00:03:00
  TRACK 03 AUDIO
    TITLE "Three"
    INDEX 01 00:06:00
"#;

    /// A fresh fixture directory holding the image, and a store over it.
    fn cue_fixture(name: &str) -> (PathBuf, Connection) {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("disc.wav"), pcm_wav(44100, 16, 2, 12)).unwrap();
        let conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        (dir, conn)
    }

    /// Every row the store holds for one image, in subsong order: the columns
    /// a cue test wants to read back.
    #[allow(clippy::type_complexity)]
    fn subsongs(conn: &Connection, image: &Path) -> Vec<(u16, String, String, u32)> {
        let mut stmt = conn
            .prepare(
                "SELECT sub, title, artist, duration_ms FROM tracks
                 WHERE path = ?1 ORDER BY sub",
            )
            .unwrap();
        let rows = stmt
            .query_map([image.to_str().unwrap()], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u16,
                    r.get(1)?,
                    r.get(2)?,
                    r.get::<_, i64>(3)? as u32,
                ))
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    /// Push a file's mtime forward. Second-granularity timestamps mean a test
    /// that rewrites a file inside the same second would otherwise look
    /// untouched, and the whole point of the combined mtime is that it moves.
    fn touch_ahead(path: &Path) {
        let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(ahead)
            .unwrap();
    }

    /// The core of it: a sheet beside an image turns one file into a row per
    /// track, the image gets no plain row of its own, and every row carries
    /// the sheet's tags over the image's own.
    #[test]
    fn a_cue_sheet_splits_its_image_into_tracks() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-split");
        let image = dir.join("disc.wav");
        std::fs::write(dir.join("disc.cue"), SHEET).unwrap();

        let summary = scan(&mut conn, &dir, |_, _, _| true).unwrap();
        assert_eq!(summary.indexed, 3, "one row per cue track");
        assert_eq!(store::count(&conn).unwrap(), 3, "and no plain image row");

        let rows = subsongs(&conn, &image);
        assert_eq!(
            rows,
            [
                (1, "One".to_string(), "The Band".to_string(), 3_000),
                // A track that named its own performer keeps it.
                (2, "Two".to_string(), "Guest".to_string(), 3_000),
                // The last track runs from its start to the file's end.
                (3, "Three".to_string(), "The Band".to_string(), 6_000),
            ]
        );

        // The album-level tags come off the sheet, the stream numbers off the
        // one probe of the image.
        let (album, album_artist, genre, year, codec, rate, depth): (
            String,
            String,
            String,
            u16,
            String,
            u32,
            u8,
        ) = conn
            .query_row(
                "SELECT album, album_artist, genre, year, codec, sample_rate, bit_depth
                 FROM tracks WHERE sub = 2",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(album, "The Album");
        assert_eq!(album_artist, "The Band");
        assert_eq!(genre, "Post Rock");
        assert_eq!(year, 2003);
        assert_eq!((codec, rate, depth), ("wav".to_string(), 44100, 16));

        // The spans landed beside the rows, the last one open-ended.
        let path = image.to_str().unwrap();
        assert_eq!(
            store::queue_meta_for_key(&conn, path, 2).unwrap().span,
            Some(crate::cue::Span {
                start_ms: 3_000,
                end_ms: Some(6_000)
            })
        );
        assert_eq!(
            store::queue_meta_for_key(&conn, path, 3).unwrap().span,
            Some(crate::cue::Span {
                start_ms: 6_000,
                end_ms: None
            })
        );

        // A second pass with nothing touched reads no tags at all.
        let summary = scan(&mut conn, &dir, |_, _, _| true).unwrap();
        assert_eq!((summary.indexed, summary.unchanged), (0, 3));
        assert_eq!(summary.removed, 0, "a claimed image is not a missing file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A sheet naming a file that isn't there resolves by stem instead, since
    /// rippers write `.wav` in the sheet and leave a lossless file on disk.
    #[test]
    fn a_cue_sheet_resolves_its_image_by_stem() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-stem");
        // The sheet says .aiff, the disk holds disc.wav.
        std::fs::write(dir.join("disc.cue"), SHEET.replace("disc.wav", "DISC.aiff")).unwrap();

        scan(&mut conn, &dir, |_, _, _| true).unwrap();
        assert_eq!(subsongs(&conn, &dir.join("disc.wav")).len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The sheet is deleted: the image goes back to being one plain file,
    /// its cue rows and their spans go, and nothing is left dangling.
    #[test]
    fn a_deleted_cue_sheet_gives_the_image_back_whole() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-deleted");
        let image = dir.join("disc.wav");
        std::fs::write(dir.join("disc.cue"), SHEET).unwrap();
        scan(&mut conn, &dir, |_, _, _| true).unwrap();
        assert_eq!(store::count(&conn).unwrap(), 3);

        std::fs::remove_file(dir.join("disc.cue")).unwrap();
        scan(&mut conn, &dir, |_, _, _| true).unwrap();

        assert_eq!(
            subsongs(&conn, &image),
            [(0, "disc".to_string(), String::new(), 12_000)],
            "one row for the whole image, titled off its filename"
        );
        assert!(store::cue_spans(&conn).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// And the other direction: a sheet dropped next to an already-indexed
    /// image replaces its plain row with the tracks.
    #[test]
    fn a_new_cue_sheet_replaces_the_plain_row() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-added");
        let image = dir.join("disc.wav");
        scan(&mut conn, &dir, |_, _, _| true).unwrap();
        assert_eq!(subsongs(&conn, &image).len(), 1, "one plain row to start");

        std::fs::write(dir.join("disc.cue"), SHEET).unwrap();
        scan(&mut conn, &dir, |_, _, _| true).unwrap();

        let subs: Vec<u16> = subsongs(&conn, &image).iter().map(|r| r.0).collect();
        assert_eq!(subs, [1, 2, 3], "the plain row gave way to the tracks");
        assert_eq!(store::cue_spans(&conn).unwrap().len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Editing the sheet reindexes the image even though the image itself
    /// never changed: a cue row's mtime is the later of the two files, so the
    /// unchanged check sees the edit.
    #[test]
    fn editing_a_cue_sheet_reindexes_the_image() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-edited");
        let image = dir.join("disc.wav");
        let cue = dir.join("disc.cue");
        std::fs::write(&cue, SHEET).unwrap();
        scan(&mut conn, &dir, |_, _, _| true).unwrap();

        // Same three tracks, one retitled, so nothing but the mtime can be
        // what tells the scan to look again.
        std::fs::write(&cue, SHEET.replace("\"Two\"", "\"Second\"")).unwrap();
        touch_ahead(&cue);
        let summary = scan(&mut conn, &dir, |_, _, _| true).unwrap();

        assert_eq!((summary.indexed, summary.unchanged), (3, 0));
        assert_eq!(subsongs(&conn, &image)[1].1, "Second");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A sheet that loses a track loses that row, and only that row.
    #[test]
    fn a_shortened_cue_sheet_drops_the_track_it_no_longer_lists() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-shortened");
        let image = dir.join("disc.wav");
        let cue = dir.join("disc.cue");
        std::fs::write(&cue, SHEET).unwrap();
        scan(&mut conn, &dir, |_, _, _| true).unwrap();

        let shorter = SHEET
            .split("  TRACK 03")
            .next()
            .expect("the sheet splits at its last track")
            .to_string();
        std::fs::write(&cue, shorter).unwrap();
        touch_ahead(&cue);
        scan(&mut conn, &dir, |_, _, _| true).unwrap();

        let rows = subsongs(&conn, &image);
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            rows[1].3, 9_000,
            "track two now runs to the end of the file"
        );
        assert_eq!(store::cue_spans(&conn).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The watch path: a sheet written into a folder arrives as one changed
    /// path, and reindex has to read it as an instruction to re-cut the image
    /// rather than try to index the sheet as a track.
    #[test]
    fn reindex_takes_a_cue_sheet_as_a_re_cut() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-reindex");
        let image = dir.join("disc.wav");
        scan(&mut conn, &dir, |_, _, _| true).unwrap();
        assert_eq!(subsongs(&conn, &image).len(), 1);

        let cue = dir.join("disc.cue");
        std::fs::write(&cue, SHEET).unwrap();
        assert_eq!(reindex(&mut conn, std::slice::from_ref(&cue)).unwrap(), 3);

        let subs: Vec<u16> = subsongs(&conn, &image).iter().map(|r| r.0).collect();
        assert_eq!(subs, [1, 2, 3]);
        assert_eq!(
            store::count(&conn).unwrap(),
            3,
            "the sheet itself never becomes a row"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A minimal PCM wav: the RIFF header, a fmt chunk naming the format,
    /// and `seconds` of silence, which is all lofty needs to report the
    /// properties.
    fn pcm_wav(rate: u32, bits: u16, channels: u16, seconds: u32) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let byte_rate = rate * block_align as u32;
        let data = vec![0u8; (byte_rate * seconds) as usize];
        let mut out = Vec::with_capacity(data.len() + 44);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }
}
