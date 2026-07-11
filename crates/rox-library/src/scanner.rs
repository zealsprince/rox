//! The scan pipeline: walk folders, read tags through lofty (ADR 4's single
//! metadata layer), and upsert rows in batches. Unchanged files, judged by
//! (mtime, size), are skipped without touching their tags. A file whose tags
//! will not parse still gets indexed under its filename, so the library never
//! silently loses a playable file.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lofty::prelude::*;
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
}

/// Scan `root` recursively into the store. Blocking; run it off the UI thread.
pub fn scan(conn: &mut Connection, root: &Path) -> rusqlite::Result<ScanSummary> {
    let known = store::local_files(conn)?;
    let mut files = Vec::new();
    collect(root, &mut files);
    files.sort();

    let mut summary = ScanSummary::default();
    let mut batch: Vec<TrackRow> = Vec::with_capacity(BATCH);
    for path in files {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let path_str = path.to_string_lossy().into_owned();
        if known.get(&path_str) == Some(&(mtime, size)) {
            summary.unchanged += 1;
            continue;
        }

        let row = match read_tags(&path) {
            Some(tags) => tags,
            None => {
                summary.untagged += 1;
                fallback_row(&path)
            }
        };
        batch.push(TrackRow { path: path_str, size, mtime, ..row });
        summary.indexed += 1;
        if batch.len() == BATCH {
            store::insert_batch(conn, &batch)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        store::insert_batch(conn, &batch)?;
    }
    Ok(summary)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
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
    let file = catch_unwind(AssertUnwindSafe(|| lofty::read_from_path(path)))
        .ok()?
        .ok()?;
    let mut row = fallback_row(path);
    row.duration_ms = file.properties().duration().as_millis() as u32;
    if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
        let text = |v: Option<std::borrow::Cow<'_, str>>| {
            v.map(|s| s.into_owned()).unwrap_or_default()
        };
        if let Some(t) = tag.title().filter(|t| !t.is_empty()) {
            row.title = t.into_owned();
        }
        row.artist = text(tag.artist());
        row.album = text(tag.album());
        row.genre = text(tag.genre());
        row.year = tag.date().map(|d| d.year).unwrap_or(0);
        row.track_no = tag.track().unwrap_or(0) as u16;
    }
    Some(row)
}

/// The row a file gets when its tags cannot be read: filename as title,
/// everything else empty. path/size/mtime are filled in by the caller.
fn fallback_row(path: &Path) -> TrackRow {
    TrackRow {
        path: String::new(),
        title: filename_title(path),
        artist: String::new(),
        album: String::new(),
        genre: String::new(),
        year: 0,
        track_no: 0,
        duration_ms: 0,
        size: 0,
        mtime: 0,
    }
}

fn filename_title(path: &Path) -> String {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    name.into_owned()
}
