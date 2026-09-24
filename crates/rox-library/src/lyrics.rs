//! Lyrics for a track: find them, parse LRC, save edits back. Three homes
//! (a sidecar, the app's store, the embedded tag), and an edit saves to the
//! home it loaded from. Blocking.
//!
//! A "no lyrics" mark in the store outranks every home, so a cleared sheet
//! isn't refilled by the next automatic lookup.
//!
//! A track without a file (a server song, a station's announced song) keeps
//! its sheet in the store under a [`Subject`] identity.
//!
//! The parser is forgiving: leading `[mm:ss.xx]` groups are timestamps, an
//! `[offset:ms]` tag shifts them, other id tags drop, and an unsynced sheet
//! reads as plain lines. Enhanced (A2) `<mm:ss.xx>` word tags become
//! [`Line::words`].

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::writer::{self, Change, Field};

/// Timed format first. Each is tried as a stem swap (track.lrc) and appended
/// (track.mp3.lrc), the two conventions in the wild.
const SIDECAR_EXTS: [&str; 2] = ["lrc", "txt"];

/// Remembered so an edit saves back to the same place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// USLT on ID3v2, UNSYNCEDLYRICS on Vorbis.
    Tag,
    Sidecar(PathBuf),
    Store(PathBuf),
}

/// What a sheet belongs to. Anything without a file has only the store.
/// A server song's identity is its id; a station's is the song it announced,
/// not the stream.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    File(PathBuf),
    /// Build through [`Subject::remote`] and [`Subject::song`] so the two
    /// namespaces never collide.
    Remote(String),
}

impl Subject {
    pub fn remote(fragment: &str) -> Subject {
        Subject::Remote(format!("track:{fragment}"))
    }

    /// Case and outer space dropped, so one song announced two ways lands on one
    /// sheet. None when either half is missing.
    pub fn song(artist: &str, title: &str) -> Option<Subject> {
        let artist = artist.trim();
        let title = title.trim();
        if artist.is_empty() || title.is_empty() {
            return None;
        }
        // The separator is a control character no announced title can carry.
        let mut id = String::from("song:");
        id.push_str(&artist.to_lowercase());
        id.push('\u{1}');
        id.push_str(&title.to_lowercase());

        Some(Subject::Remote(id))
    }

    pub fn file(&self) -> Option<&Path> {
        match self {
            Subject::File(path) => Some(path),
            Subject::Remote(_) => None,
        }
    }

    /// A file hashes its bare path, which keeps pre-remote store entries valid.
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

/// A byte range into [`Line::text`], so a display can cut mid-word.
#[derive(Clone, Debug)]
pub struct Word {
    pub at: f64,
    pub range: Range<usize>,
}

/// `at` is None for an unsynced line. `words` and `end` are empty unless the
/// sheet is enhanced (A2).
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub at: Option<f64>,
    pub text: String,
    pub words: Vec<Word>,
    /// Off a trailing `<mm:ss.xx>` tag.
    pub end: Option<f64>,
}

pub struct Lyrics {
    pub source: Source,
    pub text: String,
    pub lines: Vec<Line>,
    /// At least one line has a timestamp.
    pub synced: bool,
}

/// Sidecar, then store, then tag. A sidecar wins: a file placed beside the
/// track is a stronger signal than what the app filled in. The "no lyrics"
/// mark overrides all three.
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
    Some(build(tag_lyrics(subject.file()?)?, Source::Tag))
}

/// A blank frame counts as missing.
pub(crate) fn tag_lyrics(path: &Path) -> Option<String> {
    writer::read(path)
        .ok()?
        .into_iter()
        .find(|(field, _)| *field == Field::Lyrics)
        .map(|(_, value)| value)
        .filter(|text| !text.trim().is_empty())
}

/// Clear every home at once, since [`save`] touches one and the others would
/// surface. The tag is only rewritten when it has words. The mark is the
/// caller's: see [`set_marked_none`].
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

fn remove_if_present(file: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Saving nothing marks the track as having no lyrics; saving words lifts
/// the mark.
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
    // Only after the write succeeded.
    match store_dir {
        Some(dir) => set_marked_none(subject, dir, text.trim().is_empty()),
        None => Ok(()),
    }
}

pub fn marked_none(subject: &Subject, store_dir: Option<&Path>) -> bool {
    store_dir.is_some_and(|dir| none_marker(dir, subject).exists())
}

/// The mark is an empty file beside the store's sheets.
pub fn set_marked_none(subject: &Subject, store_dir: &Path, on: bool) -> Result<(), String> {
    let file = none_marker(store_dir, subject);
    if !on {
        return remove_if_present(&file).map_err(|e| format!("clear lyrics mark: {e}"));
    }
    fs::create_dir_all(store_dir).map_err(|e| format!("create lyrics folder: {e}"))?;
    fs::write(&file, []).map_err(|e| format!("write lyrics mark: {e}"))
}

/// The store's folder is created on first save.
fn save_file(file: &Path, text: &str, make_dir: bool) -> Result<(), String> {
    if text.trim().is_empty() {
        return remove_if_present(file).map_err(|e| format!("remove lyrics file: {e}"));
    }
    if make_dir && let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create lyrics folder: {e}"))?;
    }
    // Clone and rename, so a crash never truncates the sheet.
    let tmp = writer::tmp_path(file);
    fs::write(&tmp, text).map_err(|e| format!("write lyrics file: {e}"))?;
    fs::rename(&tmp, file).map_err(|e| format!("rename lyrics file: {e}"))
}

/// One flat folder named by a hash of the subject, so no library folder
/// shape is mirrored.
pub fn store_file(dir: &Path, subject: &Subject) -> PathBuf {
    store_entry(dir, subject, "lrc")
}

/// The store sheet's name under another extension.
pub fn none_marker(dir: &Path, subject: &Subject) -> PathBuf {
    store_entry(dir, subject, "none")
}

fn store_entry(dir: &Path, subject: &Subject, ext: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in subject.ident() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{hash:016x}.{ext}"))
}

pub fn default_sidecar(path: &Path) -> PathBuf {
    path.with_extension("lrc")
}

pub fn format_stamp(secs: f64) -> String {
    format!("[{}]", stamp_body(secs, 2))
}

/// Rounded in whole units of the last place before splitting off minutes,
/// so 59.999 becomes 01:00.00, never 00:60.00.
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

/// Move every `[..]` line stamp and `<..>` word stamp by `delta`, keeping each
/// stamp's own precision and every other byte. Nothing goes below zero.
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
                // A whole-second stamp grows to two places so the nudge fits.
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

/// Every stamp as (editor row, seconds), offset applied, in stable time
/// order. A line with several stamps appears once per stamp.
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

/// None before the first stamp. Same grace as [`active_line`].
pub fn row_at(rows: &[(usize, f64)], position: f64) -> Option<usize> {
    rows.iter()
        .take_while(|(_, at)| *at <= position + 0.05)
        .last()
        .map(|(row, _)| *row)
}

/// A leading id tag stops the strip.
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

/// Public so a rename can move each sidecar convention to its matching new
/// name, `.mp3.lrc` to `.flac.lrc`.
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

pub fn parse(text: &str) -> (Vec<Line>, bool) {
    // The offset tag can appear anywhere. Positive means the lyrics run early.
    let offset = text.lines().find_map(offset_tag).unwrap_or(0.0) / 1000.0;

    let mut timed = Vec::new();
    for raw in text.lines() {
        let (times, body) = scan_times(raw);
        let (body, words, end) = scan_words(&body);

        // A repeated stamp's word clock shifts by its distance from the first.
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

    // No timestamps: plain lines in file order, blanks kept for verse spacing.
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

/// Id tags among the groups are dropped.
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

/// Each `<mm:ss.xx>` tag times the text up to the next. A bracket with no
/// time stays in the text, so `<3` still reads.
fn scan_words(body: &str) -> (String, Vec<Word>, Option<f64>) {
    if !body.contains('<') {
        return (body.trim_end().to_string(), Vec::new(), None);
    }

    let mut text = String::with_capacity(body.len());
    let mut words: Vec<Word> = Vec::new();
    let mut open: Option<(f64, usize)> = None;
    let mut rest = body;

    while let Some(bracket) = rest.find('<') {
        text.push_str(&rest[..bracket]);
        let after = &rest[bracket..];

        let Some(close) = after.find('>') else {
            text.push_str(after);
            rest = "";
            break;
        };

        match parse_time(&after[1..close]) {
            Some(at) => {
                push_word(&mut words, open.take(), &text);
                open = Some((at, text.len()));
            }
            None => text.push_str(&after[..=close]),
        }
        rest = &after[close + 1..];
    }
    text.push_str(rest);

    // A last tag with only whitespace after it closes the line; with text after
    // it, it opened the final word.
    let end = match open {
        Some((at, start)) if text[start..].trim().is_empty() => Some(at),
        open => {
            push_word(&mut words, open, &text);
            None
        }
    };
    (text.trim_end().to_string(), words, end)
}

/// Trimmed at both ends so a display never lights the gap before the next
/// word. A tag over whitespace alone names no word.
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

fn parse_time(inner: &str) -> Option<f64> {
    let (mins, secs) = inner.split_once(':')?;
    let mins: f64 = mins.trim().parse().ok()?;
    let secs: f64 = secs.trim().parse().ok()?;
    (mins >= 0.0 && (0.0..60.0).contains(&secs)).then_some(mins * 60.0 + secs)
}

fn offset_tag(line: &str) -> Option<f64> {
    let inner = line.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once(':')?;
    key.trim()
        .eq_ignore_ascii_case("offset")
        .then(|| value.trim().parse().ok())
        .flatten()
}

/// How long a rest waits past the line it follows, so the last words linger.
const REST_HOLD_SECS: f64 = 4.0;

/// A lead-in rest before a late first line, and a rest in each gap wider
/// than `gap_secs`. Relies on [`parse`]'s time order.
pub fn weave_rests(raw: &Arc<Lyrics>, intro: bool, gap: bool, gap_secs: f64) -> Arc<Lyrics> {
    if !raw.synced || (!intro && !gap) {
        return raw.clone();
    }
    let mut lines = Vec::with_capacity(raw.lines.len() + 4);
    let mut prev_timed: Option<f64> = None;
    for line in &raw.lines {
        if let Some(at) = line.at {
            match prev_timed {
                None if intro && at > gap_secs => lines.push(rest_line(0.0)),
                // Clamped before the midpoint so a short gap still splits cleanly.
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

pub fn rest_line(at: f64) -> Line {
    Line {
        at: Some(at),
        text: String::new(),
        ..Line::default()
    }
}

/// None before the first line. Relies on [`parse`]'s time order.
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

/// A byte offset into [`Line::text`] on a char boundary. A word-timed line
/// slides through each word over its own span, mid-word included; a
/// line-synced one spreads evenly. `until` is the next line's start, used
/// when the line doesn't time its own end.
pub fn read_head(line: &Line, position: f64, until: Option<f64>) -> usize {
    let Some(start) = line.at else {
        return 0;
    };
    // With no end at all the line is the sheet's last and reads as done.
    let end = line.end.or(until);

    if position <= start {
        return 0;
    }

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
            return match line.words.first() {
                Some(first) if position < first.at => 0,
                _ => line.text.len(),
            };
        }
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

    // Walk back so a split never lands mid-character.
    floor_boundary(&line.text, head.min(line.text.len()))
}

fn word_at(line: &Line, position: f64) -> Option<usize> {
    let mut found = None;
    for (ix, word) in line.words.iter().enumerate() {
        if word.at > position {
            break;
        }
        found = Some(ix);
    }
    match (found, line.end) {
        (Some(ix), Some(end)) if ix + 1 == line.words.len() && position > end => None,
        (found, _) => found,
    }
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Assumed span of a last line-synced line, so it reads through rather than
/// snapping whole.
const LINE_SPAN_SECS: f64 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// LRC tags come in any order, and every reader expects time order. Equal
    /// stamps keep file order.
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
        assert_eq!(lines[0].at, Some(9.5));
    }

    #[test]
    fn stamp_formats_and_strips_round_trip() {
        assert_eq!(format_stamp(83.5), "[01:23.50]");
        assert_eq!(format_stamp(0.0), "[00:00.00]");
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
        assert_eq!(
            shift_stamps(sheet, -0.25),
            "[ti:Song]\r\n[00:09.75][00:20.25]chorus <00:09.95>word\n\n[00:00.00]early\n[01:59.650]late\nplain [Chorus] line\n"
        );
        assert_eq!(shift_stamps("[00:05]hi", 0.25), "[00:05.25]hi");
        assert_eq!(shift_stamps("[00:05.00]a [b", 1.0), "[00:06.00]a [b");
    }

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

        save(&track, &target, "", Some(&dir)).unwrap();
        assert!(marked_none(&track, Some(&dir)));
        assert!(load(&track, Some(&dir)).is_none());

        save(&track, &target, "words", Some(&dir)).unwrap();
        assert!(!marked_none(&track, Some(&dir)));
        assert!(load(&track, Some(&dir)).is_some());

        let _ = fs::remove_dir_all(&dir);
    }

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

        let loaded = load(&subject, Some(&store)).unwrap();
        assert!(matches!(loaded.source, Source::Sidecar(_)));

        wipe(&subject, Some(&store)).unwrap();
        assert!(!track.with_extension("lrc").exists());
        assert!(!store_file(&store, &subject).exists());
        assert!(tag_lyrics(&track).is_none());
        assert!(load(&subject, Some(&store)).is_none());

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

    #[test]
    fn plain_synced_line_has_no_words() {
        let (lines, _) = parse("[00:12.00]Bring me to life\n");
        assert_eq!(lines[0].text, "Bring me to life");
        assert!(lines[0].words.is_empty());
        assert_eq!(lines[0].end, None);
    }

    #[test]
    fn non_time_brackets_stay_in_the_text() {
        let (lines, _) = parse("[00:12.00]i <3 you <not a tag>\n");
        assert_eq!(lines[0].text, "i <3 you <not a tag>");
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn offset_shifts_words_with_the_line() {
        let (lines, _) = parse("[offset:500]\n[00:12.00]<00:12.00>Bring <00:12.40>me<00:13.00>\n");
        assert_eq!(lines[0].at, Some(11.5));
        assert_eq!(lines[0].words[0].at, 11.5);
        assert_eq!(lines[0].words[1].at, 11.9);
        assert_eq!(lines[0].end, Some(12.5));
    }

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

    #[test]
    fn read_head_slides_through_the_sung_word() {
        let (lines, _) =
            parse("[00:12.00]<00:12.00>Bring <00:12.40>me <00:12.80>to <00:13.10>life<00:15.00>\n");
        let line = &lines[0];
        let head = |at| &line.text[..read_head(line, at, None)];

        assert_eq!(head(11.0), "");
        assert_eq!(head(12.0), "");
        // Where exactly is float arithmetic; cutting inside the word is the point.
        assert!(head(12.2).starts_with("Br"));
        assert!(head(12.2).len() < "Bring".len());
        assert_eq!(head(12.4), "Bring ");
        assert_eq!(head(12.8), "Bring me ");
        assert_eq!(head(13.1), "Bring me to ");
        assert_eq!(head(20.0), "Bring me to life");
    }

    #[test]
    fn read_head_spreads_a_line_synced_line() {
        let (lines, _) = parse("[00:10.00]abcd\n[00:20.00]next\n");
        let line = &lines[0];

        assert_eq!(read_head(line, 10.0, Some(20.0)), 0);
        assert_eq!(read_head(line, 15.0, Some(20.0)), 2);
        assert_eq!(read_head(line, 20.0, Some(20.0)), 4);
    }

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
