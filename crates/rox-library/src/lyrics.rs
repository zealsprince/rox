//! Lyrics for a track: where to find them, how to read the LRC-ish text
//! players store, and how to save an edit back. Three homes are checked,
//! a sidecar file next to the audio file, the app's own lyrics store,
//! and the embedded tag, and the one a load came from is remembered so
//! an edit is saved back to the same place rather than guessing. The reader
//! never touches the audio stream and the tag save goes through the writer's
//! atomic layer; the sidecar and store saves clone and rename the same way.
//! Blocking IO throughout, run it off the UI thread.
//!
//! A fourth state overrides those three: a track can be marked as having
//! no lyrics at all. Clearing a sheet only empties whichever home held it,
//! and an instrumental or a mis-tagged track would just be refilled by the
//! next automatic lookup, so a save of nothing leaves a marker in the store
//! and the marker outranks every home on the way back in.
//!
//! The parser is deliberately forgiving. A line's leading `[mm:ss.xx]`
//! groups become timestamps (several on one line repeat the text at each
//! time), an `[offset:ms]` tag shifts them, and the other id tags
//! (`[ar:]`, `[ti:]`, and the like) are dropped. Text with no timestamps
//! at all comes back as plain lines in file order, so an unsynced sheet
//! still reads.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// A sheet in the app's own lyrics store, so library folders get
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
/// parsed lines a display steps through, and where both came from.
pub struct Lyrics {
    pub source: Source,
    pub text: String,
    pub lines: Vec<Line>,
    /// At least one line has a timestamp, so a display can follow
    /// playback rather than only scroll.
    pub synced: bool,
}

/// A track's lyrics from the first home that has them: a sidecar file,
/// then the app's store under `store_dir`, then the embedded tag. None
/// when none of them has any. A sidecar wins over everything: it's where
/// timed `.lrc` lyrics are kept, and a file placed next to the track is the
/// stronger signal of intent than the store the app fills on its own.
///
/// A track marked as having none reads as none whatever the homes hold,
/// so the mark is one answer and not three to keep in step.
pub fn load(path: &Path, store_dir: Option<&Path>) -> Option<Lyrics> {
    if marked_none(path, store_dir) {
        return None;
    }
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
    Some(build(tag_lyrics(path)?, Source::Tag))
}

/// The words the embedded tag holds, or None when the frame is missing
/// or blank. Blank counts as missing throughout: a file that kept an empty
/// USLT frame reads as a track with no lyrics, not a track with none of
/// them.
pub(crate) fn tag_lyrics(path: &Path) -> Option<String> {
    writer::read(path)
        .ok()?
        .into_iter()
        .find(|(field, _)| *field == Field::Lyrics)
        .map(|(_, value)| value)
        .filter(|text| !text.trim().is_empty())
}

/// Take a track's lyrics out of every home at once: each sidecar beside
/// it, the store sheet, and the embedded tag. [`save`] only ever touches
/// the one home its target names, which leaves the others to surface the
/// moment the first is gone, so wiping is its own operation rather than a
/// clear of whichever home happened to win the last load.
///
/// The tag is only rewritten when it actually has words, so wiping a
/// track whose sheet was a sidecar never rewrites the audio file. The mark
/// is left to the caller: this removes, [`set_marked_none`] makes it stay
/// removed.
pub fn wipe(path: &Path, store_dir: Option<&Path>) -> Result<(), String> {
    for side in sidecar_candidates(path) {
        remove_if_present(&side).map_err(|e| format!("remove lyrics file: {e}"))?;
    }
    if let Some(dir) = store_dir {
        remove_if_present(&store_file(dir, path))
            .map_err(|e| format!("remove lyrics file: {e}"))?;
    }
    if tag_lyrics(path).is_some() {
        writer::commit(
            path,
            &[Change {
                field: Field::Lyrics,
                value: None,
            }],
        )?;
    }
    Ok(())
}

/// Delete a file, counting an absent one as done. Every lyrics home is
/// optional, so a clear passes over the ones that were never there.
fn remove_if_present(file: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Save edited lyrics back to `target`. Tag lyrics go through the
/// writer's atomic commit (a clear removes the frame); a sidecar or
/// store file is rewritten in place, or unlinked when cleared. The store
/// folder is created on the first write.
///
/// Saving nothing is a statement, not just an empty write: it marks the
/// track as having no lyrics under `store_dir`, and saving words again
/// takes the mark back off.
pub fn save(
    path: &Path,
    target: &Source,
    text: &str,
    store_dir: Option<&Path>,
) -> Result<(), String> {
    match target {
        Source::Tag => {
            let value = (!text.trim().is_empty()).then(|| text.to_string());
            writer::commit(
                path,
                &[Change {
                    field: Field::Lyrics,
                    value,
                }],
            )
        }
        Source::Sidecar(file) => save_file(file, text, false),
        Source::Store(file) => save_file(file, text, true),
    }?;
    // Only once the write succeeded, so a failed save leaves the mark where
    // it was rather than claiming a clear that never happened.
    match store_dir {
        Some(dir) => set_marked_none(path, dir, text.trim().is_empty()),
        None => Ok(()),
    }
}

/// Whether the track is marked as having no lyrics, the state a cleared
/// sheet leaves behind so nothing refills it.
pub fn marked_none(path: &Path, store_dir: Option<&Path>) -> bool {
    store_dir.is_some_and(|dir| none_marker(dir, path).exists())
}

/// Set or lift the "no lyrics" mark. The mark is an empty file beside the
/// store's sheets, so it costs a `stat` to read and persists across
/// restarts without a column of its own.
pub fn set_marked_none(path: &Path, store_dir: &Path, on: bool) -> Result<(), String> {
    let file = none_marker(store_dir, path);
    if !on {
        return remove_if_present(&file).map_err(|e| format!("clear lyrics mark: {e}"));
    }
    fs::create_dir_all(store_dir).map_err(|e| format!("create lyrics folder: {e}"))?;
    fs::write(&file, []).map_err(|e| format!("write lyrics mark: {e}"))
}

/// Write or clear one plain lyrics file, making its folder first when
/// asked (the store's folder does not exist until something saves).
fn save_file(file: &Path, text: &str, make_dir: bool) -> Result<(), String> {
    if text.trim().is_empty() {
        return remove_if_present(file).map_err(|e| format!("remove lyrics file: {e}"));
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
/// and a track maps to the same file every time.
pub fn store_file(dir: &Path, path: &Path) -> PathBuf {
    store_entry(dir, path, "lrc")
}

/// The "no lyrics" mark for a track, the store sheet's name under another
/// extension so both stay together and neither can be mistaken for the
/// other.
pub fn none_marker(dir: &Path, path: &Path) -> PathBuf {
    store_entry(dir, path, "none")
}

/// One store entry for a track under `ext`. FNV-1a over the whole track
/// path, plenty of spread for library-sized sets.
fn store_entry(dir: &Path, path: &Path, ext: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{hash:016x}.{ext}"))
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
    Lyrics {
        source,
        text,
        lines,
        synced,
    }
}

/// The sidecar paths to try for a track, in order. Public because a file
/// that moves takes its lyrics with it: the rename steps through this list
/// for the old path and the new one and moves what it finds, position by
/// position, so a `.mp3.lrc` becomes a `.flac.lrc` and not the other
/// convention.
pub fn sidecar_candidates(path: &Path) -> Vec<PathBuf> {
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
    // The offset tag can appear anywhere; find it first so every timed line
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
    // blank lines, so verse spacing is kept.
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

/// How long a rest waits past the line it follows before the sheet moves
/// to it, so the last words linger instead of blinking away.
const REST_HOLD_SECS: f64 = 4.0;

/// The loaded sheet with rests woven in: a leading blank line before a
/// first sung line that opens past `gap_secs`, and a blank line in each
/// gap between sung lines wider than `gap_secs`, placed a short hold after
/// the line it follows so the last words linger before the sheet moves to
/// the rest. The sheet comes back untouched when it has no timing or
/// both rests are off.
///
/// Relies on [`parse`] handing lines back in time order, which it
/// guarantees: the gap pass measures against a sorted sheet.
pub fn weave_rests(raw: &Arc<Lyrics>, intro: bool, gap: bool, gap_secs: f64) -> Arc<Lyrics> {
    if !raw.synced || (!intro && !gap) {
        return raw.clone();
    }
    let mut lines = Vec::with_capacity(raw.lines.len() + 4);
    let mut prev_timed: Option<f64> = None;
    for line in &raw.lines {
        if let Some(at) = line.at {
            match prev_timed {
                // Before the first sung line: a lead-in rest when the intro
                // runs long enough to earn one.
                None if intro && at > gap_secs => lines.push(rest_line(0.0)),
                // Between two sung lines: a rest a short hold past the first,
                // clamped to before the midpoint so a shorter gap still
                // splits cleanly.
                Some(prev) if gap && at - prev > gap_secs => {
                    let hold = ((at - prev) * 0.5).min(REST_HOLD_SECS);
                    lines.push(rest_line(prev + hold));
                }
                _ => {}
            }
            prev_timed = Some(at);
        }
        lines.push(line.clone());
    }
    Arc::new(Lyrics {
        source: raw.source.clone(),
        text: raw.text.clone(),
        lines,
        synced: raw.synced,
    })
}

/// A blank timed line, which a display shows as a rest and seeks like any
/// other.
pub fn rest_line(at: f64) -> Line {
    Line {
        at: Some(at),
        text: String::new(),
    }
}

/// The last timed line at or before `position`, the one under the
/// playhead. None before the first line's time, so nothing lights up
/// during an intro. Leans on [`parse`]'s time order the same way
/// [`weave_rests`] does: the scan stops at the first line past the
/// playhead.
pub fn active_line(lyrics: &Lyrics, position: f64) -> Option<usize> {
    let mut active = None;
    for (ix, line) in lyrics.lines.iter().enumerate() {
        match line.at {
            Some(at) if at <= position + 0.05 => active = Some(ix),
            Some(_) => break,
            None => {}
        }
    }
    active
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LRC files legally hold their tags in any order, and everything
    /// reading the parsed lines (the panel's playhead scan, the rest
    /// weave) steps through them expecting time order. Two lines sharing a
    /// stamp keep the order the file gave them: a sorted sheet comes back
    /// untouched.
    #[test]
    fn timed_lines_parse_and_sort() {
        let (lines, synced) = parse("[00:12.50]second\n[00:01.00]first\n");
        assert!(synced);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].at, Some(1.0));
        assert_eq!(lines[1].text, "second");
        assert_eq!(lines[1].at, Some(12.5));

        let (lines, _) = parse("[00:30.00]c\n[00:05.00]a\n[00:05.00]b\n[00:20.00]d\n");
        let read: Vec<(Option<f64>, &str)> =
            lines.iter().map(|l| (l.at, l.text.as_str())).collect();
        assert_eq!(
            read,
            [
                (Some(5.0), "a"),
                (Some(5.0), "b"),
                (Some(20.0), "d"),
                (Some(30.0), "c"),
            ]
        );
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
    fn clearing_a_store_sheet_marks_the_track_and_writing_lifts_it() {
        let dir = std::env::temp_dir().join(format!("rox-lyrics-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let track = Path::new("/music/instrumental.flac");
        let target = Source::Store(store_file(&dir, track));

        save(track, &target, "[00:01.00]words", Some(&dir)).unwrap();
        assert!(!marked_none(track, Some(&dir)));
        assert!(load(track, Some(&dir)).is_some());

        // Clearing says the track has none, and it stays said.
        save(track, &target, "", Some(&dir)).unwrap();
        assert!(marked_none(track, Some(&dir)));
        assert!(load(track, Some(&dir)).is_none());

        // Words again take the mark back off.
        save(track, &target, "words", Some(&dir)).unwrap();
        assert!(!marked_none(track, Some(&dir)));
        assert!(load(track, Some(&dir)).is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    /// A wipe has to reach the homes a load never got to. A track holding
    /// both a sidecar and an embedded sheet loads as the sidecar, so
    /// clearing what loaded would leave the tag to surface the moment the
    /// sidecar is gone.
    #[test]
    fn wipe_clears_every_home_including_the_tag() {
        let dir = crate::writer::scratch("lyrics-wipe");
        let track = crate::writer::flac_file(&dir, "track.flac");
        let store = dir.join("store");
        writer::commit(
            &track,
            &[Change {
                field: Field::Lyrics,
                value: Some("embedded words".into()),
            }],
        )
        .unwrap();
        fs::write(track.with_extension("lrc"), "[00:01.00]sidecar words").unwrap();
        save(
            &track,
            &Source::Store(store_file(&store, &track)),
            "stored",
            Some(&store),
        )
        .unwrap();

        // The sidecar is the one that loads, so it's all a clear of the
        // loaded source would have taken.
        let loaded = load(&track, Some(&store)).unwrap();
        assert!(matches!(loaded.source, Source::Sidecar(_)));

        wipe(&track, Some(&store)).unwrap();
        assert!(!track.with_extension("lrc").exists());
        assert!(!store_file(&store, &track).exists());
        assert!(tag_lyrics(&track).is_none());
        assert!(load(&track, Some(&store)).is_none());

        // Nothing left to take, and the audio file is not rewritten for it.
        wipe(&track, Some(&store)).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mark_outranks_a_sidecar() {
        let dir = std::env::temp_dir().join(format!("rox-lyrics-mark-{}", std::process::id()));
        let side = dir.join("track.lrc");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&side, "[00:01.00]words").unwrap();
        let track = dir.join("track.flac");

        assert!(load(&track, Some(&dir)).is_some());
        set_marked_none(&track, &dir, true).unwrap();
        assert!(load(&track, Some(&dir)).is_none());
        set_marked_none(&track, &dir, false).unwrap();
        assert!(load(&track, Some(&dir)).is_some());

        let _ = fs::remove_dir_all(&dir);
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
