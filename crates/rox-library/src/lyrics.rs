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
//! Not every track is a file. A Subsonic song lives on a server and a
//! radio station announces its songs in band, so neither has a sidecar to
//! sit beside or a tag to write into. Both still get words: a [`Subject`]
//! names what a sheet belongs to, and the one with no file behind it keeps
//! its sheet in the app's store under whatever identity its source can
//! promise is stable.
//!
//! The parser is deliberately forgiving. A line's leading `[mm:ss.xx]`
//! groups become timestamps (several on one line repeat the text at each
//! time), an `[offset:ms]` tag shifts them, and the other id tags
//! (`[ar:]`, `[ti:]`, and the like) are dropped. Text with no timestamps
//! at all comes back as plain lines in file order, so an unsynced sheet
//! still reads.
//!
//! Enhanced (A2) sheets time each word as well, with a `<mm:ss.xx>` tag
//! before it and often one closing the line. Those come off the text into
//! [`Line::words`], so a display can run a read head through a line at the
//! speed it was actually sung instead of spreading it evenly, and so the
//! tags never reach the panel as literal text.

use std::fs;
use std::ops::Range;
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

/// What a sheet belongs to. A file on disk has three homes to check and an
/// audio stream to write a tag into; anything else has only the app's own
/// store, so the whole of [`load`], [`save`] and [`wipe`] narrows to that
/// one home for it.
///
/// The remote arm carries an identity string rather than a key, because
/// the two kinds of remote track identify themselves differently. A server
/// song is the same song every time its id comes back, so the id is the
/// identity. A radio station is one URL playing a different song every
/// three minutes, so the identity is the song it announced and not the
/// stream it came down.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    /// A file on disk, at the path it sits at.
    File(PathBuf),
    /// A track with no file behind it, under an identity its source
    /// guarantees is stable. Build these through [`Subject::remote`] and
    /// [`Subject::song`] so the two namespaces can never collide.
    Remote(String),
}

impl Subject {
    /// A track a server holds, under the fragment its key writes: the
    /// source name and the id it gave the song.
    pub fn remote(fragment: &str) -> Subject {
        Subject::Remote(format!("track:{fragment}"))
    }

    /// A song known only by what a station said it was. Case and outer
    /// space are dropped so the same song announced as "Artist - Title"
    /// and "ARTIST -  Title" lands on one sheet, and None when either half
    /// is missing: there is nothing to file words under yet.
    pub fn song(artist: &str, title: &str) -> Option<Subject> {
        let artist = artist.trim();
        let title = title.trim();
        if artist.is_empty() || title.is_empty() {
            return None;
        }
        // Built by hand rather than formatted, so the separator stays a
        // control character no announced title can carry.
        let mut id = String::from("song:");
        id.push_str(&artist.to_lowercase());
        id.push('\u{1}');
        id.push_str(&title.to_lowercase());

        Some(Subject::Remote(id))
    }

    /// The file behind this, for the reads and writes that need one.
    pub fn file(&self) -> Option<&Path> {
        match self {
            Subject::File(path) => Some(path),
            Subject::Remote(_) => None,
        }
    }

    /// The bytes the store hashes a sheet's name out of. A file hashes its
    /// path exactly as it always did, so a store filled before any of this
    /// existed still answers.
    fn ident(&self) -> &[u8] {
        match self {
            Subject::File(path) => path.as_os_str().as_encoded_bytes(),
            Subject::Remote(id) => id.as_bytes(),
        }
    }
}

impl From<PathBuf> for Subject {
    fn from(path: PathBuf) -> Self {
        Subject::File(path)
    }
}

/// One timed word in an enhanced (A2) sheet: when it starts, and where it
/// sits in the line it belongs to. The position is a byte range into
/// [`Line::text`] rather than a copy of the word, so the line stays one
/// string and a display can cut it anywhere, including inside a word, to
/// put a read head partway through.
#[derive(Clone, Debug)]
pub struct Word {
    pub at: f64,
    pub range: Range<usize>,
}

/// One lyric line: its start time in seconds when the source timed it,
/// None when it did not, and the text.
///
/// An enhanced (A2) sheet times each word inside the line as well, which
/// fills [`words`](Line::words) and, when the line closes with a trailing
/// tag, [`end`](Line::end). A plain line-synced sheet leaves both empty
/// and a display falls back to spreading the line across its own span.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub at: Option<f64>,
    pub text: String,
    /// The line's words in text order, empty unless the source timed them.
    pub words: Vec<Word>,
    /// When the last word stops, off a trailing `<mm:ss.xx>` tag. None
    /// leaves a display to guess from the next line's start.
    pub end: Option<f64>,
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
pub fn load(subject: &Subject, store_dir: Option<&Path>) -> Option<Lyrics> {
    if marked_none(subject, store_dir) {
        return None;
    }
    if let Some(path) = subject.file() {
        for side in sidecar_candidates(path) {
            if let Ok(text) = fs::read_to_string(&side)
                && !text.trim().is_empty()
            {
                return Some(build(text, Source::Sidecar(side)));
            }
        }
    }
    if let Some(dir) = store_dir {
        let file = store_file(dir, subject);
        if let Ok(text) = fs::read_to_string(&file)
            && !text.trim().is_empty()
        {
            return Some(build(text, Source::Store(file)));
        }
    }
    // The store is the whole of a remote track's world; there is no file
    // under it to carry a tag.
    Some(build(tag_lyrics(subject.file()?)?, Source::Tag))
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
pub fn wipe(subject: &Subject, store_dir: Option<&Path>) -> Result<(), String> {
    if let Some(path) = subject.file() {
        for side in sidecar_candidates(path) {
            remove_if_present(&side).map_err(|e| format!("remove lyrics file: {e}"))?;
        }
    }
    if let Some(dir) = store_dir {
        remove_if_present(&store_file(dir, subject))
            .map_err(|e| format!("remove lyrics file: {e}"))?;
    }
    if let Some(path) = subject.file()
        && tag_lyrics(path).is_some()
    {
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
    subject: &Subject,
    target: &Source,
    text: &str,
    store_dir: Option<&Path>,
) -> Result<(), String> {
    match target {
        Source::Tag => {
            let path = subject
                .file()
                .ok_or_else(|| "no file to write lyrics into".to_string())?;
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
        Some(dir) => set_marked_none(subject, dir, text.trim().is_empty()),
        None => Ok(()),
    }
}

/// Whether the track is marked as having no lyrics, the state a cleared
/// sheet leaves behind so nothing refills it.
pub fn marked_none(subject: &Subject, store_dir: Option<&Path>) -> bool {
    store_dir.is_some_and(|dir| none_marker(dir, subject).exists())
}

/// Set or lift the "no lyrics" mark. The mark is an empty file beside the
/// store's sheets, so it costs a `stat` to read and persists across
/// restarts without a column of its own.
pub fn set_marked_none(subject: &Subject, store_dir: &Path, on: bool) -> Result<(), String> {
    let file = none_marker(store_dir, subject);
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
    if make_dir && let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create lyrics folder: {e}"))?;
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
pub fn store_file(dir: &Path, subject: &Subject) -> PathBuf {
    store_entry(dir, subject, "lrc")
}

/// The "no lyrics" mark for a track, the store sheet's name under another
/// extension so both stay together and neither can be mistaken for the
/// other.
pub fn none_marker(dir: &Path, subject: &Subject) -> PathBuf {
    store_entry(dir, subject, "none")
}

/// One store entry for a track under `ext`. FNV-1a over the subject's
/// identity, plenty of spread for library-sized sets.
fn store_entry(dir: &Path, subject: &Subject, ext: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in subject.ident() {
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
    format!("[{}]", stamp_body(secs, 2))
}

/// The `mm:ss.xx` inside a time tag, to `decimals` places. Rounded in
/// whole units of the last place before the split into minutes, so a
/// position a hair under the minute rounds to the next minute rather
/// than to sixty seconds.
fn stamp_body(secs: f64, decimals: usize) -> String {
    let unit = 10_u64.pow(decimals as u32);
    let total = (secs.max(0.0) * unit as f64).round() as u64;
    let mins = total / (60 * unit);
    let rest = total % (60 * unit);
    if decimals == 0 {
        format!("{mins:02}:{rest:02}")
    } else {
        format!(
            "{mins:02}:{:02}.{:0width$}",
            rest / unit,
            rest % unit,
            width = decimals
        )
    }
}

/// `text` with every time tag moved by `delta` seconds, the editor's
/// offset nudge. The leading `[mm:ss.xx]` line stamps move, and so do the
/// `<mm:ss.xx>` word stamps of enhanced LRC, so a sheet timed at both
/// levels stays in step with itself. Id tags and the words stay as they
/// were, every other byte of the text included, and a stamp keeps its own
/// precision: a three-decimal sheet doesn't come back rounded to two. No
/// stamp moves before zero.
pub fn shift_stamps(text: &str, delta: f64) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(['[', '<']) {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let close = if rest.starts_with('[') { ']' } else { '>' };
        let Some(end) = rest.find(close) else {
            break;
        };
        let inner = &rest[1..end];
        match parse_time(inner) {
            Some(secs) => {
                // A whole-second stamp still needs the places the nudge
                // moves by, so it grows to two.
                let decimals = inner
                    .split_once('.')
                    .map_or(0, |(_, frac)| frac.trim().len())
                    .max(2);
                out.push_str(&rest[..1]);
                out.push_str(&stamp_body(secs + delta, decimals));
                out.push(close);
            }
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Every stamp in `text` as (row, seconds): the line's index in the text
/// as an editor counts rows, and the time the stamp sounds at, with an
/// `[offset:ms]` tag applied the way [`parse`] applies it. A line
/// carrying several stamps appears once per stamp. Time order, stable,
/// so [`row_at`] can stop at the first stamp past the playhead the way
/// [`active_line`] does, and two rows sharing a time keep their order.
pub fn stamp_rows(text: &str) -> Vec<(usize, f64)> {
    let offset = text.lines().find_map(offset_tag).unwrap_or(0.0) / 1000.0;
    let mut rows: Vec<(usize, f64)> = text
        .lines()
        .enumerate()
        .flat_map(|(row, raw)| {
            scan_times(raw)
                .0
                .into_iter()
                .map(move |at| (row, (at - offset).max(0.0)))
        })
        .collect();
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    rows
}

/// The row under the playhead among [`stamp_rows`]: the row of the last
/// stamp at or before `position`, with the same grace [`active_line`]
/// gives. None before the first stamp, so nothing lights during an intro.
pub fn row_at(rows: &[(usize, f64)], position: f64) -> Option<usize> {
    rows.iter()
        .take_while(|(_, at)| *at <= position + 0.05)
        .last()
        .map(|(row, _)| *row)
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
        // An enhanced (A2) line carries a `<mm:ss.xx>` tag before each
        // word. Pull those out here so the tags never reach a display as
        // literal text, and a plain line just comes back with no words.
        let (body, words, end) = scan_words(&body);

        // Several stamps on one line repeat the same text at each time.
        // The word clock was written for the first of them, so each repeat
        // carries its own copy shifted by how far it sits from that first
        // one. A sheet that only ever stamps a line once, which is nearly
        // all of them, shifts by zero.
        let first = times.first().copied();
        for at in times {
            let shift = at - first.unwrap_or(at);
            timed.push(Line {
                at: Some((at - offset).max(0.0)),
                text: body.clone(),
                words: words
                    .iter()
                    .map(|word| Word {
                        at: (word.at + shift - offset).max(0.0),
                        range: word.range.clone(),
                    })
                    .collect(),
                end: end.map(|end| (end + shift - offset).max(0.0)),
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
            ..Line::default()
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

/// Split an enhanced (A2) line body into its text with the inline
/// `<mm:ss.xx>` tags removed, the words those tags timed, and the time a
/// trailing tag closes the line at.
///
/// Each tag times whatever text follows it up to the next tag, so a word
/// spans from where its tag sat to where the next one does. A line with no
/// tags comes back as itself with no words, which is the plain
/// line-synced case and nearly every sheet in the wild. Brackets with no
/// time in them are left in the text, so a lyric that writes `<3` still
/// reads.
fn scan_words(body: &str) -> (String, Vec<Word>, Option<f64>) {
    // Cheap reject before any allocation: no bracket, nothing to scan.
    if !body.contains('<') {
        return (body.trim_end().to_string(), Vec::new(), None);
    }

    let mut text = String::with_capacity(body.len());
    let mut words: Vec<Word> = Vec::new();
    // The tag that opened the stretch of text being built, and where in
    // the clean text that stretch starts.
    let mut open: Option<(f64, usize)> = None;
    let mut rest = body;

    while let Some(bracket) = rest.find('<') {
        text.push_str(&rest[..bracket]);
        let after = &rest[bracket..];

        let Some(close) = after.find('>') else {
            // An unclosed bracket is just text; take the rest and stop.
            text.push_str(after);
            rest = "";
            break;
        };

        match parse_time(&after[1..close]) {
            Some(at) => {
                push_word(&mut words, open.take(), &text);
                open = Some((at, text.len()));
            }
            // Not a time, so the brackets are part of the lyric.
            None => text.push_str(&after[..=close]),
        }
        rest = &after[close + 1..];
    }
    text.push_str(rest);

    // The last tag has nothing after it to close it. With words behind it
    // it marks where the line stops; with text behind it, it opened the
    // final word like any other.
    let end = match open {
        Some((at, start)) if text[start..].trim().is_empty() => Some(at),
        open => {
            push_word(&mut words, open, &text);
            None
        }
    };
    (text.trim_end().to_string(), words, end)
}

/// Record the word a tag opened, spanning from where the tag sat to the
/// end of the text built since it. The span is trimmed at both ends: a
/// display colors it, and one that swallowed the surrounding spaces would
/// light the gap before the next word's turn. A tag with only whitespace
/// behind it names no word and is dropped.
fn push_word(words: &mut Vec<Word>, open: Option<(f64, usize)>, text: &str) {
    let Some((at, start)) = open else { return };

    let span = &text[start..];
    let from = start + (span.len() - span.trim_start().len());
    let to = start + span.trim_end().len();
    if from >= to {
        return;
    }

    words.push(Word {
        at,
        range: from..to,
    });
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
        ..Line::default()
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

/// How far into `line` the read head has run at `position`, as a byte
/// offset into [`Line::text`] landing on a character boundary. `until` is
/// when the line stops, which the caller takes off the next timed line, and
/// is only consulted when the line doesn't time its own end.
///
/// A word-timed line advances at the speed it was sung: the head sits at
/// the start of the word under the playhead and slides through it over
/// that word's own span, so it can land mid-word the way a karaoke fill
/// does. A line-synced one has nothing finer to go on and spreads the
/// whole text evenly across its span instead, which is a guess but a
/// smooth one.
pub fn read_head(line: &Line, position: f64, until: Option<f64>) -> usize {
    let Some(start) = line.at else {
        return 0;
    };
    // The line's own trailing tag is the honest end; without one, the next
    // line's start is the best available, and with neither the line is the
    // last on the sheet and reads as already run through.
    let end = line.end.or(until);

    if position <= start {
        return 0;
    }

    // A word's span runs to the next word, then to the line's end. With no
    // word under the playhead at all the head is past the last one, which
    // is the whole line.
    let (from, to, opens, closes) = match word_at(line, position) {
        Some(ix) => {
            let word = &line.words[ix];
            let next = line.words.get(ix + 1);
            (
                word.range.start,
                word.range.end,
                word.at,
                next.map(|next| next.at).or(end).unwrap_or(word.at),
            )
        }
        None if !line.words.is_empty() => {
            // Either side of the timed words: nothing lit before the first
            // one starts, the whole line once the last one is done.
            return match line.words.first() {
                Some(first) if position < first.at => 0,
                _ => line.text.len(),
            };
        }
        // Line-synced: the whole text over the whole span.
        None => (
            0,
            line.text.len(),
            start,
            end.unwrap_or(start + LINE_SPAN_SECS),
        ),
    };

    let frac = if closes > opens {
        ((position - opens) / (closes - opens)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let head = from + ((to - from) as f64 * frac).round() as usize;

    // A byte offset in the middle of a multi-byte character would panic a
    // split, so walk back to where that character starts.
    floor_boundary(&line.text, head.min(line.text.len()))
}

/// The word under the playhead: the last one that has started. None
/// before the first word, and on a line with no words at all.
fn word_at(line: &Line, position: f64) -> Option<usize> {
    let mut found = None;
    for (ix, word) in line.words.iter().enumerate() {
        if word.at > position {
            break;
        }
        found = Some(ix);
    }
    // Past the last word's own end, the head has run off the line.
    match (found, line.end) {
        (Some(ix), Some(end)) if ix + 1 == line.words.len() && position > end => None,
        (found, _) => found,
    }
}

/// `at` moved back to the nearest character boundary at or below it, so a
/// split there never lands inside a multi-byte character.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The span a line-synced line is assumed to run for with nothing timed
/// after it, so the last line on a sheet still reads through instead of
/// snapping whole.
const LINE_SPAN_SECS: f64 = 4.0;

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
    fn stamps_round_at_the_last_place_not_at_sixty_seconds() {
        assert_eq!(format_stamp(59.999), "[01:00.00]");
        assert_eq!(format_stamp(119.996), "[02:00.00]");
        assert_eq!(format_stamp(-3.0), "[00:00.00]");
    }

    #[test]
    fn shifting_moves_every_stamp_and_nothing_else() {
        let sheet = "[ti:Song]\r\n[00:10.00][00:20.50]chorus <00:10.20>word\n\n[00:00.10]early\n[01:59.900]late\nplain [Chorus] line\n";
        assert_eq!(
            shift_stamps(sheet, 0.25),
            "[ti:Song]\r\n[00:10.25][00:20.75]chorus <00:10.45>word\n\n[00:00.35]early\n[02:00.150]late\nplain [Chorus] line\n"
        );
        // Back the other way lands where it started, and a stamp can't
        // go negative: the early line pins to zero instead.
        assert_eq!(
            shift_stamps(sheet, -0.25),
            "[ti:Song]\r\n[00:09.75][00:20.25]chorus <00:09.95>word\n\n[00:00.00]early\n[01:59.650]late\nplain [Chorus] line\n"
        );
        // A whole-second stamp gains the places the nudge needs.
        assert_eq!(shift_stamps("[00:05]hi", 0.25), "[00:05.25]hi");
        // An unclosed bracket is text like any other.
        assert_eq!(shift_stamps("[00:05.00]a [b", 1.0), "[00:06.00]a [b");
    }

    /// The editor lights the row under the playhead, so the lookup has to
    /// answer in the text's own line indices: id tags and blank lines
    /// count, a line with two stamps lights at both, an out-of-order
    /// sheet resolves by time, and the offset tag moves the whole sheet.
    #[test]
    fn stamp_rows_keep_editor_line_indices() {
        let sheet = "[offset:500]\n\n[00:10.00][00:30.00]chorus\n[00:20.00]verse\n";
        let rows = stamp_rows(sheet);
        assert_eq!(rows, [(2, 9.5), (3, 19.5), (2, 29.5)]);
        assert_eq!(row_at(&rows, 0.0), None);
        assert_eq!(row_at(&rows, 9.46), Some(2));
        assert_eq!(row_at(&rows, 19.5), Some(3));
        assert_eq!(row_at(&rows, 40.0), Some(2));
        assert!(stamp_rows("no stamps\n[ti:x]").is_empty());
    }

    #[test]
    fn clearing_a_store_sheet_marks_the_track_and_writing_lifts_it() {
        let dir = std::env::temp_dir().join(format!("rox-lyrics-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let track = Subject::File(PathBuf::from("/music/instrumental.flac"));
        let target = Source::Store(store_file(&dir, &track));

        save(&track, &target, "[00:01.00]words", Some(&dir)).unwrap();
        assert!(!marked_none(&track, Some(&dir)));
        assert!(load(&track, Some(&dir)).is_some());

        // Clearing says the track has none, and it stays said.
        save(&track, &target, "", Some(&dir)).unwrap();
        assert!(marked_none(&track, Some(&dir)));
        assert!(load(&track, Some(&dir)).is_none());

        // Words again take the mark back off.
        save(&track, &target, "words", Some(&dir)).unwrap();
        assert!(!marked_none(&track, Some(&dir)));
        assert!(load(&track, Some(&dir)).is_some());

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
        let subject = Subject::File(track.clone());
        save(
            &subject,
            &Source::Store(store_file(&store, &subject)),
            "stored",
            Some(&store),
        )
        .unwrap();

        // The sidecar is the one that loads, so it's all a clear of the
        // loaded source would have taken.
        let loaded = load(&subject, Some(&store)).unwrap();
        assert!(matches!(loaded.source, Source::Sidecar(_)));

        wipe(&subject, Some(&store)).unwrap();
        assert!(!track.with_extension("lrc").exists());
        assert!(!store_file(&store, &subject).exists());
        assert!(tag_lyrics(&track).is_none());
        assert!(load(&subject, Some(&store)).is_none());

        // Nothing left to take, and the audio file is not rewritten for it.
        wipe(&subject, Some(&store)).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mark_outranks_a_sidecar() {
        let dir = std::env::temp_dir().join(format!("rox-lyrics-mark-{}", std::process::id()));
        let side = dir.join("track.lrc");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&side, "[00:01.00]words").unwrap();
        let track = Subject::File(dir.join("track.flac"));

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
        let file = |p: &str| Subject::File(PathBuf::from(p));
        let a = store_file(dir, &file("/music/a.mp3"));
        let b = store_file(dir, &file("/music/b.mp3"));
        assert_eq!(a, store_file(dir, &file("/music/a.mp3")));
        assert_ne!(a, b);
        assert!(a.starts_with(dir));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("lrc"));
    }

    /// The whole point of a song subject: the same song announced by two
    /// stations, in whatever case and spacing each of them uses, lands on
    /// one sheet. A missing half is no song at all, and a server track
    /// files under its own id rather than either.
    #[test]
    fn a_stations_song_files_under_the_song() {
        let dir = Path::new("/data/lyrics");
        let one = Subject::song("Boards of Canada", "Roygbiv").unwrap();
        assert_eq!(
            Subject::song("  BOARDS OF CANADA ", "roygbiv  "),
            Some(one.clone())
        );
        assert_ne!(
            Subject::song("Boards of Canada", "Olson"),
            Some(one.clone())
        );
        assert_eq!(Subject::song("", "Roygbiv"), None);
        assert_eq!(Subject::song("Boards of Canada", " "), None);

        // A song has no file to write a tag or a sidecar into, and its
        // store entry is its own rather than any file's.
        assert!(one.file().is_none());
        assert_ne!(
            store_file(dir, &one),
            store_file(dir, &Subject::remote("subsonic-1|abc"))
        );
    }

    #[test]
    fn plain_text_keeps_lines_untimed() {
        let (lines, synced) = parse("verse one\n\nverse two\n");
        assert!(!synced);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.at.is_none()));
        assert_eq!(lines[1].text, "");
    }

    /// The word tags an enhanced sheet writes inline come off the text
    /// and become a clock; before this they rendered as literal text.
    #[test]
    fn enhanced_line_times_its_words() {
        let (lines, synced) =
            parse("[00:12.00]<00:12.00>Bring <00:12.40>me <00:12.80>to <00:13.10>life<00:15.00>\n");
        assert!(synced);
        assert_eq!(lines.len(), 1);

        let line = &lines[0];
        assert_eq!(line.text, "Bring me to life");
        assert_eq!(line.end, Some(15.0));

        let words: Vec<(&str, f64)> = line
            .words
            .iter()
            .map(|w| (&line.text[w.range.clone()], w.at))
            .collect();
        assert_eq!(
            words,
            vec![("Bring", 12.0), ("me", 12.4), ("to", 12.8), ("life", 13.1)]
        );
    }

    /// A line-synced sheet is the common case and must come through
    /// untouched, words empty so a display knows to spread it evenly.
    #[test]
    fn plain_synced_line_has_no_words() {
        let (lines, _) = parse("[00:12.00]Bring me to life\n");
        assert_eq!(lines[0].text, "Bring me to life");
        assert!(lines[0].words.is_empty());
        assert_eq!(lines[0].end, None);
    }

    /// Angle brackets with no time in them are lyric text, not tags.
    #[test]
    fn non_time_brackets_stay_in_the_text() {
        let (lines, _) = parse("[00:12.00]i <3 you <not a tag>\n");
        assert_eq!(lines[0].text, "i <3 you <not a tag>");
        assert!(lines[0].words.is_empty());
    }

    /// An offset tag shifts the word clock the same way it shifts the
    /// line, or the two would drift apart.
    #[test]
    fn offset_shifts_words_with_the_line() {
        let (lines, _) = parse("[offset:500]\n[00:12.00]<00:12.00>Bring <00:12.40>me<00:13.00>\n");
        assert_eq!(lines[0].at, Some(11.5));
        assert_eq!(lines[0].words[0].at, 11.5);
        assert_eq!(lines[0].words[1].at, 11.9);
        assert_eq!(lines[0].end, Some(12.5));
    }

    /// One line stamped at two times repeats, and the second copy's word
    /// clock has to move with it rather than staying on the first.
    #[test]
    fn repeated_stamp_shifts_its_word_clock() {
        let (lines, _) = parse("[00:10.00][00:30.00]<00:10.00>na <00:10.50>na<00:11.00>\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].words[0].at, 10.0);
        assert_eq!(lines[1].at, Some(30.0));
        assert_eq!(lines[1].words[0].at, 30.0);
        assert_eq!(lines[1].words[1].at, 30.5);
        assert_eq!(lines[1].end, Some(31.0));
    }

    /// The read head walks a word-timed line at the speed it was sung,
    /// landing inside the word under the playhead.
    #[test]
    fn read_head_slides_through_the_sung_word() {
        let (lines, _) =
            parse("[00:12.00]<00:12.00>Bring <00:12.40>me <00:12.80>to <00:13.10>life<00:15.00>\n");
        let line = &lines[0];
        let head = |at| &line.text[..read_head(line, at, None)];

        assert_eq!(head(11.0), "");
        assert_eq!(head(12.0), "");
        // Partway through "Bring", which spans 12.0 to 12.4. Where exactly
        // is float arithmetic; that it cuts inside the word at all is the
        // thing, since that is what a karaoke fill looks like.
        assert!(head(12.2).starts_with("Br"));
        assert!(head(12.2).len() < "Bring".len());
        // On a word's own stamp the head sits at its first character, so
        // the space behind it reads as sung.
        assert_eq!(head(12.4), "Bring ");
        assert_eq!(head(12.8), "Bring me ");
        assert_eq!(head(13.1), "Bring me to ");
        // Past the line's own end, every word is behind the head.
        assert_eq!(head(20.0), "Bring me to life");
    }

    /// With no word clock the head still moves, spread evenly across the
    /// span the next line closes.
    #[test]
    fn read_head_spreads_a_line_synced_line() {
        let (lines, _) = parse("[00:10.00]abcd\n[00:20.00]next\n");
        let line = &lines[0];

        assert_eq!(read_head(line, 10.0, Some(20.0)), 0);
        assert_eq!(read_head(line, 15.0, Some(20.0)), 2);
        assert_eq!(read_head(line, 20.0, Some(20.0)), 4);
    }

    /// A head landing mid-character would panic the split that renders it.
    #[test]
    fn read_head_lands_on_a_character_boundary() {
        let (lines, _) = parse("[00:10.00]\u{3042}\u{3044}\u{3046}\n");
        let line = &lines[0];
        for step in 0..=40 {
            let at = 10.0 + f64::from(step) * 0.1;
            let head = read_head(line, at, Some(14.0));
            assert!(line.text.is_char_boundary(head), "cut at {head} at {at}");
        }
    }
}
