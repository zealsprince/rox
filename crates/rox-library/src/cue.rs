//! CUE sheet support: the parser and the two types that describe a cue track
//! through the rest of the app. A cue rip is one image file (a whole-disc
//! FLAC or WAV) split into tracks by timestamps in a sidecar .cue sheet, so
//! a track stops being a file and becomes a span inside one. Identity per
//! the subsong model: a track is (source, path, sub), where sub is 0 for a
//! plain file and the 1-based cue track number for a span, and the source
//! is "local" for everything that came off disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

/// A cue track's slice of its image file, in milliseconds from the start.
/// `end_ms` is None on the last track of an image, which runs to the end of
/// the file; the store keeps that as NULL so the boundary follows the file
/// rather than a duration measured at scan time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start_ms: u32,
    pub end_ms: Option<u32>,
}

impl Span {
    /// The span's length where it has one; the last track of an image
    /// answers None and the caller falls back to the file's own end.
    pub fn len_ms(&self) -> Option<u32> {
        self.end_ms.map(|end| end.saturating_sub(self.start_ms))
    }
}

/// Which source a track belongs to: "local" for a file on disk, whatever a
/// source names itself otherwise ("subsonic:<server-id>", "radio"). An
/// `Arc<str>` rather than a `String` because a key is cloned per queue entry
/// and per queue snapshot while the value repeats across every row of a
/// library, so the clone should be a pointer bump.
pub type SourceId = Arc<str>;

/// The source every track had before sources existed: a file on disk.
pub const LOCAL: &str = "local";

/// The one shared allocation behind every local key. Handing out clones of
/// it keeps the overwhelmingly common case off the allocator.
pub fn local() -> SourceId {
    static LOCAL_ID: OnceLock<SourceId> = OnceLock::new();

    LOCAL_ID.get_or_init(|| Arc::from(LOCAL)).clone()
}

/// A source string as a [`SourceId`], sharing the one local allocation for
/// the case that's almost every row. Anything reading a source off a
/// database row or a projection row goes through this rather than
/// `Arc::from`, so a library of a million local tracks holds one.
pub fn source_id(source: &str) -> SourceId {
    if source == LOCAL {
        local()
    } else {
        Arc::from(source)
    }
}

/// What each source is called where a person reads it: "Local", a
/// server's own name or host, "Radio". The source string is a storage key,
/// and a Subsonic one is a digest nobody would recognize, so every surface
/// that shows a source or matches one typed by hand asks here.
///
/// The names live in settings, which this crate sits below, so the layer
/// that reads them fills the table: the catalog, each time it loads a
/// projection. Held process-wide rather than on the projection because
/// the queue, the history and the playlists match `source:` over rows the
/// projection never sees.
static SOURCE_LABELS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// Replace the whole name table.
pub fn set_source_labels(labels: HashMap<String, String>) {
    if let Ok(mut table) = SOURCE_LABELS.write() {
        *table = Some(labels);
    }
}

/// What a source is called: its name from the table, or the source string
/// itself for one the table doesn't know, which is still better than
/// nothing and is what a source added before the next load shows.
pub fn source_label(source: &str) -> String {
    SOURCE_LABELS
        .read()
        .ok()
        .and_then(|table| table.as_ref()?.get(source).cloned())
        .unwrap_or_else(|| source.to_string())
}

/// The prefix every Subsonic source string carries, one source per
/// configured server with a digest of its URL and user behind the colon.
/// Written out here rather than imported because the format lives in
/// rox-net, which this crate sits below.
pub const SUBSONIC_PREFIX: &str = "subsonic:";

/// Which kind of source a track came from, as the UI branches on it. The
/// source string is the storage form and there are as many of them as
/// there are configured servers; this is the three cases a surface
/// actually draws differently. A station has no timeline and gets its
/// song title off the stream, a server-backed track has no file behind
/// it, and a local file is everything the app did before sources.
///
/// Anything the match doesn't recognize reads as [`Origin::Subsonic`]
/// would be a lie, so it reads as local: a future source added without
/// touching this still draws like a plain track rather than borrowing a
/// station's live handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    Local,
    Subsonic,
    Radio,
}

impl Origin {
    /// Read a source string. Takes the string rather than a key so a
    /// projection row, which carries its source and no key, can ask too.
    pub fn of(source: &str) -> Origin {
        if source == crate::stations::SOURCE {
            Origin::Radio
        } else if source.starts_with(SUBSONIC_PREFIX) {
            Origin::Subsonic
        } else {
            Origin::Local
        }
    }
}

/// What a play request points at: a source, something within it, and which
/// subsong of that. Plain files are sub 0; cue tracks use their 1-based
/// track number. This is the currency the player and panels trade in where
/// a bare PathBuf used to do, so two tracks of the same image stay distinct
/// in a queue.
///
/// The source is part of the key rather than a lookup off it, because
/// identity repeats across sources: a Subsonic server's song id and a path
/// on disk can be the same string, and two servers can hand back the same
/// id for different music.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrackKey {
    pub source: SourceId,
    pub path: PathBuf,
    pub sub: u16,
}

impl From<PathBuf> for TrackKey {
    fn from(path: PathBuf) -> Self {
        TrackKey {
            source: local(),
            path,
            sub: 0,
        }
    }
}

impl TrackKey {
    /// True for a track that lives on disk, which is still most of them.
    pub fn is_local(&self) -> bool {
        &*self.source == LOCAL
    }

    /// Which kind of source this track came from, for the surfaces that
    /// draw one differently.
    pub fn origin(&self) -> Origin {
        Origin::of(&self.source)
    }

    /// The string form for stores that only hold text (m3u exports): the
    /// bare path for a plain file, `path#N` for a cue track. Readers try
    /// the string as a literal path first, so a real file whose name ends
    /// in `#2` still resolves to itself ahead of the fragment reading.
    ///
    /// A local key writes exactly what it always wrote, so an m3u exported
    /// before sources and one exported after are the same file. Any other
    /// source prefixes `source|`.
    pub fn to_fragment(&self) -> String {
        let path = self.path.display();

        let body = if self.sub == 0 {
            path.to_string()
        } else {
            format!("{path}#{}", self.sub)
        };

        if self.is_local() {
            body
        } else {
            format!("{}|{body}", self.source)
        }
    }

    /// Read a fragment string back, `exists` deciding whether the literal
    /// reading wins: handed a callback that checks the store (or the disk),
    /// a name that really ends in `#2` beats the cue reading of it. The
    /// source prefix is read the same way round, since `|` is a legal
    /// character in a file name and a path holding one has to stay whole.
    pub fn from_fragment(s: &str, exists: impl Fn(&str) -> bool) -> TrackKey {
        if !exists(s)
            && let Some((source, rest)) = s.split_once('|')
            && !source.is_empty()
            && source != LOCAL
        {
            // Past the prefix there's no on-disk reading left to lose to, so
            // a `#N` suffix here means what it says.
            let (path, sub) = match rest.rsplit_once('#') {
                Some((path, sub)) => match sub.parse::<u16>() {
                    Ok(sub) if sub > 0 => (path, sub),

                    _ => (rest, 0),
                },

                None => (rest, 0),
            };

            return TrackKey {
                source: Arc::from(source),
                path: PathBuf::from(path),
                sub,
            };
        }

        if !exists(s)
            && let Some((path, sub)) = s.rsplit_once('#')
            && let Ok(sub) = sub.parse::<u16>()
            && sub > 0
            && exists(path)
        {
            return TrackKey {
                source: local(),
                path: PathBuf::from(path),
                sub,
            };
        }

        TrackKey {
            source: local(),
            path: PathBuf::from(s),
            sub: 0,
        }
    }
}

/// A parsed .cue sheet: the album-level tags, then the image files it splits.
/// Most sheets name exactly one file, but a per-track rip with a cue on top
/// (one FILE per song) is legal and shows up in the wild, so files is a list
/// and track spans never cross a file boundary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CueSheet {
    pub title: String,
    pub performer: String,
    pub genre: String,
    pub year: u16,
    pub files: Vec<CueFile>,
}

/// One image file and the tracks cut out of it. `path` is the FILE argument
/// exactly as the sheet wrote it, relative names and all; resolving that
/// against the sheet's own directory is the scanner's call, not the parser's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CueFile {
    pub path: String,
    pub tracks: Vec<CueTrack>,
}

/// A single cue track. `number` is what the sheet said rather than the
/// position in the list, since that number is the `sub` half of a TrackKey
/// and has to stay stable when data tracks are skipped out of the middle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueTrack {
    pub number: u16,
    pub title: String,
    pub performer: String,
    pub span: Span,
}

/// The cp1252 mapping for 0x80 to 0x9F, the one stretch where Windows-1252
/// and Latin-1 disagree. Everything below 0x80 is ASCII and everything from
/// 0xA0 up matches Latin-1, which is a straight cast to char. The five slots
/// cp1252 leaves undefined (0x81, 0x8D, 0x8F, 0x90, 0x9D) answer U+FFFD, so
/// a byte that was never text stays visibly wrong instead of quietly
/// borrowing a C1 control's identity.
const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}', '\u{017d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
];

/// Get text out of a cue file's bytes. Sheets have no encoding declaration
/// and predate UTF-8 by a decade, so the rule is UTF-8 first (with the BOM
/// that Windows editors like to leave behind stripped) and Windows-1252 as
/// the fallback when that fails. cp1252 can't fail, every byte maps to
/// something, which is exactly why it only runs second: guess it too early
/// and a real UTF-8 sheet turns into mojibake.
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => CP1252_HIGH[(b - 0x80) as usize],
                other => other as char,
            })
            .collect(),
    }
}

/// Peel the leading bare word off a line, handing back the word and what
/// follows it. Leading whitespace goes, which makes the indentation every
/// cue sheet uses under TRACK a non-issue.
fn split_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    match s.find(char::is_whitespace) {
        Some(at) => Some((&s[..at], &s[at..])),
        None => Some((s, "")),
    }
}

/// Read one command argument. Quoted arguments run to the closing quote and
/// may hold spaces, bare ones stop at the next whitespace. An unterminated
/// quote takes the rest of the line instead of dropping the value, since a
/// truncated title still beats no title.
fn read_arg(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        return Some(match rest.find('"') {
            Some(at) => (rest[..at].to_string(), &rest[at + 1..]),
            None => (rest.trim_end().to_string(), ""),
        });
    }
    split_token(s).map(|(word, rest)| (word.to_string(), rest))
}

/// `mm:ss:ff` to milliseconds. Frames are 1/75 of a second on a CD, and the
/// division truncates, so 37 frames is 493ms rather than 493.33. Minutes
/// aren't capped at 59 because a single-file rip counts straight through the
/// disc, and a missing frame field is read as zero.
fn parse_time(s: &str) -> Option<u32> {
    let mut parts = s.split(':');
    let minutes: u64 = parts.next()?.trim().parse().ok()?;
    let seconds: u64 = parts.next()?.trim().parse().ok()?;
    let frames: u64 = parts
        .next()
        .and_then(|f| f.trim().parse().ok())
        .unwrap_or(0);
    let ms = (minutes * 60 + seconds) * 1000 + frames * 1000 / 75;
    Some(ms.min(u32::MAX as u64) as u32)
}

/// The first standalone four-digit run in a REM DATE value. Sheets write the
/// year as `1997`, `1997-05-01`, or the odd `05/1997`, and looking for a run
/// of exactly four digits picks the year out of all three without pretending
/// to parse a date.
fn first_year(s: &str) -> u16 {
    s.split(|c: char| !c.is_ascii_digit())
        .find(|group| group.len() == 4)
        .and_then(|group| group.parse().ok())
        .unwrap_or(0)
}

/// A track being filled in as its lines go by. Both indexes are kept because
/// INDEX 01 is the real start and INDEX 00 is the pregap, and a sheet that
/// only ever writes INDEX 00 still has to yield a usable start.
struct Pending {
    number: u16,
    title: String,
    performer: String,
    index00: Option<u32>,
    index01: Option<u32>,
}

/// Where the parser is relative to a TRACK command. The Skipped arm matters
/// as much as the other two: a data track's TITLE has to go nowhere, and
/// without a state for it that title would fall through to the album.
enum TrackState {
    Album,
    Skipped,
    Audio(Pending),
}

/// Close out the track being built and hang it on the current file. A track
/// with no INDEX at all is dropped, as is one that arrived before any FILE
/// line, because neither has a span anything could play.
fn flush_track(state: &mut TrackState, file: &mut Option<CueFile>) {
    let TrackState::Audio(pending) = std::mem::replace(state, TrackState::Album) else {
        return;
    };
    let (Some(start_ms), Some(file)) = (pending.index01.or(pending.index00), file.as_mut()) else {
        return;
    };
    file.tracks.push(CueTrack {
        number: pending.number,
        title: pending.title,
        performer: pending.performer,
        span: Span {
            start_ms,
            end_ms: None,
        },
    });
}

/// Read a .cue sheet. Answers None when nothing playable came out of it, so
/// a sheet that's all data tracks, or one that isn't a cue sheet at all,
/// reads the same as an unparseable one to the caller. Anything it doesn't
/// recognise (CATALOG, ISRC, FLAGS, SONGWRITER, PREGAP, plain junk) is
/// skipped rather than treated as an error, since half the sheets in a real
/// library have some ripper's private line.
pub fn parse(bytes: &[u8]) -> Option<CueSheet> {
    let text = decode(bytes);
    let mut sheet = CueSheet::default();
    let mut current: Option<CueFile> = None;
    let mut state = TrackState::Album;

    for line in text.lines() {
        let Some((command, rest)) = split_token(line) else {
            continue;
        };
        match command.to_ascii_uppercase().as_str() {
            "FILE" => {
                flush_track(&mut state, &mut current);
                if let Some(done) = current.take() {
                    sheet.files.push(done);
                }
                // The trailing WAVE/MP3/BINARY word is noise: the decoder
                // reads the real format off the file, and sheets lie here.
                if let Some((path, _)) = read_arg(rest) {
                    current = Some(CueFile {
                        path,
                        tracks: Vec::new(),
                    });
                }
            }
            "TRACK" => {
                flush_track(&mut state, &mut current);
                let number = read_arg(rest).and_then(|(n, tail)| {
                    let kind = read_arg(tail).map(|(k, _)| k).unwrap_or_default();
                    n.parse::<u16>()
                        .ok()
                        .filter(|_| kind.eq_ignore_ascii_case("AUDIO"))
                });
                state = match number {
                    Some(number) => TrackState::Audio(Pending {
                        number,
                        title: String::new(),
                        performer: String::new(),
                        index00: None,
                        index01: None,
                    }),
                    // A data track, or a TRACK line we couldn't read. Either
                    // way the whole block including its indexes goes.
                    None => TrackState::Skipped,
                };
            }
            "TITLE" => {
                let value = read_arg(rest).map(|(v, _)| v).unwrap_or_default();
                match &mut state {
                    TrackState::Audio(pending) => pending.title = value,
                    TrackState::Album => sheet.title = value,
                    TrackState::Skipped => {}
                }
            }
            "PERFORMER" => {
                let value = read_arg(rest).map(|(v, _)| v).unwrap_or_default();
                match &mut state {
                    TrackState::Audio(pending) => pending.performer = value,
                    TrackState::Album => sheet.performer = value,
                    TrackState::Skipped => {}
                }
            }
            "INDEX" => {
                if let TrackState::Audio(pending) = &mut state
                    && let Some((number, tail)) = read_arg(rest)
                {
                    let at = read_arg(tail).and_then(|(time, _)| parse_time(&time));
                    match (number.trim().parse::<u8>().ok(), at) {
                        (Some(0), Some(at)) => pending.index00 = Some(at),
                        (Some(1), Some(at)) => pending.index01 = Some(at),
                        // INDEX 02 and up are intra-track markers nothing
                        // here plays from, so they're dropped.
                        _ => {}
                    }
                }
            }
            "REM" => {
                // REM is the escape hatch rippers hang their own tags off.
                // Only genre and date mean anything to us; the rest, COMMENT
                // included, is a comment and is treated like one.
                let Some((keyword, tail)) = split_token(rest) else {
                    continue;
                };
                let value = read_arg(tail).map(|(v, _)| v).unwrap_or_default();
                if keyword.eq_ignore_ascii_case("GENRE") {
                    sheet.genre = value;
                } else if keyword.eq_ignore_ascii_case("DATE") {
                    sheet.year = first_year(&value);
                }
            }
            _ => {}
        }
    }

    flush_track(&mut state, &mut current);
    if let Some(done) = current.take() {
        sheet.files.push(done);
    }

    for file in &mut sheet.files {
        // A track runs until the next one starts, which is why the ends get
        // filled in here rather than as the tracks are read. The last track
        // of every file keeps None: it runs to the end of its image, and
        // that boundary belongs to the file, not the sheet.
        let starts: Vec<u32> = file.tracks.iter().map(|t| t.span.start_ms).collect();
        for (i, track) in file.tracks.iter_mut().enumerate() {
            track.span.end_ms = starts.get(i + 1).copied();
        }
        // Out-of-order or duplicate timestamps would make an empty or
        // backwards span. Drop those tracks and keep the rest of the sheet.
        file.tracks
            .retain(|t| t.span.end_ms.is_none_or(|end| end > t.span.start_ms));
    }
    sheet.files.retain(|file| !file.tracks.is_empty());

    // A track that never named a performer belongs to whoever made the
    // album, which is the common case: only compilations bother repeating
    // PERFORMER per track.
    for file in &mut sheet.files {
        for track in &mut file.tracks {
            if track.performer.is_empty() {
                track.performer = sheet.performer.clone();
            }
        }
    }

    (!sheet.files.is_empty()).then_some(sheet)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD: &str = r#"REM GENRE "Alternative Rock"
REM DATE 1997
REM COMMENT "ExactAudioCopy v1.3"
PERFORMER "The Verve"
TITLE "Urban Hymns"
FILE "Urban Hymns.flac" WAVE
  TRACK 01 AUDIO
    TITLE "Bitter Sweet Symphony"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Sonnet"
    PERFORMER "Richard Ashcroft"
    INDEX 00 05:58:00
    INDEX 01 05:58:37
  TRACK 03 AUDIO
    TITLE "The Rolling People"
    INDEX 01 10:20:11
"#;

    #[test]
    fn reads_album_tags_and_spans() {
        let sheet = parse(STANDARD.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.title, "Urban Hymns");
        assert_eq!(sheet.performer, "The Verve");
        assert_eq!(sheet.genre, "Alternative Rock");
        assert_eq!(sheet.year, 1997);
        assert_eq!(sheet.files.len(), 1);

        let file = &sheet.files[0];
        assert_eq!(file.path, "Urban Hymns.flac");
        let numbers: Vec<u16> = file.tracks.iter().map(|t| t.number).collect();
        assert_eq!(numbers, [1, 2, 3]);

        // 5:58 and 37 frames is 358000 + 493ms, the frame division floored.
        assert_eq!(file.tracks[0].span.start_ms, 0);
        assert_eq!(file.tracks[0].span.end_ms, Some(358_493));
        assert_eq!(file.tracks[1].span.start_ms, 358_493);
        // 10:20 and 11 frames is 620000 + 146ms.
        assert_eq!(file.tracks[1].span.end_ms, Some(620_146));
        assert_eq!(file.tracks[2].span.start_ms, 620_146);
        assert_eq!(file.tracks[2].span.end_ms, None, "last track runs to EOF");

        assert_eq!(file.tracks[0].title, "Bitter Sweet Symphony");
        assert_eq!(file.tracks[0].performer, "The Verve", "falls back to album");
        assert_eq!(file.tracks[1].performer, "Richard Ashcroft");
    }

    #[test]
    fn span_len_matches_the_gap() {
        let sheet = parse(STANDARD.as_bytes()).expect("sheet parses");
        let tracks = &sheet.files[0].tracks;
        assert_eq!(tracks[0].span.len_ms(), Some(358_493));
        assert_eq!(tracks[1].span.len_ms(), Some(261_653));
        assert_eq!(tracks[2].span.len_ms(), None);
    }

    #[test]
    fn decodes_windows_1252_bytes() {
        // Raw cp1252, not UTF-8: 0xE9 is e-acute, 0xF6 is o-umlaut, 0x92 is
        // the curly apostrophe that only cp1252 puts in that slot.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PERFORMER \"Bj");
        bytes.push(0xF6);
        bytes.extend_from_slice(b"rk\"\nTITLE \"Caf");
        bytes.push(0xE9);
        bytes.extend_from_slice(b" de Flore\"\nFILE \"live.flac\" WAVE\n");
        bytes.extend_from_slice(b"  TRACK 01 AUDIO\n    TITLE \"L");
        bytes.push(0x92);
        bytes.extend_from_slice(b"amour\"\n    INDEX 01 00:00:00\n");

        assert!(
            std::str::from_utf8(&bytes).is_err(),
            "fixture has to be invalid UTF-8 or it never hits the fallback"
        );
        let sheet = parse(&bytes).expect("sheet parses");
        assert_eq!(sheet.performer, "Björk");
        assert_eq!(sheet.title, "Café de Flore");
        assert_eq!(sheet.files[0].tracks[0].title, "L\u{2019}amour");
    }

    #[test]
    fn keeps_utf8_when_it_is_valid() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            "PERFORMER \"Sigur Rós\"\nFILE \"a.flac\" WAVE\n\
             TRACK 01 AUDIO\nINDEX 01 00:00:00\n"
                .as_bytes(),
        );
        let sheet = parse(&bytes).expect("sheet parses");
        assert_eq!(sheet.performer, "Sigur Rós", "BOM stripped, UTF-8 kept");
    }

    #[test]
    fn ends_do_not_cross_file_boundaries() {
        let text = "FILE \"disc1.flac\" WAVE\n\
                    TRACK 01 AUDIO\nINDEX 01 00:00:00\n\
                    TRACK 02 AUDIO\nINDEX 01 03:00:00\n\
                    FILE \"disc2.flac\" WAVE\n\
                    TRACK 03 AUDIO\nINDEX 01 00:00:00\n\
                    TRACK 04 AUDIO\nINDEX 01 02:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.files.len(), 2);
        assert_eq!(sheet.files[0].tracks[0].span.end_ms, Some(180_000));
        assert_eq!(
            sheet.files[0].tracks[1].span.end_ms, None,
            "last track of disc 1 runs to its own EOF"
        );
        assert_eq!(sheet.files[1].tracks[0].span.start_ms, 0);
        assert_eq!(sheet.files[1].tracks[0].span.end_ms, Some(120_000));
        assert_eq!(sheet.files[1].tracks[1].span.end_ms, None);
    }

    #[test]
    fn falls_back_to_index_00() {
        let text = "FILE \"a.flac\" WAVE\n\
                    TRACK 01 AUDIO\nINDEX 01 00:00:00\n\
                    TRACK 02 AUDIO\nINDEX 00 01:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.files[0].tracks[1].span.start_ms, 60_000);
    }

    #[test]
    fn skips_data_tracks_whole() {
        let text = "FILE \"mixed.bin\" BINARY\n\
                    TRACK 01 AUDIO\nTITLE \"Song\"\nINDEX 01 00:00:00\n\
                    TRACK 02 MODE1/2352\nTITLE \"Data\"\nINDEX 01 02:00:00\n\
                    TRACK 03 AUDIO\nTITLE \"Other\"\nINDEX 01 04:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        let tracks = &sheet.files[0].tracks;
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].number, 1);
        assert_eq!(tracks[1].number, 3, "numbers stay the sheet's own");
        assert_eq!(
            tracks[0].span.end_ms,
            Some(240_000),
            "the data track's index never becomes a boundary"
        );
        assert_eq!(
            sheet.title, "",
            "the data track's title stays off the album"
        );
    }

    #[test]
    fn reads_bare_and_quoted_arguments() {
        let text = "TITLE Nevermind\n\
                    PERFORMER \"Nirvana\"\n\
                    FILE nevermind.flac WAVE\n\
                    TRACK 01 AUDIO\nTITLE \"Smells Like Teen Spirit\"\nINDEX 01 00:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.title, "Nevermind");
        assert_eq!(sheet.performer, "Nirvana");
        assert_eq!(sheet.files[0].path, "nevermind.flac");
        assert_eq!(sheet.files[0].tracks[0].title, "Smells Like Teen Spirit");
    }

    #[test]
    fn ignores_junk_and_unknown_commands() {
        let text = "CATALOG 0602527915100\n\
                    REM RIPPER whatever\n\
                    \n\
                    @@@ not a command @@@\n\
                    FILE \"a.flac\" WAVE\n\
                    TRACK 01 AUDIO\n\
                    FLAGS DCP\n\
                    ISRC GBAYE9700251\n\
                    SONGWRITER \"Someone\"\n\
                    PREGAP 00:02:00\n\
                    INDEX 01 00:00:00\n\
                    INDEX 02 00:30:00\n\
                    POSTGAP 00:01:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.files[0].tracks.len(), 1);
        assert_eq!(
            sheet.files[0].tracks[0].span.start_ms, 0,
            "pregap and INDEX 02 leave the start alone"
        );
    }

    #[test]
    fn commands_are_case_insensitive() {
        let text = "rem genre Jazz\n\
                    rem date 1959-08-17\n\
                    title \"Kind of Blue\"\n\
                    performer \"Miles Davis\"\n\
                    file \"kob.flac\" wave\n\
                    track 01 audio\n\
                    title \"So What\"\n\
                    index 01 00:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.title, "Kind of Blue");
        assert_eq!(sheet.genre, "Jazz");
        assert_eq!(sheet.year, 1959);
        assert_eq!(sheet.files[0].tracks[0].title, "So What");
    }

    #[test]
    fn tolerates_crlf_and_indentation() {
        let text = "TITLE \"Album\"\r\nFILE \"a.flac\" WAVE\r\n\
                    \tTRACK 01 AUDIO\r\n\t\tINDEX 01 00:00:00\r\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.files[0].path, "a.flac");
        assert_eq!(sheet.files[0].tracks.len(), 1);
    }

    #[test]
    fn drops_tracks_without_an_index() {
        let text = "FILE \"a.flac\" WAVE\n\
                    TRACK 01 AUDIO\nTITLE \"No index\"\n\
                    TRACK 02 AUDIO\nINDEX 01 01:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.files[0].tracks.len(), 1);
        assert_eq!(sheet.files[0].tracks[0].number, 2);
    }

    #[test]
    fn drops_zero_length_and_backwards_spans() {
        let text = "FILE \"a.flac\" WAVE\n\
                    TRACK 01 AUDIO\nINDEX 01 01:00:00\n\
                    TRACK 02 AUDIO\nINDEX 01 01:00:00\n\
                    TRACK 03 AUDIO\nINDEX 01 05:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        let numbers: Vec<u16> = sheet.files[0].tracks.iter().map(|t| t.number).collect();
        assert_eq!(numbers, [2, 3], "track 1 ends where it starts, so it goes");
    }

    #[test]
    fn drops_files_with_no_surviving_tracks() {
        let text = "FILE \"empty.flac\" WAVE\n\
                    FILE \"real.flac\" WAVE\n\
                    TRACK 01 AUDIO\nINDEX 01 00:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.files.len(), 1);
        assert_eq!(sheet.files[0].path, "real.flac");
    }

    #[test]
    fn no_audio_tracks_is_none() {
        let data_only = "FILE \"disc.bin\" BINARY\n\
                         TRACK 01 MODE1/2352\nINDEX 01 00:00:00\n";
        assert!(parse(data_only.as_bytes()).is_none());
        assert!(parse(b"").is_none());
        assert!(parse(b"this is not a cue sheet at all\n").is_none());
        let no_index = "FILE \"a.flac\" WAVE\nTRACK 01 AUDIO\nTITLE \"x\"\n";
        assert!(parse(no_index.as_bytes()).is_none());
    }

    #[test]
    fn missing_album_tags_are_empty() {
        let text = "FILE \"a.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n";
        let sheet = parse(text.as_bytes()).expect("sheet parses");
        assert_eq!(sheet.title, "");
        assert_eq!(sheet.performer, "");
        assert_eq!(sheet.genre, "");
        assert_eq!(sheet.year, 0);
        assert_eq!(sheet.files[0].tracks[0].performer, "");
    }

    #[test]
    fn frame_rounding_floors() {
        assert_eq!(parse_time("00:00:74"), Some(986));
        assert_eq!(parse_time("00:00:75"), Some(1000));
        assert_eq!(parse_time("00:00:01"), Some(13));
        assert_eq!(parse_time("99:59:00"), Some(5_999_000));
        assert_eq!(parse_time("01:30"), Some(90_000), "frames may be absent");
        assert_eq!(parse_time("nope"), None);
    }

    #[test]
    fn year_reading_takes_the_four_digit_run() {
        assert_eq!(first_year("1997"), 1997);
        assert_eq!(first_year("1997-05-01"), 1997);
        assert_eq!(first_year("05/1997"), 1997);
        assert_eq!(first_year("no year here"), 0);
        assert_eq!(first_year("123456"), 0, "a longer run isn't a year");
    }

    #[test]
    fn track_key_fragments_round_trip() {
        let plain = TrackKey::from(PathBuf::from("/m/album.flac"));
        assert_eq!(plain.to_fragment(), "/m/album.flac");
        assert_eq!(
            TrackKey::from_fragment(&plain.to_fragment(), |s| s == "/m/album.flac"),
            plain
        );

        let cue = TrackKey {
            source: local(),
            path: PathBuf::from("/m/album.flac"),
            sub: 7,
        };
        assert_eq!(cue.to_fragment(), "/m/album.flac#7");
        assert_eq!(
            TrackKey::from_fragment(&cue.to_fragment(), |s| s == "/m/album.flac"),
            cue
        );

        // A file that really is named `...#2` wins over the cue reading.
        let literal = TrackKey::from(PathBuf::from("/m/track#2"));
        assert_eq!(
            TrackKey::from_fragment("/m/track#2", |s| s == "/m/track#2"),
            literal
        );
    }

    #[test]
    fn non_local_fragments_carry_their_source() {
        let remote = TrackKey {
            source: Arc::from("subsonic:home"),
            path: PathBuf::from("tr-1042"),
            sub: 0,
        };
        assert_eq!(remote.to_fragment(), "subsonic:home|tr-1042");
        assert_eq!(
            TrackKey::from_fragment(&remote.to_fragment(), |_| false),
            remote
        );

        let remote_cue = TrackKey {
            source: Arc::from("subsonic:home"),
            path: PathBuf::from("tr-1042"),
            sub: 3,
        };
        assert_eq!(remote_cue.to_fragment(), "subsonic:home|tr-1042#3");
        assert_eq!(
            TrackKey::from_fragment(&remote_cue.to_fragment(), |_| false),
            remote_cue
        );
    }

    #[test]
    fn origins_read_off_the_source_string() {
        assert_eq!(Origin::of("local"), Origin::Local);
        assert_eq!(Origin::of("radio"), Origin::Radio);
        assert_eq!(Origin::of("subsonic:9f2a1c"), Origin::Subsonic);

        // A source nobody has taught this about draws like a plain track
        // rather than borrowing a station's live handling.
        assert_eq!(Origin::of("tidal:abc"), Origin::Local);
        assert_eq!(Origin::of(""), Origin::Local);

        // The prefix is a prefix, not the whole string: a source that only
        // spells the word is not a server.
        assert_eq!(Origin::of("subsonic"), Origin::Local);
    }

    #[test]
    fn keys_carry_their_origin() {
        assert_eq!(
            TrackKey::from(PathBuf::from("/m/a.flac")).origin(),
            Origin::Local
        );

        let station = TrackKey {
            source: Arc::from("radio"),
            path: PathBuf::from("https://stream.example/live"),
            sub: 0,
        };
        assert_eq!(station.origin(), Origin::Radio);

        let served = TrackKey {
            source: Arc::from("subsonic:home"),
            path: PathBuf::from("tr-1042"),
            sub: 0,
        };
        assert_eq!(served.origin(), Origin::Subsonic);
    }

    #[test]
    fn a_path_with_a_pipe_in_it_stays_local() {
        // `|` is legal in a file name, so the prefix split only happens when
        // the whole string isn't a path the caller recognises.
        let piped = TrackKey::from(PathBuf::from("/m/a|b.flac"));
        assert_eq!(piped.to_fragment(), "/m/a|b.flac");
        assert_eq!(
            TrackKey::from_fragment("/m/a|b.flac", |s| s == "/m/a|b.flac"),
            piped
        );
    }
}
