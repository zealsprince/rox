//! The scan pipeline: walk folders, read tags through lofty (ADR 4), and
//! upsert rows in batches. Files whose (mtime, size) match are skipped; a
//! file whose tags won't parse still indexes under its filename.
//!
//! On a big library the cost is stats, not tag reads, so the walk uses the
//! directory entry's kind and each batch stats in parallel, which hides
//! exFAT and network-mount latency.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use lofty::file::TaggedFile;
use lofty::flac::FlacFile;
use lofty::mpeg::MpegFile;
use lofty::ogg::OpusFile;
use lofty::prelude::*;
use rayon::prelude::*;
use rusqlite::Connection;

use crate::TrackRow;
use crate::exclude::Exclusions;
use crate::store;

/// What the scan indexes and an external open accepts; one list so they
/// never drift. Tracks the engine's codecs (ADR 2). Video containers stay
/// off so a scan never swallows a film library.
pub const EXTENSIONS: &[&str] = &[
    "flac", "mp3", "wav", "ogg", "oga", "opus", "m4a", "m4b", "aac", "aif", "aiff", "aifc", "mka",
    "caf",
];

/// Not in [`EXTENSIONS`]: a sheet is never played, only used to split its
/// image.
pub const CUE_EXTENSION: &str = "cue";
const BATCH: usize = 512;

#[derive(Default)]
pub struct ScanSummary {
    pub indexed: usize,
    pub unchanged: usize,
    /// Indexed by filename because the tags wouldn't read.
    pub untagged: usize,
    pub removed: usize,
    /// Stopped by `progress`. What was counted is stored; the rest never ran.
    pub aborted: bool,
}

/// Scan `root` recursively into the store. Blocking. Excluded paths fall out
/// through the prune like deleted files. `progress` gets (scanned, total,
/// path) from worker threads, out of order; returning false stops the scan
/// at the next batch boundary.
pub fn scan(
    conn: &mut Connection,
    root: &Path,
    exclude: &Exclusions,
    progress: impl Fn(usize, usize, &Path) -> bool + Sync,
) -> rusqlite::Result<ScanSummary> {
    let mut known = store::local_files(conn)?;
    let stored_cues = store::cue_subs(conn)?;
    let mut walk = Walk::default();
    collect(root, root, exclude, &mut walk);
    // Sort by the string form: the prune binary-searches this against stored
    // path strings, and PathBuf order puts "/m/a/b.mp3" before "/m/a.mp3".
    // Unstable is fine (no duplicates) and saves a scratch buffer at 10M paths.
    walk.audio
        .sort_unstable_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let claimed = claims(&walk.cues, &walk.audio);
    // An image no sheet claims any more must be re-read even if unchanged, or
    // dropping its cue rows would lose the file.
    for path in stored_cues.keys() {
        if !claimed.contains_key(Path::new(path)) {
            known.remove(path);
        }
    }
    // Borrowed, not cloned: at ten million files a second copy is a gigabyte.
    let files: Vec<&PathBuf> = walk
        .audio
        .iter()
        .filter(|path| !claimed.contains_key(*path))
        .collect();
    let total = files.len() + claimed.len();

    let mut summary = ScanSummary::default();
    let scanned = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    for chunk in files.chunks(BATCH) {
        let outcomes: Vec<Outcome> = chunk
            .par_iter()
            .map(|path| {
                let outcome = process_file(path, &known);
                let done = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                if !progress(done, total, path.as_path()) {
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

    // One image per task: each is a single probe that fans out into rows.
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
                // Always square the stored subsongs with the sheet: retire the plain sub 0
                // row and any tracks a re-edited sheet dropped.
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

    // An image whose sheet is gone keeps only its plain row.
    if !summary.aborted {
        for path in stored_cues.keys() {
            if !claimed.contains_key(Path::new(path)) {
                store::retain_subs(conn, path, &[0])?;
            }
        }
    }

    // Prune stored rows the walk didn't find. Never after an aborted walk, and
    // never when the root won't list (unplugged drive, dropped mount): both read
    // as empty while the files are still there.
    if !summary.aborted && std::fs::read_dir(root).is_ok() {
        summary.removed = store::prune_missing(conn, root, &walk.audio)?;
    }
    // Relink playlist members, listens and bookmarks orphaned by an earlier
    // prune.
    crate::playlists::reattach(conn)?;
    crate::listens::reattach(conn)?;
    crate::bookmarks::reattach(conn)?;
    Ok(summary)
}

/// Re-read exactly these files, the writer's write-back: the library
/// converges without a rescan. Blocking.
pub fn reindex(conn: &mut Connection, paths: &[PathBuf]) -> rusqlite::Result<usize> {
    let known = HashMap::new();
    // A named sheet means re-cut its images. Sheets beside a named audio file
    // count too, or a touched image comes back as one plain row.
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

    let mut targets: Vec<PathBuf> = paths.iter().filter(|p| is_audio(p)).cloned().collect();
    for image in claimed.keys() {
        targets.push(image.clone());
    }
    targets.sort();
    targets.dedup();

    let rows: Vec<TrackRow> = targets
        .par_iter()
        .flat_map(|path| match claimed.get(path) {
            // An empty stored-subs map forces the read.
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
        // Same subsong bookkeeping as a full scan, in the same order.
        for (image, claim) in &claimed {
            let keep: Vec<u16> = claim.tracks.iter().map(|t| t.number).collect();
            store::retain_subs(conn, &image.to_string_lossy(), &keep)?;
        }
        for path in targets.iter().filter(|p| !claimed.contains_key(*p)) {
            store::retain_subs(conn, &path.to_string_lossy(), &[0])?;
        }
        // A returning watched file gets the same reattach a scan runs.
        crate::playlists::reattach(conn)?;
        crate::listens::reattach(conn)?;
        crate::bookmarks::reattach(conn)?;
    }
    Ok(rows.len())
}

/// Kept apart from the store write so reads run in parallel and the write
/// stays one transaction.
enum Outcome {
    /// Vanished or wouldn't stat between the walk and the read.
    Missing,
    /// (mtime, size) matched the stored row.
    Unchanged,
    /// Boxed to keep the enum small.
    Indexed { row: Box<TrackRow>, untagged: bool },
}

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
    // Borrowed for the lookup: an allocation per skipped file is most of what
    // the fast path would cost.
    let path_str = path.to_string_lossy();
    if known.get(path_str.as_ref()) == Some(&(mtime, size)) {
        return Outcome::Unchanged;
    }

    let (row, untagged) = match read_tags(path) {
        Some(tags) => (tags, false),
        None => (fallback_row(path), true),
    };
    Outcome::Indexed {
        row: Box::new(TrackRow {
            path: path_str.into_owned(),
            size,
            mtime,
            ..row
        }),
        untagged,
    }
}

/// Read a file outside any scanned root (drop, CLI open) the way a scan
/// would. None only when it won't stat.
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

/// The scan's walk without the store, for maintenance passes. Honours
/// `exclude`. Blocking.
pub fn audio_files(root: &Path, exclude: &Exclusions) -> Vec<PathBuf> {
    audio_files_in(root, root, exclude)
}

/// For a folder under `root`: root-anchored patterns still anchor at the
/// root.
pub fn audio_files_in(root: &Path, dir: &Path, exclude: &Exclusions) -> Vec<PathBuf> {
    let mut walk = Walk::default();
    collect(root, dir, exclude, &mut walk);
    walk.audio
}

/// The one filter deciding what becomes a track, shared by the walk and the
/// watcher.
pub fn is_audio(path: &Path) -> bool {
    !is_junk(path)
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// .DS_Store and AppleDouble `._name` sidecars, which keep the real
/// extension and would land as rows that never decode.
pub fn is_junk(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name == ".DS_Store" || name.starts_with("._")
}

/// `.Trashes` and `$RECYCLE.BIN` would put deleted tracks back in the
/// library; the rest only cost walk time. Case-insensitive, like the
/// volumes.
const JUNK_DIRS: &[&str] = &[
    ".Trashes",
    ".Spotlight-V100",
    ".fseventsd",
    ".TemporaryItems",
    "$RECYCLE.BIN",
    "System Volume Information",
    "@eaDir",
];

/// Kept short on purpose: a wrong skip loses real music silently. Never
/// applied to the walk's own root.
pub fn is_junk_dir(path: &Path) -> bool {
    // A folder can have an AppleDouble sidecar of its own.
    if is_junk(path) {
        return true;
    }

    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| JUNK_DIRS.iter().any(|junk| name.eq_ignore_ascii_case(junk)))
}

/// Never playable and never a row; it only cuts the image beside it.
pub fn is_cue(path: &Path) -> bool {
    !is_junk(path)
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(CUE_EXTENSION))
}

/// Audio, or a cue sheet whose edit re-cuts its image. Filtering on
/// [`is_audio`] alone would drop sheet edits.
pub fn is_relevant(path: &Path) -> bool {
    is_audio(path) || is_cue(path)
}

#[derive(Default)]
struct Walk {
    audio: Vec<PathBuf>,
    cues: Vec<PathBuf>,
}

/// `root` is only there so slashed exclude patterns anchor at it.
fn collect(root: &Path, dir: &Path, exclude: &Exclusions, out: &mut Walk) {
    let mut seen = HashSet::new();
    // Seed with the root so a link back up to it stops too.
    if let Ok(canon) = std::fs::canonicalize(dir) {
        seen.insert(canon);
    }
    collect_into(root, dir, exclude, out, &mut seen);
}

fn collect_into(
    root: &Path,
    dir: &Path,
    exclude: &Exclusions,
    out: &mut Walk,
    seen: &mut HashSet<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Exclusions first: a matching folder is skipped without a stat.
        if exclude.matches(root, &path) {
            continue;
        }

        // The entry's kind avoids a stat per file; symlinks and filesystems that
        // omit the kind fall back to one.
        let is_dir = match entry.file_type() {
            Ok(ft) if !ft.is_symlink() => ft.is_dir(),
            _ => path.is_dir(),
        };
        if is_dir {
            // Skip OS metadata folders and volume trash outright.
            if is_junk_dir(&path) {
                continue;
            }

            // Symlink loop guard: skip a real path already walked. One that won't
            // canonicalize still gets walked, and read_dir ends the loop.
            if let Ok(canon) = std::fs::canonicalize(&path) {
                // Already walked this real path: a cycle, skip it.
                if !seen.insert(canon) {
                    continue;
                }
            }
            collect_into(root, &path, exclude, out, seen);
        } else if is_audio(&path) {
            out.audio.push(path);
        } else if is_cue(&path) {
            out.cues.push(path);
        }
    }
}

struct Claim {
    cue_path: PathBuf,
    /// Folded into the row's mtime, so a sheet edit reindexes the image.
    cue_mtime: i64,
    album: String,
    album_artist: String,
    genre: String,
    year: u16,
    tracks: Vec<crate::cue::CueTrack>,
}

/// A sheet that won't parse or resolves to nothing claims nothing. Two
/// sheets naming one image: the last read wins.
fn claims(cues: &[PathBuf], audio: &[PathBuf]) -> HashMap<PathBuf, Claim> {
    if cues.is_empty() {
        return HashMap::new();
    }
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

/// Loosening tries: exact name, any casing, then the same stem under any
/// audio extension (rippers write `.wav` and leave `.flac`). Only walked files
/// match, so an undecodable `.bin` image resolves to nothing.
fn resolve_image(
    cue_dir: &Path,
    arg: &str,
    by_dir: &HashMap<&Path, Vec<&Path>>,
) -> Option<PathBuf> {
    // Windows sheets use backslashes, and a multi-disc sheet may point into a
    // subfolder.
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

enum CueOutcome {
    Missing,
    /// Nothing read; holds the skipped track count.
    Unchanged(usize),
    Indexed(Vec<TrackRow>),
}

/// The unchanged check uses the combined mtime and the stored subsongs, so a
/// sheet edit alone still re-emits.
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
    // The later of image and sheet, so a touched sheet counts as a change.
    let mtime = image_mtime.max(claim.cue_mtime);
    let path_str = image.to_string_lossy().into_owned();
    let subs: Vec<u16> = claim.tracks.iter().map(|t| t.number).collect();
    if known.get(&path_str) == Some(&(mtime, size)) && stored_cues.get(&path_str) == Some(&subs) {
        return CueOutcome::Unchanged(subs.len());
    }

    // One probe for the whole disc, not one per track.
    let image_tags = read_tags(image).unwrap_or_else(|| fallback_row(image));
    CueOutcome::Indexed(cue_rows(image, claim, &image_tags, size, mtime))
}

/// The sheet wins on album-level fields; the image's tags fill gaps.
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
            // The last track runs to the file's end; a sheet that outlived its image
            // floors at zero.
            let duration_ms = track
                .span
                .len_ms()
                .unwrap_or_else(|| image_tags.duration_ms.saturating_sub(track.span.start_ms));
            let artist = track.performer.clone();
            let title = if track.title.is_empty() {
                image_tags.title.clone()
            } else {
                track.title.clone()
            };
            let album_artist = if claim.album_artist.is_empty() {
                artist.clone()
            } else {
                claim.album_artist.clone()
            };
            let album = if claim.album.is_empty() {
                image_tags.album.clone()
            } else {
                claim.album.clone()
            };
            // A subsong carries the image's sort name only where it kept the image's
            // value.
            let inherit = |value: &str, image_value: &str, image_sort: &str| {
                if value == image_value {
                    image_sort.to_string()
                } else {
                    String::new()
                }
            };
            TrackRow {
                title_sort: inherit(&title, &image_tags.title, &image_tags.title_sort),
                artist_sort: inherit(&artist, &image_tags.artist, &image_tags.artist_sort),
                album_artist_sort: inherit(
                    &album_artist,
                    &image_tags.album_artist,
                    &image_tags.album_artist_sort,
                ),
                album_sort: inherit(&album, &image_tags.album, &image_tags.album_sort),
                path: path.clone(),
                sub: track.number,
                remote_url: String::new(),
                remote_live: false,
                title,
                album_artist,
                artist,
                album,
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
                // The sheet knows nothing about discs.
                disc_no: image_tags.disc_no,
                track_no: track.number,
                duration_ms,
                codec: image_tags.codec.clone(),
                bitrate_kbps: image_tags.bitrate_kbps,
                sample_rate_hz: image_tags.sample_rate_hz,
                bit_depth: image_tags.bit_depth,
                rating: 0,
                // Album gain only: the image's track gain is the whole disc's.
                replay_gain: crate::replaygain::ReplayGain {
                    track_db: None,
                    track_peak: None,
                    album_db: image_tags.replay_gain.album_db,
                    album_peak: image_tags.replay_gain.album_peak,
                },
                // No tempo: the image's number describes the whole disc. Each subsong goes
                // to the analysis pass instead.
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

/// Isolated per file: a parser error or panic costs that file its tags,
/// never the scan.
///
/// MPEG, FLAC, MP4 and Opus parse natively so the rating (and Opus's R128
/// gain), which the generic tag drops, come off the same single parse. The
/// native file converts to a `TaggedFile` exactly as `Probe::read` would.
fn read_tags(path: &Path) -> Option<TrackRow> {
    let source = crate::tag_source::open(path).ok()?;
    let (file, rating, r128) = catch_unwind(AssertUnwindSafe(move || {
        let probe = lofty::probe::Probe::new(source)
            .guess_file_type()
            .ok()?
            .options(crate::parse_opts());
        let opts = crate::parse_opts();
        // guess_file_type rewinds, so read_from sees what Probe::read would.
        match probe.file_type() {
            Some(lofty::file::FileType::Mpeg) => {
                let mut reader = probe.into_inner();
                let mpeg = MpegFile::read_from(&mut reader, opts).ok()?;
                let rating = mpeg.id3v2().and_then(crate::rating::from_id3v2);
                Some((TaggedFile::from(mpeg), rating, None))
            }
            Some(lofty::file::FileType::Flac) => {
                let mut reader = probe.into_inner();
                let flac = FlacFile::read_from(&mut reader, opts).ok()?;
                let rating = flac.vorbis_comments().and_then(crate::rating::from_vorbis);
                Some((TaggedFile::from(flac), rating, None))
            }
            Some(lofty::file::FileType::Opus) => {
                let mut reader = probe.into_inner();
                let opus = OpusFile::read_from(&mut reader, opts).ok()?;
                let comments = opus.vorbis_comments();
                let rating = crate::rating::from_vorbis(comments);
                let r128 = crate::replaygain::read_r128(comments);
                Some((TaggedFile::from(opus), rating, r128))
            }
            Some(lofty::file::FileType::Mp4) => {
                let mut reader = probe.into_inner();
                let mp4 = lofty::mp4::Mp4File::read_from(&mut reader, opts).ok()?;
                let rating = mp4.ilst().and_then(crate::rating::from_ilst);
                Some((TaggedFile::from(mp4), rating, None))
            }
            _ => probe.read().ok().map(|f| (f, None, None)),
        }
    }))
    .ok()??;
    let mut row = fallback_row(path);
    row.duration_ms = file.properties().duration().as_millis() as u32;
    // A fragmented MP4 has empty sample tables, so lofty says zero; read the
    // movie header instead (see [`crate::mp4`]).
    if row.duration_ms == 0
        && let Some(secs) = crate::mp4::fragment_duration_secs(path)
    {
        row.duration_ms = (secs * 1000.0).round() as u32;
    }
    if let Some(codec) = match file.file_type() {
        lofty::file::FileType::Flac => Some("flac"),
        lofty::file::FileType::Mpeg => Some("mp3"),
        lofty::file::FileType::Wav => Some("wav"),
        lofty::file::FileType::Vorbis => Some("vorbis"),
        lofty::file::FileType::Opus => Some("opus"),
        lofty::file::FileType::Aiff => Some("aiff"),
        lofty::file::FileType::Aac => Some("aac"),
        // Mp4 holds AAC or ALAC and lofty doesn't say which; keep the guess.
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
    // Lossy formats have no bit depth.
    row.bit_depth = file.properties().bit_depth().unwrap_or(0);
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        let text =
            |v: Option<std::borrow::Cow<'_, str>>| v.map(|s| s.into_owned()).unwrap_or_default();
        if let Some(t) = tag.title().filter(|t| !t.is_empty()) {
            row.title = t.into_owned();
        }
        row.artist = text(tag.artist());
        // Falls back to the track artist so a plain album groups the same either
        // way.
        row.album_artist = tag
            .get_string(lofty::tag::ItemKey::AlbumArtist)
            .filter(|s| !s.is_empty())
            .unwrap_or(&row.artist)
            .to_string();
        row.album = text(tag.album());
        // No fallback to the display name: the projection decides that.
        row.title_sort = tag
            .get_string(lofty::tag::ItemKey::TrackTitleSortOrder)
            .unwrap_or_default()
            .to_string();
        row.artist_sort = tag
            .get_string(lofty::tag::ItemKey::TrackArtistSortOrder)
            .unwrap_or_default()
            .to_string();
        row.album_artist_sort = tag
            .get_string(lofty::tag::ItemKey::AlbumArtistSortOrder)
            .unwrap_or_default()
            .to_string();
        // Except here: an album artist borrowed from the artist borrows its sort
        // name. The cue path inherits the same way.
        if row.album_artist_sort.is_empty() && row.album_artist == row.artist {
            row.album_artist_sort = row.artist_sort.clone();
        }
        row.album_sort = tag
            .get_string(lofty::tag::ItemKey::AlbumTitleSortOrder)
            .unwrap_or_default()
            .to_string();
        // Vorbis repeats GENRE and ID3v2.4 null-separates TCON; lofty hands both
        // over as items.
        row.genre = crate::genre::join(tag.get_strings(lofty::tag::ItemKey::Genre));
        row.year = tag.date().map(|d| d.year).unwrap_or(0);
        row.disc_no = tag.disk().unwrap_or(0) as u16;
        row.track_no = tag.track().unwrap_or(0) as u16;
        row.bpm = crate::tempo::read(tag);
    }
    // Read across every tag: mp3gain writes to an APEv2 tag beside an ID3v2
    // primary that has none.
    row.replay_gain = replay_gain_across_tags(&file);
    // Opus carries R128 instead; a standard tag still wins.
    if let Some(r128) = r128
        && !row.replay_gain.any()
    {
        row.replay_gain = r128;
    }
    row.rating = rating.unwrap_or(0);
    Some(row)
}

/// First tag holding each key wins, per key, primary first. Only ReplayGain
/// reads this wide: a second tag disagreeing about the title is a conflict,
/// not a gap.
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

/// Filename as title, extension as codec. The caller fills path, size and
/// mtime.
fn fallback_row(path: &Path) -> TrackRow {
    TrackRow {
        title_sort: String::new(),
        artist_sort: String::new(),
        album_artist_sort: String::new(),
        album_sort: String::new(),
        path: String::new(),
        sub: 0,
        cue: None,
        remote_url: String::new(),
        remote_live: false,
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

    #[test]
    fn os_junk_is_neither_audio_nor_a_cue() {
        for name in [".DS_Store", "._track.mp3", "._album.cue", "._cover.jpg"] {
            let path = Path::new("/m/Album").join(name);
            assert!(is_junk(&path), "{name} is OS junk");
            assert!(!is_audio(&path), "{name} must not become a row");
            assert!(!is_cue(&path), "{name} must not cut an image");
            assert!(!is_relevant(&path), "{name} is not worth reindexing");
        }

        let track = Path::new("/m/Album/track.mp3");
        assert!(!is_junk(track));
        assert!(is_audio(track));
        assert!(!is_cue(track));

        let sheet = Path::new("/m/Album/album.cue");
        assert!(!is_junk(sheet));
        assert!(!is_audio(sheet));
        assert!(is_cue(sheet));

        assert!(is_junk_dir(Path::new("/m/.Trashes")));
        assert!(is_junk_dir(Path::new("/m/.spotlight-v100")));
        assert!(is_junk_dir(Path::new("/m/$RECYCLE.BIN")));
        assert!(is_junk_dir(Path::new("/m/System Volume Information")));
        assert!(is_junk_dir(Path::new("/m/@eaDir")));
        assert!(!is_junk_dir(Path::new("/m/Album")));
    }

    /// The walk is what the prune diffs against, so anything it yields here
    /// becomes a row.
    #[test]
    fn the_walk_yields_only_real_audio() {
        let dir = std::env::temp_dir().join(format!("rox-scanner-junk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["Album", ".Trashes/501", "@eaDir"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for name in [
            "Album/a.flac",
            "Album/._a.flac",
            "Album/.DS_Store",
            "Album/album.cue",
            "Album/._album.cue",
            ".DS_Store",
            ".Trashes/501/deleted.mp3",
            "@eaDir/thumb.mp3",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }

        let mut walk = Walk::default();
        collect(&dir, &dir, &Exclusions::default(), &mut walk);

        assert_eq!(
            walk.audio,
            vec![dir.join("Album/a.flac")],
            "sidecars, .DS_Store, and the junk folders all stay out"
        );
        assert_eq!(
            walk.cues,
            vec![dir.join("Album/album.cue")],
            "the sidecar of a sheet is not a sheet"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_excluded_folder_leaves_and_returns_with_its_pattern() {
        let dir = std::env::temp_dir().join(format!("rox-scanner-exclude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["Album", "Live/2019"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for name in ["Album/a.mp3", "Album/b.tmp.mp3", "Live/2019/set.mp3"] {
            std::fs::write(dir.join(name), b"not audio").unwrap();
        }

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        let everything = Exclusions::default();
        let patterns = Exclusions::new(&["live".to_string(), "*.tmp.*".to_string()]);

        let s = scan(&mut conn, &dir, &everything, |_, _, _| true).unwrap();
        assert_eq!(s.indexed, 3);

        let s = scan(&mut conn, &dir, &patterns, |_, _, _| true).unwrap();
        assert_eq!(s.removed, 2, "the Live folder and the temp file go");
        assert_eq!(store::count(&conn).unwrap(), 1);
        assert!(dir.join("Live/2019/set.mp3").exists());

        let s = scan(&mut conn, &dir, &everything, |_, _, _| true).unwrap();
        assert_eq!(s.indexed, 2, "and come back once the patterns do");
        assert_eq!(store::count(&conn).unwrap(), 3);

        let anchored = Exclusions::new(&["Live/2019".to_string()]);
        let walked = audio_files_in(&dir, &dir.join("Live"), &anchored);
        assert!(
            walked.is_empty(),
            "a walk from inside still anchors at the root"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reindex_rereads_named_files() {
        let dir = std::env::temp_dir().join("rox-scanner-reindex");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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
            store::meta_for_path(conn, crate::cue::LOCAL, path.to_str().unwrap())
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

    #[test]
    fn reindex_reads_sort_names() {
        let dir = std::env::temp_dir().join("rox-scanner-sortnames");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut audio = Vec::new();
        for frame in 0..3u32 {
            audio.extend([0xFF, 0xFB, 0x90, 0x00]);
            audio.extend((0..413u32).map(|i| ((frame * 413 + i) * 7 % 251) as u8));
        }
        let path = dir.join("track.mp3");
        std::fs::write(&path, &audio).unwrap();

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();

        let sorts = |conn: &Connection| {
            conn.query_row(
                "SELECT title_sort, artist_sort, album_artist_sort, album_sort
                 FROM tracks WHERE path = ?1",
                [path.to_str().unwrap()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap()
        };

        writer::commit(
            &path,
            &[Change {
                field: Field::Title,
                value: Some("Lemon".into()),
            }],
        )
        .unwrap();
        assert_eq!(reindex(&mut conn, std::slice::from_ref(&path)).unwrap(), 1);
        assert_eq!(
            sorts(&conn),
            (String::new(), String::new(), String::new(), String::new())
        );

        writer::commit(
            &path,
            &[
                Change {
                    field: Field::TitleSort,
                    value: Some("Lemon".into()),
                },
                Change {
                    field: Field::ArtistSort,
                    value: Some("Yonezu, Kenshi".into()),
                },
                Change {
                    field: Field::AlbumArtistSort,
                    value: Some("Yonezu, Kenshi".into()),
                },
                Change {
                    field: Field::AlbumSort,
                    value: Some("Bootleg".into()),
                },
            ],
        )
        .unwrap();
        assert_eq!(reindex(&mut conn, std::slice::from_ref(&path)).unwrap(), 1);
        assert_eq!(
            sorts(&conn),
            (
                "Lemon".to_string(),
                "Yonezu, Kenshi".to_string(),
                "Yonezu, Kenshi".to_string(),
                "Bootleg".to_string(),
            )
        );
    }

    #[test]
    fn a_borrowed_album_artist_borrows_its_sort_name() {
        let dir = std::env::temp_dir().join("rox-scanner-borrowed-sort");
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
            &[
                Change {
                    field: Field::Artist,
                    value: Some("米津玄師".into()),
                },
                Change {
                    field: Field::ArtistSort,
                    value: Some("Yonezu, Kenshi".into()),
                },
            ],
        )
        .unwrap();

        let row = read_one(&path).unwrap();
        assert_eq!(row.album_artist, "米津玄師");
        assert_eq!(row.album_artist_sort, "Yonezu, Kenshi");

        writer::commit(
            &path,
            &[Change {
                field: Field::AlbumArtist,
                value: Some("Various Artists".into()),
            }],
        )
        .unwrap();
        let row = read_one(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(row.album_artist, "Various Artists");
        assert_eq!(row.album_artist_sort, "");
    }

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

        let combined = read_one(&path).unwrap().rating;
        let standalone = crate::rating::read_path(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(combined, 75);
        assert_eq!(standalone, 75);
        assert_eq!(combined, standalone);
    }

    #[test]
    fn opus_rating_matches_across_read_paths() {
        use lofty::config::{ParseOptions, WriteOptions};
        use lofty::file::AudioFile;

        let dir =
            std::env::temp_dir().join(format!("rox-scanner-opus-rating-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.opus");
        std::fs::copy(opus_fixture(), &path).unwrap();

        let mut file = std::fs::File::open(&path).unwrap();
        let mut opus = OpusFile::read_from(&mut file, ParseOptions::new()).unwrap();
        opus.vorbis_comments_mut()
            .push(crate::rating::FMPS_KEY.to_string(), "0.75".to_string());
        opus.save_to_path(&path, WriteOptions::default()).unwrap();

        let combined = read_one(&path).unwrap().rating;
        let standalone = crate::rating::read_path(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(combined, 75);
        assert_eq!(standalone, Some(75));
    }

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
        assert_eq!(row.replay_gain.album_peak, None);
    }

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

    #[test]
    fn rescan_prunes_deleted_files() {
        let dir = std::env::temp_dir().join("rox-scanner-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let files = ["a/1.mp3", "a/2.mp3", "b/1.mp3"];
        for name in files {
            std::fs::write(dir.join(name), b"not audio").unwrap();
        }

        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        let scan = |conn: &mut Connection| {
            scan(conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap()
        };

        let s = scan(&mut conn);
        assert_eq!(s.indexed, 3);
        assert_eq!(s.removed, 0);
        assert_eq!(store::count(&conn).unwrap(), 3);

        std::fs::remove_file(dir.join("a/2.mp3")).unwrap();
        let s = scan(&mut conn);
        assert_eq!(s.removed, 1);
        assert_eq!(store::count(&conn).unwrap(), 2);
        assert!(
            store::id_for_path(
                &conn,
                crate::cue::LOCAL,
                dir.join("a/2.mp3").to_str().unwrap()
            )
            .unwrap()
            .is_none()
        );
        assert!(
            store::id_for_path(
                &conn,
                crate::cue::LOCAL,
                dir.join("a/1.mp3").to_str().unwrap()
            )
            .unwrap()
            .is_some()
        );

        std::fs::remove_dir_all(&dir).unwrap();
        let s = scan(&mut conn);
        assert_eq!(s.removed, 0);
        assert_eq!(store::count(&conn).unwrap(), 2);
    }

    /// "a/" against "a.mp3" is where path order and string order disagree;
    /// sorted wrong, the prune would delete rows for files still there.
    #[test]
    fn a_rescan_keeps_files_whose_names_straddle_a_folder() {
        let dir = std::env::temp_dir().join("rox-scanner-prune-order");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("a b")).unwrap();
        let files = [
            "a.mp3",
            "a b.mp3",
            "a-b.mp3",
            "a/1.mp3",
            "a/2.mp3",
            "a b/1.mp3",
        ];
        for name in files {
            std::fs::write(dir.join(name), b"not audio").unwrap();
        }
        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        assert_eq!(
            scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true)
                .unwrap()
                .indexed,
            files.len()
        );

        let s = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(s.removed, 0);
        assert_eq!(s.unchanged, files.len());
        assert_eq!(store::count(&conn).unwrap(), files.len() as u64);

        std::fs::remove_file(dir.join("a/1.mp3")).unwrap();
        let s = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(s.removed, 1);
        assert!(
            store::id_for_path(
                &conn,
                crate::cue::LOCAL,
                dir.join("a.mp3").to_str().unwrap()
            )
            .unwrap()
            .is_some(),
            "the file next to the folder is untouched"
        );
    }

    #[test]
    fn an_aborted_scan_prunes_nothing() {
        let dir = std::env::temp_dir().join("rox-scanner-prune-aborted");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["1.mp3", "2.mp3", "3.mp3"] {
            std::fs::write(dir.join(name), b"not audio").unwrap();
        }
        let mut conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        assert_eq!(
            scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true)
                .unwrap()
                .indexed,
            3
        );

        std::fs::remove_file(dir.join("2.mp3")).unwrap();
        let s = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| false).unwrap();
        assert!(s.aborted, "the progress callback said stop");
        assert_eq!(s.removed, 0);
        assert_eq!(store::count(&conn).unwrap(), 3);

        let s = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(s.removed, 1);
        assert_eq!(store::count(&conn).unwrap(), 2);
    }

    #[test]
    fn read_one_fills_path_on_loose_file() {
        let dir = std::env::temp_dir().join("rox-scanner-read-one");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("loose track.mp3");
        std::fs::write(&path, b"not audio").unwrap();

        let row = read_one(&path).unwrap();
        assert_eq!(row.path, path.to_string_lossy());
        assert_eq!(row.title, "loose track");
        assert_eq!(row.codec, "mp3");
        assert_eq!(row.size, 9);
        assert!(row.mtime > 0);

        assert!(read_one(&dir.join("missing.mp3")).is_none());
    }

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

    /// A twelve-second image cut into 3s, 3s, and the rest.
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

    fn cue_fixture(name: &str) -> (PathBuf, Connection) {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("disc.wav"), pcm_wav(44100, 16, 2, 12)).unwrap();
        let conn = store::open(&dir.join("library.db")).unwrap();
        store::init_schema(&conn).unwrap();
        (dir, conn)
    }

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

    /// Second-granularity mtimes: a rewrite inside the same second would look
    /// untouched.
    fn touch_ahead(path: &Path) {
        let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(ahead)
            .unwrap();
    }

    #[test]
    fn a_cue_sheet_splits_its_image_into_tracks() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-split");
        let image = dir.join("disc.wav");
        std::fs::write(dir.join("disc.cue"), SHEET).unwrap();

        let summary = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(summary.indexed, 3, "one row per cue track");
        assert_eq!(store::count(&conn).unwrap(), 3, "and no plain image row");

        let rows = subsongs(&conn, &image);
        assert_eq!(
            rows,
            [
                (1, "One".to_string(), "The Band".to_string(), 3_000),
                (2, "Two".to_string(), "Guest".to_string(), 3_000),
                (3, "Three".to_string(), "The Band".to_string(), 6_000),
            ]
        );

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

        let path = image.to_str().unwrap();
        assert_eq!(
            store::queue_meta_for_key(&conn, crate::cue::LOCAL, path, 2)
                .unwrap()
                .span,
            Some(crate::cue::Span {
                start_ms: 3_000,
                end_ms: Some(6_000)
            })
        );
        assert_eq!(
            store::queue_meta_for_key(&conn, crate::cue::LOCAL, path, 3)
                .unwrap()
                .span,
            Some(crate::cue::Span {
                start_ms: 6_000,
                end_ms: None
            })
        );

        let summary = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!((summary.indexed, summary.unchanged), (0, 3));
        assert_eq!(summary.removed, 0, "a claimed image is not a missing file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cue_sheet_resolves_its_image_by_stem() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-stem");
        std::fs::write(dir.join("disc.cue"), SHEET.replace("disc.wav", "DISC.aiff")).unwrap();

        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(subsongs(&conn, &dir.join("disc.wav")).len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_deleted_cue_sheet_gives_the_image_back_whole() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-deleted");
        let image = dir.join("disc.wav");
        std::fs::write(dir.join("disc.cue"), SHEET).unwrap();
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(store::count(&conn).unwrap(), 3);

        std::fs::remove_file(dir.join("disc.cue")).unwrap();
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();

        assert_eq!(
            subsongs(&conn, &image),
            [(0, "disc".to_string(), String::new(), 12_000)],
            "one row for the whole image, titled off its filename"
        );
        assert!(store::cue_spans(&conn).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_cue_sheet_replaces_the_plain_row() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-added");
        let image = dir.join("disc.wav");
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
        assert_eq!(subsongs(&conn, &image).len(), 1, "one plain row to start");

        std::fs::write(dir.join("disc.cue"), SHEET).unwrap();
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();

        let subs: Vec<u16> = subsongs(&conn, &image).iter().map(|r| r.0).collect();
        assert_eq!(subs, [1, 2, 3], "the plain row gave way to the tracks");
        assert_eq!(store::cue_spans(&conn).unwrap().len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editing_a_cue_sheet_reindexes_the_image() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-edited");
        let image = dir.join("disc.wav");
        let cue = dir.join("disc.cue");
        std::fs::write(&cue, SHEET).unwrap();
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();

        // Same three tracks, so only the mtime can trigger the re-read.
        std::fs::write(&cue, SHEET.replace("\"Two\"", "\"Second\"")).unwrap();
        touch_ahead(&cue);
        let summary = scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();

        assert_eq!((summary.indexed, summary.unchanged), (3, 0));
        assert_eq!(subsongs(&conn, &image)[1].1, "Second");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shortened_cue_sheet_drops_the_track_it_no_longer_lists() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-shortened");
        let image = dir.join("disc.wav");
        let cue = dir.join("disc.cue");
        std::fs::write(&cue, SHEET).unwrap();
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();

        let shorter = SHEET
            .split("  TRACK 03")
            .next()
            .expect("the sheet splits at its last track")
            .to_string();
        std::fs::write(&cue, shorter).unwrap();
        touch_ahead(&cue);
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();

        let rows = subsongs(&conn, &image);
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            rows[1].3, 9_000,
            "track two now runs to the end of the file"
        );
        assert_eq!(store::cue_spans(&conn).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reindex_takes_a_cue_sheet_as_a_re_cut() {
        let (dir, mut conn) = cue_fixture("rox-scanner-cue-reindex");
        let image = dir.join("disc.wav");
        scan(&mut conn, &dir, &Exclusions::default(), |_, _, _| true).unwrap();
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

    #[test]
    fn cue_tracks_inherit_the_sort_names_of_the_values_they_keep() {
        let image = fallback_row(Path::new("/music/disc.flac"));
        let image_tags = TrackRow {
            title: "Disc".into(),
            title_sort: "Disc, The".into(),
            artist: "米津玄師".into(),
            artist_sort: "Yonezu, Kenshi".into(),
            album_artist: "米津玄師".into(),
            album_artist_sort: "Yonezu, Kenshi".into(),
            album: "STRAY SHEEP".into(),
            album_sort: "Stray Sheep".into(),
            ..image
        };
        let track = |number: u16, title: &str, performer: &str| crate::cue::CueTrack {
            number,
            title: title.into(),
            performer: performer.into(),
            span: crate::cue::Span {
                start_ms: u32::from(number) * 1_000,
                end_ms: None,
            },
        };
        let claim = Claim {
            cue_path: PathBuf::from("/music/disc.cue"),
            cue_mtime: 0,
            album: String::new(),
            album_artist: String::new(),
            genre: String::new(),
            year: 0,
            tracks: vec![track(1, "One", "米津玄師"), track(2, "Two", "Guest")],
        };

        let rows = cue_rows(Path::new("/music/disc.flac"), &claim, &image_tags, 0, 0);
        let sorts = |row: &TrackRow| {
            (
                row.title_sort.clone(),
                row.artist_sort.clone(),
                row.album_artist_sort.clone(),
                row.album_sort.clone(),
            )
        };
        assert_eq!(
            sorts(&rows[0]),
            (
                String::new(),
                "Yonezu, Kenshi".to_string(),
                "Yonezu, Kenshi".to_string(),
                "Stray Sheep".to_string()
            )
        );
        assert_eq!(
            sorts(&rows[1]),
            (
                String::new(),
                String::new(),
                String::new(),
                "Stray Sheep".to_string()
            )
        );
    }

    /// One second of a 440 Hz tone.
    fn opus_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../rox-playback/tests/fixtures/tone-440.opus")
    }

    #[test]
    fn read_one_indexes_an_opus_file() {
        let row = read_one(&opus_fixture()).expect("the fixture is readable");
        assert_eq!(row.codec, "opus");
        assert!(
            row.duration_ms.abs_diff(1000) <= 10,
            "one second within 10 ms, got {} ms",
            row.duration_ms
        );
        // The row reports what lofty reads; playback resamples to 48 kHz anyway.
        assert_eq!(row.sample_rate_hz, 48_000);
        assert!(
            row.replay_gain.track_db.is_none(),
            "the fixture is untagged"
        );
    }

    #[test]
    fn read_one_converts_an_opus_r128_gain_to_replaygain() {
        use lofty::config::{ParseOptions, WriteOptions};
        use lofty::file::AudioFile;

        let dir =
            std::env::temp_dir().join(format!("rox-scanner-opus-r128-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.opus");
        std::fs::copy(opus_fixture(), &path).unwrap();

        let mut file = std::fs::File::open(&path).unwrap();
        let mut opus = OpusFile::read_from(&mut file, ParseOptions::new()).unwrap();
        opus.vorbis_comments_mut()
            .push("R128_TRACK_GAIN".to_string(), "-1280".to_string());
        opus.save_to_path(&path, WriteOptions::default()).unwrap();

        let row = read_one(&path).unwrap();
        assert_eq!(row.replay_gain.track_db, Some(0.0));
        assert_eq!(
            row.replay_gain.album_db, None,
            "only the track key was written"
        );
        assert_eq!(
            row.replay_gain.track_peak, None,
            "the R128 scheme has no peaks to read"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
