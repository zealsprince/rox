//! Lyrics for a track: where to find them, how to read the LRC-ish text
//! players store, and how to save an edit back. Three homes are checked,
//! a sidecar file next to the audio file, the app's own lyrics store,
//! and the embedded tag, and the one a load came from is remembered so
//! an edit lands back in the same place rather than guessing. The reader
//! never touches the audio stream and the tag save rides the writer's
//! atomic layer; the sidecar and store saves clone and rename the same way.
//! Blocking IO throughout, run it off the UI thread.
//!
//! The parser is deliberately forgiving. A line's leading `[mm:ss.xx]`
//! groups become timestamps (several on one line repeat the text at each
//! time), an `[offset:ms]` tag shifts them, and the other id tags
//! (`[ar:]`, `[ti:]`, and the like) are dropped. Text with no timestamps
//! at all comes back as plain lines in file order, so an unsynced sheet
//! still reads.

use std::fs;
use std::path::{Path, PathBuf};

use crate::writer::{self, Change, Field};

/// The sidecar extensions checked next to the audio file, timed format
/// first. Each is tried both as a stem swap (track.lrc) and appended to
/// the whole name (track.mp3.lrc), the two conventions in the wild.
const SIDECAR_EXTS: [&str; 2] = ["lrc", "txt"];

/// Where a track's lyrics came from, so an edit saves back to the same
/// place instead of picking one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The embedded tag (USLT on ID3v2, UNSYNCEDLYRICS on Vorbis).
    Tag,
    /// A sidecar file beside the audio file.
    Sidecar(PathBuf),
    /// A sheet in the app's own lyrics store, so library folders carry
    /// nothing extra.
    Store(PathBuf),
}

/// One lyric line: its start time in seconds when the source timed it,
/// None when it did not, and the text.
#[derive(Clone, Debug)]
pub struct Line {
    pub at: Option<f64>,
    pub text: String,
}

/// A track's loaded lyrics: the raw text an editor round-trips, the
/// parsed lines a display walks, and where both came from.
pub struct Lyrics {
    pub source: Source,
    pub text: String,
    pub lines: Vec<Line>,
    /// At least one line carries a timestamp, so a display can follow
    /// playback rather than only scroll.
    pub synced: bool,
}

/// A track's lyrics from the first home that has them: a sidecar file,
/// then the app's store under `store_dir`, then the embedded tag. None
/// when none carries any. A sidecar wins over everything: it is where
/// timed `.lrc` lyrics live, and a file placed next to the track is the
/// stronger signal of intent than the store the app fills on its own.
pub fn load(path: &Path, store_dir: Option<&Path>) -> Option<Lyrics> {
    for side in sidecar_candidates(path) {
        if let Ok(text) = fs::read_to_string(&side) {
            if !text.trim().is_empty() {
                return Some(build(text, Source::Sidecar(side)));
            }
        }
    }
    if let Some(dir) = store_dir {
        let file = store_file(dir, path);
        if let Ok(text) = fs::read_to_string(&file) {
            if !text.trim().is_empty() {
                return Some(build(text, Source::Store(file)));
            }
        }
    }
    let text = writer::read(path)
        .ok()?
        .into_iter()
        .find(|(field, _)| *field == Field::Lyrics)
        .map(|(_, value)| value)
        .filter(|text| !text.trim().is_empty())?;
    Some(build(text, Source::Tag))
}

/// Save edited lyrics back to `target`. Tag lyrics go through the
/// writer's atomic commit (a clear removes the frame); a sidecar or
/// store file is rewritten in place, or unlinked when cleared. The store
/// folder is created on the first write.
pub fn save(path: &Path, target: &Source, text: &str) -> Result<(), String> {
    match target {
        Source::Tag => {
            let value = (!text.trim().is_empty()).then(|| text.to_string());
            writer::commit(path, &[Change { field: Field::Lyrics, value }])
        }
        Source::Sidecar(file) => save_file(file, text, false),
        Source::Store(file) => save_file(file, text, true),
    }
}

/// Write or clear one plain lyrics file, making its folder first when
/// asked (the store's folder does not exist until something saves).
fn save_file(file: &Path, text: &str, make_dir: bool) -> Result<(), String> {
    if text.trim().is_empty() {
        return match fs::remove_file(file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove lyrics file: {e}")),
        };
    }
    if make_dir {
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create lyrics folder: {e}"))?;
        }
    }
    // A sibling clone and rename, so a crash mid-write never leaves the
    // sheet truncated.
    let tmp = writer::tmp_path(file);
    fs::write(&tmp, text).map_err(|e| format!("write lyrics file: {e}"))?;
    fs::rename(&tmp, file).map_err(|e| format!("rename lyrics file: {e}"))
}

/// The store file for a track: one flat folder, the name a stable hash
/// of the whole track path, so no library folder shape gets mirrored
/// and a track maps to the same file every time. FNV-1a, plenty of
/// spread for library-sized sets.
pub fn store_file(dir: &Path, path: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{hash:016x}.lrc"))
}

/// The `.lrc` sidecar path for a track, for saving lyrics to a file when
/// none existed to load.
pub fn default_sidecar(path: &Path) -> PathBuf {
    path.with_extension("lrc")
}

/// Format a position in seconds as an LRC time tag, `[mm:ss.xx]`, the
/// stamp the editor prepends to a line.
pub fn format_stamp(secs: f64) -> String {
    let secs = secs.max(0.0);
    let mins = (secs / 60.0).floor();
    format!("[{:02}:{:05.2}]", mins as u64, secs - mins * 60.0)
}

/// Strip a line's leading LRC time tags, returning the lyric text after
/// them. A leading non-time bracket (an id tag) stops the strip, so it
/// and the rest of the line are left alone.
pub fn strip_leading_stamps(line: &str) -> &str {
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start();
        let Some(inner_end) = trimmed.strip_prefix('[').and_then(|r| r.find(']')) else {
            return trimmed;
        };
        if parse_time(&trimmed[1..=inner_end]).is_none() {
            return trimmed;
        }
        rest = &trimmed[inner_end + 2..];
    }
}

fn build(text: String, source: Source) -> Lyrics {
    let (lines, synced) = parse(&text);
    Lyrics { source, text, lines, synced }
}

/// The sidecar paths to try for a track, in order.
fn sidecar_candidates(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(SIDECAR_EXTS.len() * 2);
    for ext in SIDECAR_EXTS {
        out.push(path.with_extension(ext));
        let mut full = path.as_os_str().to_os_string();
        full.push(".");
        full.push(ext);
        out.push(PathBuf::from(full));
    }
    out
}

/// Parse LRC-ish text into lines, plus whether any line was timed.
pub fn parse(text: &str) -> (Vec<Line>, bool) {
    // The offset tag can sit anywhere; find it first so every timed line
    // shifts by it. Positive offset means the lyrics run early, so it
    // subtracts from each time.
    let offset = text.lines().find_map(offset_tag).unwrap_or(0.0) / 1000.0;

    let mut timed = Vec::new();
    for raw in text.lines() {
        let (times, body) = scan_times(raw);
        for at in times {
            timed.push(Line {
                at: Some((at - offset).max(0.0)),
                text: body.clone(),
            });
        }
    }
    if !timed.is_empty() {
        timed.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
        return (timed, true);
    }

    // No timestamps anywhere: a plain sheet, kept in file order with its
    // blank lines, so verse spacing survives.
    let plain = text
        .lines()
        .map(|line| Line {
            at: None,
            text: line.trim_end().to_string(),
        })
        .collect();
    (plain, false)
}

/// Strip a line's leading `[..]` groups, returning the timestamps among
/// them in seconds and the lyric text left after them. Id tags among the
/// groups (no `mm:ss` shape) are dropped.
fn scan_times(line: &str) -> (Vec<f64>, String) {
    let mut rest = line;
    let mut times = Vec::new();
    loop {
        let trimmed = rest.trim_start();
        let Some(inner_end) = trimmed.strip_prefix('[').and_then(|r| r.find(']')) else {
            rest = trimmed;
            break;
        };
        let inner = &trimmed[1..=inner_end];
        if let Some(secs) = parse_time(inner) {
            times.push(secs);
        }
        rest = &trimmed[inner_end + 2..];
    }
    (times, rest.trim_end().to_string())
}

/// Parse an LRC time-tag body ("mm:ss", "mm:ss.xx", "mm:ss.xxx") into
/// seconds. None for id tags and anything else.
fn parse_time(inner: &str) -> Option<f64> {
    let (mins, secs) = inner.split_once(':')?;
    let mins: f64 = mins.trim().parse().ok()?;
    let secs: f64 = secs.trim().parse().ok()?;
    (mins >= 0.0 && (0.0..60.0).contains(&secs)).then_some(mins * 60.0 + secs)
}

/// The milliseconds of an `[offset:ms]` tag, if this line is one.
fn offset_tag(line: &str) -> Option<f64> {
    let inner = line.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case("offset")
        .then(|| value.trim().parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_lines_parse_and_sort() {
        let (lines, synced) = parse("[00:12.50]second\n[00:01.00]first\n");
        assert!(synced);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].at, Some(1.0));
        assert_eq!(lines[1].text, "second");
        assert_eq!(lines[1].at, Some(12.5));
    }

    #[test]
    fn repeated_timestamps_repeat_the_line() {
        let (lines, _) = parse("[00:05.00][00:20.00]chorus\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].at, Some(5.0));
        assert_eq!(lines[1].at, Some(20.0));
        assert!(lines.iter().all(|l| l.text == "chorus"));
    }

    #[test]
    fn id_tags_drop_and_offset_shifts() {
        let (lines, synced) = parse("[ti:Song]\n[offset:500]\n[00:10.00]line\n");
        assert!(synced);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "line");
        // A +500ms offset runs the lyrics early, so the time drops half a
        // second.
        assert_eq!(lines[0].at, Some(9.5));
    }

    #[test]
    fn stamp_formats_and_strips_round_trip() {
        assert_eq!(format_stamp(83.5), "[01:23.50]");
        assert_eq!(format_stamp(0.0), "[00:00.00]");
        // A fresh line keeps its text; a stamped line loses only the
        // stamp, an id tag and plain text stay put.
        assert_eq!(strip_leading_stamps("hello"), "hello");
        assert_eq!(strip_leading_stamps("[00:12.00]hello"), "hello");
        assert_eq!(strip_leading_stamps("[00:01.00][00:05.00]hi"), "hi");
        assert_eq!(strip_leading_stamps("[ti:Song]"), "[ti:Song]");
    }

    #[test]
    fn store_files_are_stable_and_distinct() {
        let dir = Path::new("/data/lyrics");
        let a = store_file(dir, Path::new("/music/a.mp3"));
        let b = store_file(dir, Path::new("/music/b.mp3"));
        assert_eq!(a, store_file(dir, Path::new("/music/a.mp3")));
        assert_ne!(a, b);
        assert!(a.starts_with(dir));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("lrc"));
    }

    #[test]
    fn plain_text_keeps_lines_untimed() {
        let (lines, synced) = parse("verse one\n\nverse two\n");
        assert!(!synced);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.at.is_none()));
        assert_eq!(lines[1].text, "");
    }
}
